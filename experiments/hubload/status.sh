#!/usr/bin/env bash
# One line per session: agent pane status; then counts of claude agents and pane observers.
#   status.sh [phase-name]   (with a name, also appends "<name> <epoch>" to $K/run/phases.txt)
. "$(dirname "$0")/env.sh"
[ -n "${1:-}" ] && echo "$1 $(date +%s)" >> "${RUN:-$K/run1}/phases.txt"
for d in "$K"/xdg/klark/sessions/s*/; do
  s=$(basename "$d")
  "$B" --session "$s" pane list 2>/dev/null | python3 -c '
import json,sys
ps=json.load(sys.stdin)["result"]["panes"]
print(sys.argv[1], " ".join("%s:%s:%s" % (p["pane_id"], p.get("agent") or "-", p.get("agent_status")) for p in ps))' "$s"
done
echo "claude agents: $(pgrep -f 'claude --model haiku' | wc -l | tr -d ' ')  observers: $(pgrep -f 'terminal session observe' | wc -l | tr -d ' ')  epoch: $(date +%s)"
