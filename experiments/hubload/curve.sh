#!/usr/bin/env bash
# Per-unit costs from simulated machines (klark-loadtest, fleet token), on the
# same hub, to fit a scaling curve. Browser 1 must be up (kl.sh 1 up).
#   RUN=$K/curve curve.sh
. "$(dirname "$0")/env.sh"
H=$(dirname "$0")
RUN=${RUN:-$K/curve}
mkdir -p "$RUN"
mark() { echo "$1 $(date +%s)" >> "$RUN/phases.txt"; echo "== $1"; }
kl() { bash "$H/kl.sh" "$@"; }
pane() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
lt() { "$LT" --hub ws://127.0.0.1:5100/agent --token kload-fleet --rate 20000 --panes 12 --interval-ms 2000 "$@"; }

mark c0_baseline
sleep 30
# Idle machines, nobody looking: 25, then +75 = 100.
lt --machines 25 --prefix a --seconds 900 --csv "$RUN/lt-a.csv" > "$RUN/lt-a.log" 2>&1 &
A=$!
sleep 5; mark c1_idle25; sleep 45
lt --machines 75 --prefix b --seconds 840 --csv "$RUN/lt-b.csv" > "$RUN/lt-b.log" 2>&1 &
B=$!
sleep 5; mark c2_idle100; sleep 60
# 4 list viewers over 100 sim machines (+ the real Mac if linked).
kl 1 'KL.open("list","/",4); return 1' >/dev/null
sleep 3; mark c3_list4_100; sleep 60
# 10 chat viewers on 10 different sim machines' agent panes: 10 transcript polls.
for i in 0 1 2 3 4 5 6 7 8 9; do kl 1 "KL.open('chat','/m/$(pane load:a-00$i)/p/p0?view=chat',1); return 1" >/dev/null; done
sleep 3; mark c4_chat10; sleep 60
# The same 10 panes watched by 3 more chat viewers each: polls must stay at 10.
for i in 0 1 2 3 4 5 6 7 8 9; do kl 1 "KL.open('chat','/m/$(pane load:a-00$i)/p/p0?view=chat',3); return 1" >/dev/null; done
sleep 3; mark c5_chat40_same10; sleep 60
kl 1 'KL.close("chat"); return 1' >/dev/null
# 10 streaming panes at 20 KB/s (+bursts), one terminal viewer each.
for i in 0 1 2 3 4 5 6 7 8 9; do kl 1 "KL.open('term','/m/$(pane load:b-00$i)/p/p1?view=terminal',1); return 1" >/dev/null; done
sleep 3; mark c6_stream10; sleep 60
echo "stream10 stats: $(kl 1 'return KL.stats()')" >> "$RUN/notes.txt"
# One hot pane, 10 more viewers on it: one machine stream fanned out.
kl 1 "KL.open('term','/m/$(pane load:b-000)/p/p1?view=terminal',10); return 1" >/dev/null
sleep 3; mark c7_stream10_fan10; sleep 60
echo "fan stats: $(kl 1 'return KL.stats()')" >> "$RUN/notes.txt"
kl 1 'KL.close(); return 1' >/dev/null
sleep 3; mark c8_leave_100idle; sleep 60
kill $A $B 2>/dev/null; pkill -f "klark-loadtest --hub ws://127.0.0.1:5100/agent --token kload-fleet"; wait
mark c9_machines_gone; sleep 45
mark c_end
