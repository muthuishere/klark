#!/usr/bin/env bash
# Leak check: open 10 viewers (4 list, 3 chat, 3 terminal) for 30 s, close
# them for 45 s, N times. If hub memory keeps climbing cycle after cycle
# there is a leak; if it plateaus, the post-leave level is GC high-water.
#   RUN=$K/cycles cycles.sh [N]
. "$(dirname "$0")/env.sh"
H=$(dirname "$0")
RUN=${RUN:-$K/cycles}
mkdir -p "$RUN"
kl() { bash "$H/kl.sh" "$@"; }
pane() { python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }
EVERY=2 bash "$H/sample.sh" "$RUN/hub.csv" &
S=$!
kl 1 up >/dev/null
sleep 20
for c in $(seq "${1:-6}"); do
  echo "open$c $(date +%s)" >> "$RUN/phases.txt"
  kl 1 "KL.open('list','/',4); KL.open('chat','/m/mac/p/$(pane s02/w1:p1)?view=chat',1); KL.open('chat','/m/mac/p/$(pane s03/w1:p1)?view=chat',1); KL.open('chat','/m/mac/p/$(pane s04/w1:p1)?view=chat',1); KL.open('term','/m/mac/p/$(pane s01/w1:p2)?view=terminal',3); return 1" >/dev/null
  sleep 30
  echo "closed$c $(date +%s)" >> "$RUN/phases.txt"
  kl 1 'return KL.close()' >/dev/null
  sleep 45
done
echo "end $(date +%s)" >> "$RUN/phases.txt"
kill $S
