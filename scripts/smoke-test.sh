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
# --- milestone 2: sensors / rtl_433
check "devices"                200 "$(code -b "$JAR" "$BASE/api/devices")"
check "recent events"          200 "$(code -b "$JAR" "$BASE/api/events")"
check "rtl_433 config"         200 "$(code -b "$JAR" "$BASE/api/decoders/rtl_433")"
check "APRS config"            200 "$(code -b "$JAR" "$BASE/api/aprs")"
check "APRS rejects 5 channels" 400 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"channels":[{"label":"a","frequency_hz":144800000,"enabled":true},{"label":"b","frequency_hz":144800000,"enabled":true},{"label":"c","frequency_hz":144800000,"enabled":true},{"label":"d","frequency_hz":144800000,"enabled":true},{"label":"e","frequency_hz":144800000,"enabled":true}]}' \
  "$BASE/api/aprs")"
CSP=$(curl -sI "$BASE/" | tr -d '\r' | grep -i '^content-security-policy:')
grep -q "img-src 'self' data: blob: https://tile.openstreetmap.org" <<<"$CSP"; check "CSP allows map tiles" 0 $?
CHECK=$(curl -s -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"spec":"n=x,m=NOPE,s=1"}' "$BASE/api/decoders/rtl_433/check")
grep -q '"ok":false' <<<"$CHECK"; check "rtl_433 rejects bad flex spec" 0 $?
check "save rejects bad flex"  400 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"frequency_hz":433920000,"protocols":[],"flex":[{"spec":"n=x,m=NOPE","enabled":true}]}' \
  "$BASE/api/decoders/rtl_433")"
VIEWER="smoke_viewer_$$"
code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$VIEWER\",\"password\":\"viewer-password-123\",\"role\":\"viewer\"}" "$BASE/api/users" >/dev/null
VJAR=$(mktemp)
curl -s -o /dev/null -c "$VJAR" -H 'Content-Type: application/json' \
  -d "{\"username\":\"$VIEWER\",\"password\":\"viewer-password-123\"}" "$BASE/api/auth/login"
check "viewer can't see sensors" 403 "$(code -b "$VJAR" "$BASE/api/devices")"
check "viewer can't see rtl_433 devices" 403 "$(code -b "$VJAR" "$BASE/api/devices?decoder=rtl_433")"
# --- milestone 3: APRS is public amateur traffic, viewers may see it
check "viewer sees APRS stations" 200 "$(code -b "$VJAR" "$BASE/api/devices?decoder=aprs")"
check "viewer sees APRS config"   200 "$(code -b "$VJAR" "$BASE/api/aprs")"
check "viewer sees APRS tracks"   200 "$(code -b "$VJAR" "$BASE/api/aprs/tracks")"
check "viewer can't change APRS"  403 "$(code -X PUT -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"channels":[]}' "$BASE/api/aprs")"
rm -f "$VJAR"
VID=$(curl -s -b "$JAR" "$BASE/api/users" | grep -o "{\"id\":[0-9]*,\"username\":\"$VIEWER\"" | grep -o '[0-9]\+' | head -1)
[[ -n "$VID" ]] && code -X DELETE -b "$JAR" -H "X-CSRF-Token: $CSRF" "$BASE/api/users/$VID" >/dev/null

# restore default tuning
code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"center_hz":145300000,"sample_rate":2048000}' "$BASE/api/radio" >/dev/null
check "logout"                 200 "$(code -b "$JAR" -X POST -H "X-CSRF-Token: $CSRF" "$BASE/api/auth/logout")"
check "session gone"           401 "$(code -b "$JAR" "$BASE/api/auth/me")"

echo
if [[ $fails -eq 0 ]]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
