#!/usr/bin/env bash
# Browser layout tests (Playwright) against a running sdrd, by default the dev
# stack on localhost:8080. Runs in the official Playwright image, so nothing
# needs installing locally.
#   ./scripts/ui-test.sh [extra playwright args, e.g. --project=phone-375]
set -euo pipefail
cd "$(dirname "$0")/.."
URL=${SDR_URL:-http://host.docker.internal:8080}
docker run --rm --add-host host.docker.internal:host-gateway \
  -e SDR_URL="$URL" -e SDR_USER="${SDR_USER:-admin}" -e SDR_PASS="${SDR_PASS:-devpassword123}" \
  -v "$PWD/web:/src:ro" \
  mcr.microsoft.com/playwright:v1.49.1-noble \
  sh -c 'mkdir -p /work && cp -r /src/playwright.config.ts /src/tests /work/ && cd /work \
    && npm init -y >/dev/null && npm i -D --silent @playwright/test@1.49.1 >/dev/null \
    && npx playwright test "$@"' -- "$@"
