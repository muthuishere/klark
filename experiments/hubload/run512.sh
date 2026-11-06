#!/usr/bin/env bash
# Phase 7: the same phases on a small box (docker --memory 512m --cpus 1),
# with the 10 real sessions + expose still running AND 100 idle simulated
# machines linked, so the list and /api/agents cover 101 machines.
#   RUN=$K/run512 run512.sh
. "$(dirname "$0")/env.sh"
H=$(dirname "$0")
export RUN=${RUN:-$K/run512}
mkdir -p "$RUN"
bash "$H/hub-up.sh" --memory 512m --cpus 1
EVERY=2 bash "$H/sample.sh" "$RUN/hub.csv" &
S=$!
echo "c_baseline $(date +%s)" >> "$RUN/phases.txt"
sleep 20
"$LT" --hub ws://127.0.0.1:5100/agent --token kload-fleet --rate 20000 --panes 12 --interval-ms 2000 \
  --machines 100 --prefix m --seconds 900 --csv "$RUN/lt.csv" > "$RUN/lt.log" 2>&1 &
L=$!
sleep 10
echo "p1_noviewers_101 $(date +%s)" >> "$RUN/phases.txt"
sleep 50
bash "$H/kl.sh" 1 up >/dev/null; bash "$H/kl.sh" 2 up >/dev/null
bash "$H/phases.sh" mac
kill $L; wait $L
echo "machines_gone $(date +%s)" >> "$RUN/phases.txt"
sleep 30
echo "end $(date +%s)" >> "$RUN/phases.txt"
docker inspect kload-hub --format 'OOMKilled={{.State.OOMKilled}} Restarts={{.RestartCount}}' | tee -a "$RUN/notes.txt"
kill $S
