#!/usr/bin/env bash
# Stop everything the load test started: klark sessions (and the claude
# agents inside them), klark expose, loadtest, samplers, the hub container
# and its volume, and the throwaway dirs.
. "$(dirname "$0")/env.sh"
for d in "$K"/xdg/klark/sessions/*/; do
  s=$(basename "$d"); "$B" --session "$s" server stop >/dev/null 2>&1 || true
done
sleep 1
pkill -f "$K/xdg" 2>/dev/null || true
pkill -f "klark --session s[0-9][0-9] server" 2>/dev/null || true
pkill -f "klark expose --hub ws://127.0.0.1:5100" 2>/dev/null || true
pkill -f "klark-loadtest --hub ws://127.0.0.1:5100" 2>/dev/null || true
pkill -f "hubload/sample.sh" 2>/dev/null || true
# claude agents started in the work dirs
for p in $(pgrep -f "claude --model haiku" 2>/dev/null); do
  case "$(lsof -a -p "$p" -d cwd -Fn 2>/dev/null | grep '^n')" in *"$K/work"*) kill "$p" ;; esac
done
if [ "${KEEP_HUB:-}" != 1 ]; then
  docker rm -f kload-hub >/dev/null 2>&1 || true
  docker volume rm kload-hub-data >/dev/null 2>&1 || true
fi
