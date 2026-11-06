#!/usr/bin/env bash
# Streaming part of the curve: 100 sim machines, 10 streaming panes at
# 20 KB/s (+ 4x bursts), then one of them fanned out to 10 more viewers in a
# second browser, then everyone leaves, then the machines go.
# Browsers 1 and 2 must be up (kl.sh N up). At most ~20 iframes per browser:
# 44 iframes in one headless tab crashed it in the first attempt.
#   RUN=$K/curve2 curve2.sh
. "$(dirname "$0")/env.sh"
H=$(dirname "$0")
RUN=${RUN:-$K/curve2}
mkdir -p "$RUN"
mark() { echo "$1 $(date +%s)" >> "$RUN/phases.txt"; echo "== $1"; }
kl() { bash "$H/kl.sh" "$@"; }
pane() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
"$LT" --hub ws://127.0.0.1:5100/agent --token kload-fleet --rate 20000 --panes 12 --interval-ms 2000 \
  --machines 100 --prefix s --seconds 600 --csv "$RUN/lt.csv" > "$RUN/lt.log" 2>&1 &
L=$!
sleep 10; mark s1_idle100; sleep 50
for i in 0 1 2 3 4 5 6 7 8 9; do kl 1 "KL.open('term','/m/$(pane load:s-00$i)/p/p1?view=terminal',1); return 1" >/dev/null; done
sleep 3; mark s2_stream10; sleep 60
echo "s2 browser1: $(kl 1 'return KL.stats()')" >> "$RUN/notes.txt"
kl 2 "KL.open('term','/m/$(pane load:s-000)/p/p1?view=terminal',10); return 1" >/dev/null
sleep 3; mark s3_fan10; sleep 60
echo "s3 browser1: $(kl 1 'return KL.stats()')" >> "$RUN/notes.txt"
echo "s3 browser2: $(kl 2 'return KL.stats()')" >> "$RUN/notes.txt"
kl 1 'return KL.close()' >/dev/null; kl 2 'return KL.close()' >/dev/null
sleep 3; mark s4_leave; sleep 90
kill $L; wait $L
mark s5_machines_gone; sleep 60
mark s_end
