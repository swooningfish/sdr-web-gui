#!/usr/bin/env bash
# Regenerates the README screenshots in docs/screenshots from a running sdrd,
# by default the dev stack on localhost:8080 (simulated receiver). Runs in the
# official Playwright image, so nothing needs installing locally.
#   ./scripts/screenshots.sh
set -euo pipefail
cd "$(dirname "$0")/.."
URL=${SDR_URL:-http://host.docker.internal:8080}
ROOT="$PWD"
# Git Bash on Windows: see ui-test.sh.
if pwd -W >/dev/null 2>&1; then ROOT="$(pwd -W)"; export MSYS_NO_PATHCONV=1; fi
mkdir -p docs/screenshots
docker run --rm --add-host host.docker.internal:host-gateway \
  -e SDR_URL="$URL" -e SDR_USER="${SDR_USER:-admin}" -e SDR_PASS="${SDR_PASS:-devpassword123}" \
  -v "$ROOT/scripts/screenshots.mjs:/work/screenshots.mjs:ro" \
  -v "$ROOT/docs/screenshots:/out" \
  mcr.microsoft.com/playwright:v1.49.1-noble \
  sh -c 'cd /work && npm init -y >/dev/null && npm i --silent playwright@1.49.1 >/dev/null \
    && node screenshots.mjs'
