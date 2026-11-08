#!/usr/bin/env bash
# API smoke test against a running sdrd (e.g. docker-compose.dev.yml).
#   ./scripts/smoke-test.sh [base-url] [user] [password]
set -uo pipefail

BASE=${1:-http://localhost:8080}
USER_NAME=${2:-admin}
PASS=${3:-devpassword123}
JAR=$(mktemp)
trap 'rm -f "$JAR"' EXIT
fails=0

check() { # name expected actual
  if [[ "$2" == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1 (expected $2, got $3)"; fails=$((fails + 1)); fi
}
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

check "health"                 200 "$(code "$BASE/api/health")"
check "SPA index"              200 "$(code "$BASE/")"
check "SPA deep link"          200 "$(code "$BASE/some/client/route")"
check "unknown api route 404"  404 "$(code "$BASE/api/nope")"
check "radio needs auth"       401 "$(code "$BASE/api/radio")"
check "bad login"              401 "$(code -H 'Content-Type: application/json' \
  -d "{\"username\":\"$USER_NAME\",\"password\":\"wrong-password\"}" "$BASE/api/auth/login")"
check "cross-origin login"     403 "$(code -H 'Content-Type: application/json' -H 'Origin: https://evil.example' \
  -d "{\"username\":\"$USER_NAME\",\"password\":\"$PASS\"}" "$BASE/api/auth/login")"

LOGIN=$(curl -s -c "$JAR" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$USER_NAME\",\"password\":\"$PASS\"}" "$BASE/api/auth/login")
CSRF=$(sed -n 's/.*"csrf":"\([0-9a-f]*\)".*/\1/p' <<<"$LOGIN")
check "login returns csrf"     64 "${#CSRF}"
check "me"                     200 "$(code -b "$JAR" "$BASE/api/auth/me")"
check "radio state"            200 "$(code -b "$JAR" "$BASE/api/radio")"
check "tune without csrf"      403 "$(code -b "$JAR" -H 'Content-Type: application/json' \
  -d '{"center_hz":433920000}' "$BASE/api/radio")"
check "tune invalid freq"      400 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"center_hz":5}' "$BASE/api/radio")"
check "tune 433.92"            200 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"center_hz":433920000,"sample_rate":1024000}' "$BASE/api/radio")"
sleep 0.3
STATE=$(curl -s -b "$JAR" "$BASE/api/radio")
grep -q '"center_hz":433920000' <<<"$STATE"; check "retune applied" 0 $?
check "ws upgrade"             101 "$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 -b "$JAR" \
  -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$BASE/api/ws")"
check "ws cross-origin"        403 "$(code --max-time 2 -b "$JAR" -H 'Origin: https://evil.example' \
  -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
  -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' "$BASE/api/ws")"
# restore default tuning
code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"center_hz":145300000,"sample_rate":2048000}' "$BASE/api/radio" >/dev/null
check "logout"                 200 "$(code -b "$JAR" -X POST -H "X-CSRF-Token: $CSRF" "$BASE/api/auth/logout")"
check "session gone"           401 "$(code -b "$JAR" "$BASE/api/auth/me")"

echo
if [[ $fails -eq 0 ]]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
