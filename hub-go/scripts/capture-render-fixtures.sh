#!/usr/bin/env bash
# Capture real agent transcripts for the rendering test (hub-go/chat/render_real_test.go).
# Usage: scripts/capture-render-fixtures.sh <out-dir> <session/pane>...
# Each pane's "klark transcript" output (what the hub receives) is saved as <agent>--<session>--<pane>.json.
# These are private conversations: keep <out-dir> outside the repo.
set -euo pipefail
out=$1; shift
mkdir -p "$out"
klark=${KLARK_BIN:-klark}
for id in "$@"; do
  session=${id%%/*}; pane=${id#*/}
  json=$("$klark" --session "$session" transcript --pane "$pane" --limit 400 --json) || { echo "skip $id" >&2; continue; }
  agent=$(printf '%s' "$json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("agent") or "unknown")')
  name="$agent--$session--${pane//:/_}.json"
  printf '%s' "$json" > "$out/$name"
  echo "$out/$name"
done
