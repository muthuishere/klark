#!/usr/bin/env bash
# E-10: restart the hub container with 100 simulated machines (+ the real
# expose, if running) linked, and time how long until all are back.
#   RUN=$K/e10 e10.sh
. "$(dirname "$0")/env.sh"
RUN=${RUN:-$K/e10}
mkdir -p "$RUN"
"$LT" --hub ws://127.0.0.1:5100/agent --token kload-fleet --machines 100 --prefix r --reconnect \
  --seconds 240 --interval-ms 250 --csv "$RUN/lt.csv" > "$RUN/lt.log" 2>&1 &
L=$!
sleep 20
echo "restart $(date +%s.%N | cut -c1-14)" >> "$RUN/phases.txt"
t0=$(python3 -c 'import time; print(time.time())')
docker restart -t 5 kload-hub >/dev/null
t1=$(python3 -c 'import time; print(time.time())')
until curl -fsS localhost:5100/healthz >/dev/null 2>&1; do sleep 0.1; done
t2=$(python3 -c 'import time; print(time.time())')
echo "docker restart took $(python3 -c "print(round($t1-$t0,2))") s; healthz after $(python3 -c "print(round($t2-$t0,2))") s" | tee -a "$RUN/notes.txt"
wait $L
python3 - "$RUN/lt.csv" "$t0" <<'EOF' | tee -a "$RUN/notes.txt"
import csv, sys, time
rows = list(csv.DictReader(open(sys.argv[1])))
# lt.csv t_s is relative to the loadtest start; find the drop and the recovery.
drop = next((r for r in rows if int(r["connected"]) < 100 and float(r["t_s"]) > 15), None)
if not drop:
    print("no drop seen"); sys.exit()
t_drop = float(drop["t_s"])
back = next((r for r in rows if float(r["t_s"]) > t_drop and int(r["connected"]) == 100), None)
low = min(int(r["connected"]) for r in rows if float(r["t_s"]) >= t_drop)
print(f"drop at t={t_drop}s, min connected {low}, all 100 back at t={back['t_s'] if back else 'never'}s "
      f"-> {float(back['t_s']) - t_drop if back else 'n/a'} s after the drop; total connects {rows[-1]['connects']}, failures {rows[-1]['failed']}")
series = [(float(r["t_s"]) - t_drop, int(r["connected"])) for r in rows if t_drop - 1 <= float(r["t_s"]) <= t_drop + 70]
print("t_after_drop_s,connected"); [print(f"{t:.2f},{c}") for t, c in series[::4]]
EOF
