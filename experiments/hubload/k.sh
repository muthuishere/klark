#!/usr/bin/env bash
# Run the worktree's klark against the load test's sessions: k.sh --session s01 pane list
. "$(dirname "$0")/env.sh"
exec "$B" "$@"
