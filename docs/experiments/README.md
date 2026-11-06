# Experiments

Each experiment: question, setup, numbers, conclusion, follow-up. Raw scripts
are in [`/experiments`](../../experiments): `mockhub.py` is a Python stand-in
hub that behaves like the lazy hub, and `drive.py` runs the matrix.

E-01 to E-04 were measured against the stand-in hub. It shows what the machines
and the wire do. E-06 and E-10 are measured against the real hub image, with
10 real klark sessions running Claude Code and simulated machines for the
100-machine curve ([`experiments/hubload/`](../../experiments/hubload/)).

| id | question | status |
|---|---|---|
| [E-01](E-01-lazy-pull-100-machines.md) | Does lazy pull hold with 100 machines? | done |
| [E-02](E-02-binary-frames-vs-base64.md) | Does dropping base64 for binary frames save bytes? | done |
| [E-03](E-03-coalesce-tick.md) | Which output merge interval: 25, 50 or 100 ms? | done → 50 ms adopted |
| [E-04](E-04-heavy-output-all-viewed.md) | Worst case: every pane viewed, heavy output | done; real-hub per-pane cost added from E-06 |
| [E-05](E-05-adaptive-tick.md) | Adaptive tick (fast when quiet, slow when flooding) | to do |
| [E-06](E-06-real-hub-resources.md) | Real hub: CPU, RAM, sockets, wire, latency; 10 real + 100 simulated machines, 10 viewers, 10 API clients; sizing table | done: lazy holds; 512m/1 cpu fits 101 machines; 5 bugs, 5 bottlenecks |
| [E-07](E-07-zstd-dictionary.md) | zstd with a terminal dictionary instead of gzip | to do |
| [E-08](E-08-sqlite-write-load.md) | SQLite writer under 100 streaming panes; how fast the cap triggers | to do |
| [E-09](E-09-browser-render.md) | xterm.js in a phone browser at 20 KB/s | to do |
| [E-10](E-10-reconnect-storm.md) | Hub restart with 100 machines reconnecting | done: all back in 1.6 s |

## How to rerun
```bash
pip install websockets
cargo build --release -p klark-link --bins
python3 experiments/drive.py
```

Real hub (E-06, E-10); the order matters, see each script's header:
```bash
cargo build --release && docker build -t kload-hub hub   # see E-06 bug 1 for the Dockerfile flag
bash experiments/hubload/hub-up.sh                        # :5100, MockLdap, tokens kload-fleet / kload-mac
bash experiments/hubload/sample.sh $K/run1/hub.csv &      # K defaults to /private/tmp/claude-501/kload
bash experiments/hubload/sessions-up.sh 10                # 10 klark sessions with Claude Code on Haiku
bash experiments/hubload/k.sh expose --hub ws://127.0.0.1:5100/agent --token kload-mac &
bash experiments/hubload/kl.sh 1 up; bash experiments/hubload/kl.sh 2 up   # headless chrome-agent viewers
bash experiments/hubload/phases.sh                        # phases 2-5, latency, API clients
bash experiments/hubload/curve.sh; bash experiments/hubload/curve2.sh   # 100 simulated machines
bash experiments/hubload/run512.sh; bash experiments/hubload/e10.sh; bash experiments/hubload/cycles.sh
python3 experiments/hubload/summarize.py $K/run1
bash experiments/hubload/down.sh                          # stops everything, removes container + volume
```
