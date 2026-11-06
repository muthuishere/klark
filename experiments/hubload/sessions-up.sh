#!/usr/bin/env bash
# Start N real klark sessions (default 10), each with three panes:
#   left  : a real Claude Code agent on Haiku doing a small, local task
#   right : an output generator (s01's is the "hot" pane, ~20 KB/s; the rest ~1 KB/s)
#   down  : an idle shell
# Throwaway work dirs under $K/work/sNN; a project .claude/settings.json allows
# only python, and Write/Edit inside that dir.
#   sessions-up.sh [N] [--no-claude]
set -euo pipefail
. "$(dirname "$0")/env.sh"
N=${1:-10}
NOCLAUDE=${2:-}
unset CLAUDECODE CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN \
  CLAUDE_CODE_BRIDGE_SESSION_ID CLAUDE_CODE_EXECPATH CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION \
  CLAUDE_CODE_SESSION_ATTENDED CLAUDE_PID CLAUDE_EFFORT
PROMPT='Write fizzbuzz.py (a fizzbuzz function plus a main that prints 1..30) and test_fizzbuzz.py using unittest in this directory only. Run the tests with python3 -m unittest. Then explain the code briefly. Do not use the network or touch files outside this directory.'
for i in $(seq -w 1 "$N"); do
  s=s$i
  [ ${#i} -lt 2 ] && s=s0$i
  W=$K/work/$s
  mkdir -p "$W/.claude"
  cat > "$W/.claude/settings.json" <<JSON
{ "permissions": { "allow": ["Bash(python3:*)", "Bash(python:*)", "Write(./**)", "Edit(./**)", "Read(./**)"],
                   "deny": ["WebFetch", "WebSearch"] } }
JSON
  KLARK_SESSION=$s nohup "$B" --session $s server > "$K/srv-$s.log" 2>&1 &
  for _ in $(seq 40); do [ -S "$K/xdg/klark/sessions/$s/klark.sock" ] && break; sleep 0.25; done
  sleep 0.5
  ws=$("$B" --session $s workspace create --cwd "$W" --label "$s-work" --no-focus)
  root=$(echo "$ws" | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r["root_pane"]["pane_id"] if isinstance(r["root_pane"],dict) else r["root_pane"])')
  right=$("$B" --session $s pane split "$root" --direction right --cwd "$W" --no-focus | python3 -c 'import json,sys; p=json.load(sys.stdin)["result"]["pane"]; print(p["pane_id"] if isinstance(p,dict) else p)')
  down=$("$B" --session $s pane split "$right" --direction down --cwd "$W" --no-focus | python3 -c 'import json,sys; p=json.load(sys.stdin)["result"]["pane"]; print(p["pane_id"] if isinstance(p,dict) else p)')
  echo "$s root=$root right=$right down=$down" | tee -a "$K/panes.txt"
  sleep 1
  if [ "$s" = s01 ]; then delay=0.002; else delay=0.05; fi
  "$B" --session $s pane run "$right" "i=0; while :; do printf '\\033[3%dm%s %s line %d build step ok\\033[0m\\n' \$((RANDOM%7+1)) $s \$(date +%H:%M:%S) \$i; i=\$((i+1)); sleep $delay; done" >/dev/null
  if [ -z "$NOCLAUDE" ]; then
    "$B" --session $s pane run "$root" "claude --model haiku --permission-mode acceptEdits '$PROMPT'" >/dev/null
  fi
done
# Answer each folder-trust question ("Yes, I trust this folder").
if [ -z "$NOCLAUDE" ]; then
  while read -r s rest; do
    root=${rest#root=}; root=${root%% *}
    "$B" --session $s pane wait-output "$root" --match "trust this folder" --timeout 30 >/dev/null 2>&1 &&
      "$B" --session $s pane send-keys "$root" down enter >/dev/null
  done < "$K/panes.txt"
fi
