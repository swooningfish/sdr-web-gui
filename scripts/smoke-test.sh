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
check "SSTV config"            200 "$(code -b "$JAR" "$BASE/api/sstv")"
check "SSTV rejects bad demod" 422 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"channels":[{"label":"x","frequency_hz":145800000,"demod":"am","enabled":true}]}' "$BASE/api/sstv")"
check "SSTV image 404"         404 "$(code -b "$JAR" "$BASE/api/sstv/image/999999999")"
SSTV_ID=$(curl -s -b "$JAR" "$BASE/api/events?decoder=sstv&limit=1" | sed -n 's/^\[{"id":\([0-9]*\).*/\1/p')
if [[ -n "$SSTV_ID" ]]; then
  check "SSTV image is a PNG" "image/png" "$(curl -s -o /dev/null -w '%{content_type}' -b "$JAR" "$BASE/api/sstv/image/$SSTV_ID")"
  check "SSTV thumbnail"      200 "$(code -b "$JAR" "$BASE/api/sstv/image/$SSTV_ID?thumb=true")"
fi
check "schedule"               200 "$(code -b "$JAR" "$BASE/api/schedule")"
check "location out of range"  400 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"lat_deg":200,"lon_deg":0,"alt_m":0}' "$BASE/api/schedule/location")"
check "test pass too short"    400 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"seconds":1}' "$BASE/api/schedule/test")"
check "bad TLE rejected"       400 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"name":"X","line1":"1 nonsense","line2":"2 nonsense"}' "$BASE/api/schedule/tle")"
SCHED=$(curl -s -b "$JAR" "$BASE/api/schedule" | sed -n 's/^{"config":\(.*\),"status":.*/\1/p')
BAD=$(sed 's/"dwell_s":[0-9]*/"dwell_s":1/' <<<"$SCHED")
check "schedule rejects 1 s dwell" 400 "$(code -X PUT -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d "$BAD" "$BASE/api/schedule")"
check "audio for non-pass 404" 404 "$(code -b "$JAR" "$BASE/api/passes/1/audio")"
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
# --- milestone 4: SSTV is public too
check "viewer sees SSTV config"   200 "$(code -b "$VJAR" "$BASE/api/sstv")"
check "viewer sees SSTV gallery"  200 "$(code -b "$VJAR" "$BASE/api/events?decoder=sstv")"
check "viewer can't change SSTV"  403 "$(code -X PUT -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"channels":[]}' "$BASE/api/sstv")"
check "viewer can't delete images" 403 "$(code -X DELETE -b "$VJAR" "$BASE/api/sstv/image/1")"
# --- milestone 5: passes and scheduling
check "viewer sees schedule"      200 "$(code -b "$VJAR" "$BASE/api/schedule")"
check "viewer can't set location" 403 "$(code -X PUT -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"lat_deg":51.5,"lon_deg":0,"alt_m":0}' "$BASE/api/schedule/location")"
check "viewer can't start test pass" 403 "$(code -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"seconds":30}' "$BASE/api/schedule/test")"
check "viewer can't change APRS"  403 "$(code -X PUT -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"enabled":true,"channels":[]}' "$BASE/api/aprs")"
# --- band scan: admins run it, everyone sees the results
check "viewer sees scan results"  200 "$(code -b "$VJAR" "$BASE/api/scan")"
check "viewer can't start a scan" 403 "$(code -b "$VJAR" -H 'Content-Type: application/json' \
  -d '{"band":"fm"}' "$BASE/api/scan")"
rm -f "$VJAR"
VID=$(curl -s -b "$JAR" "$BASE/api/users" | grep -o "{\"id\":[0-9]*,\"username\":\"$VIEWER\"" | grep -o '[0-9]\+' | head -1)
[[ -n "$VID" ]] && code -X DELETE -b "$JAR" -H "X-CSRF-Token: $CSRF" "$BASE/api/users/$VID" >/dev/null

# --- band scan: the simulator's DAB multiplexes on 11D and 12B
check "scan rejects unknown band" 422 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"band":"lw"}' "$BASE/api/scan")"
check "start DAB scan"         200 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"band":"dab"}' "$BASE/api/scan")"
check "one scan at a time"     400 "$(code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"band":"fm"}' "$BASE/api/scan")"
for _ in $(seq 1 90); do
  SCAN=$(curl -s -b "$JAR" "$BASE/api/scan")
  grep -q '"running":false' <<<"$SCAN" && break
  sleep 1
done
grep -q '"running":false' <<<"$SCAN"; check "DAB scan finishes" 0 $?
grep -q '"id":"dab-11D-C223"' <<<"$SCAN" && grep -q '"name":"Sim News 24"' <<<"$SCAN"; check "DAB scan names services" 0 $?
grep -q '"notes":\[\]' <<<"$SCAN"; check "no notes about neighbouring channels" 0 $?
grep -q '"name":"Sim Gold"' <<<"$SCAN"; check "DAB scan finds the second multiplex" 0 $?
SAVED=$(curl -s -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"ids":["dab-11D-C223"],"shared":false}' "$BASE/api/scan/save")
grep -Eq '"(added|duplicates)":1' <<<"$SAVED"; check "save a scanned station" 0 $?
BID=$(curl -s -b "$JAR" "$BASE/api/bookmarks" | grep -o '{[^{}]*"name":"Sim News 24"[^{}]*"shared":false' | grep -o '"id":[0-9]*' | grep -o '[0-9]\+' | head -1)
[[ -n "$BID" ]]; check "scanned station is a bookmark" 0 $?
[[ -n "$BID" ]] && code -X DELETE -b "$JAR" -H "X-CSRF-Token: $CSRF" "$BASE/api/bookmarks/$BID" >/dev/null

# restore default tuning
code -b "$JAR" -H "X-CSRF-Token: $CSRF" -H 'Content-Type: application/json' \
  -d '{"center_hz":145300000,"sample_rate":2048000}' "$BASE/api/radio" >/dev/null
check "logout"                 200 "$(code -b "$JAR" -X POST -H "X-CSRF-Token: $CSRF" "$BASE/api/auth/logout")"
check "session gone"           401 "$(code -b "$JAR" "$BASE/api/auth/me")"

echo
if [[ $fails -eq 0 ]]; then echo "all checks passed"; else echo "$fails check(s) failed"; exit 1; fi
