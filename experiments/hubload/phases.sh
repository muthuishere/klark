#!/usr/bin/env bash
# Phases 2-5 of E-06 against a running hub + 10 real sessions + expose, with
# viewer browser 1 already up (kl.sh 1 up). Writes to $RUN.
#   RUN=$K/run1 phases.sh [machine-name]
. "$(dirname "$0")/env.sh"
H=$(dirname "$0")
RUN=${RUN:-$K/run1}
M=${1:-mac}
pane() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
mark() { echo "$1 $(date +%s)" >> "$RUN/phases.txt"; echo "== $1"; }
kl() { bash "$H/kl.sh" "$@"; }

if [ -z "${SKIP_LIST:-}" ]; then
  mark p2_list4
  kl 1 'KL.open("list","/",4); return KL.stats()'
  sleep 60
fi
bash "$H/approve.sh"

mark p3_chat3
for s in s02 s03 s04; do kl 1 "KL.open('chat','/m/$M/p/$(pane "$s/w1:p1")?view=chat',1); return 1" >/dev/null; done
sleep 20
for i in 1 2 3; do
  echo "chat latency $i: $(kl 1 'return await KL.chatLatency(0, "Reply with the single word OK.")')" | tee -a "$RUN/latency.txt"
  sleep 15
done
bash "$H/approve.sh"
sleep 10

mark p4_term3_api10
kl 1 "KL.open('term','/m/$M/p/$(pane s01/w1:p3)?view=terminal',1); KL.open('term','/m/$M/p/$(pane s01/w1:p2)?view=terminal',2); return 1" >/dev/null
python3 "$H/api.py" --clients 10 --seconds 60 > "$RUN/api.json" 2>&1 &
API=$!
sleep 8
for i in 1 2 3; do
  echo "type latency (idle shell, hot pane streaming) $i: $(kl 1 'return await KL.typeLatency(0,10,200)')" | tee -a "$RUN/latency.txt"
  sleep 5
done
mark p4b_slow_viewer
kl 2 "KL.open('term','/m/$M/p/$(pane s01/w1:p2)?view=terminal',1); setInterval(()=>{const t=Date.now(); while(Date.now()-t<1500);},1600); return 1" >/dev/null
sleep 20
for i in 1 2; do
  echo "type latency with a slow viewer $i: $(kl 1 'return await KL.typeLatency(0,10,200)')" | tee -a "$RUN/latency.txt"
  sleep 3
done
wait $API
echo "viewer stats browser1: $(kl 1 'return KL.stats()')" | tee -a "$RUN/latency.txt"
echo "viewer stats browser2 (slow): $(kl 2 'return KL.stats()')" | tee -a "$RUN/latency.txt"
bash "$H/status.sh" | tail -1 | tee -a "$RUN/latency.txt"

mark p5_all_leave
kl 1 'return KL.close()'
kl 2 'return KL.close()'
sleep 15
bash "$H/status.sh" | tail -1 | tee -a "$RUN/latency.txt"
sleep 75
bash "$H/status.sh" | tail -1 | tee -a "$RUN/latency.txt"
mark p5_end
