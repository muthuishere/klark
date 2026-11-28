#!/usr/bin/env bash
# Check how the hub renders every pane, in ONE browser tab (chrome-agent): for each pane, open its chat page and run
# scripts/render-check.js (scrolls the windowed chat, checks every row). Prints one JSON report per pane; exits 1
# when any pane has a problem.
# Usage: HUB=http://127.0.0.1:7471 MACHINE=mac scripts/render-check.sh <session/pane>...
# Sign in to the hub in that tab first (or set HUB_USER/HUB_PASSWORD to sign in through the API from the page).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
hub=${HUB:-http://127.0.0.1:7470}; machine=${MACHINE:-mac}
export CHROME_AGENT_ID=${CHROME_AGENT_ID:-klark-render-check}
check=$(cat "$here/render-check.js")
chrome-agent goto "$hub/login" >/dev/null
if [ -n "${HUB_USER:-}" ]; then
  chrome-agent evalAsync "const r=await fetch('api/auth/login',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({username:'$HUB_USER',password:'${HUB_PASSWORD:-}'})});return r.status" >/dev/null
fi
fail=0
for id in "$@"; do
  enc=$(python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$id")
  chrome-agent goto "$hub/m/$machine/p/$enc" >/dev/null
  out=$(chrome-agent evalAsync "const EXPECT_PANE=$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$id");
$check" | tail -1)
  echo "$out"
  printf '%s' "$out" | grep -q '"ok":true' || fail=1
done
exit $fail
