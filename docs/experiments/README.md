# Experiments

Each experiment: question, setup, numbers, conclusion, follow-up. Raw scripts
are in [`/experiments`](../../experiments): `mockhub.py` is a Python stand-in
hub that behaves like the lazy hub, and `drive.py` runs the matrix.

**Every number below is measured against the stand-in hub, not the Blazor hub,
which has not been built yet.** It shows what the machines and the wire do. Hub
CPU and memory are E-06.

| id | question | status |
|---|---|---|
| [E-01](E-01-lazy-pull-100-machines.md) | Does lazy pull hold with 100 machines? | done |
| [E-02](E-02-binary-frames-vs-base64.md) | Does dropping base64 for binary frames save bytes? | done |
| [E-03](E-03-coalesce-tick.md) | Which output merge interval: 25, 50 or 100 ms? | done → 50 ms adopted |
| [E-04](E-04-heavy-output-all-viewed.md) | Worst case: every pane viewed, heavy output | done |
| [E-05](E-05-adaptive-tick.md) | Adaptive tick (fast when quiet, slow when flooding) | to do |
| [E-06](E-06-real-hub-resources.md) | Blazor hub CPU and RAM with 100 machines and 20 viewers | to do (needs build) |
| [E-07](E-07-zstd-dictionary.md) | zstd with a terminal dictionary instead of gzip | to do |
| [E-08](E-08-sqlite-write-load.md) | SQLite writer under 100 streaming panes; how fast the cap triggers | to do |
| [E-09](E-09-browser-render.md) | xterm.js in a phone browser at 20 KB/s | to do |
| [E-10](E-10-reconnect-storm.md) | Hub restart with 100 machines reconnecting | to do |

## How to rerun
```bash
pip install websockets
cargo build --release -p klark-link --bins
python3 experiments/drive.py
```
