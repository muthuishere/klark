#!/usr/bin/env bash
# Approve any blocked Claude permission prompt (option 1) and optionally give
# every agent a follow-up prompt, so agents keep working during a phase.
#   approve.sh [follow-up text]
. "$(dirname "$0")/env.sh"
while read -r s rest; do
  root=${rest#root=}; root=${root%% *}
  screen=$("$B" --session $s pane read "$root" 2>/dev/null)
  if echo "$screen" | grep -q "Do you want to proceed"; then "$B" --session $s pane send-keys "$root" enter >/dev/null; fi
  if [ -n "${1:-}" ]; then "$B" --session $s pane send-text "$root" "$1" >/dev/null; "$B" --session $s pane send-keys "$root" enter >/dev/null; fi
done < "$K/panes.txt"
