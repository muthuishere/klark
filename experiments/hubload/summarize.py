"""Summarize a run: per phase, hub memory / CPU / RSS / threads / sockets / disk and wire rates.

  python3 summarize.py $K/run1      (reads hub.csv and phases.txt there)

rx = bytes into the hub container (machines' output + browsers' input),
tx = bytes out of it (to browsers and machines). Markdown table on stdout.
"""
import csv, sys, os

run = sys.argv[1]
rows = list(csv.DictReader(open(os.path.join(run, "hub.csv"))))
rows = [r for r in rows if r.get("rss_kib")]
phases = [l.split() for l in open(os.path.join(run, "phases.txt")) if l.strip()]
phases = [(n, int(t)) for n, t in phases]
print("| phase | s | docker mem MiB (mean/max) | CPU % (mean/max) | RSS MiB (max) | threads (max) | TCP est (max) | rx KB/s | tx KB/s | SQLite KB (end) |")
print("|---|---|---|---|---|---|---|---|---|---|")
for i, (name, start) in enumerate(phases):
    end = phases[i + 1][1] if i + 1 < len(phases) else int(rows[-1]["epoch"])
    sel = [r for r in rows if start <= int(r["epoch"]) < end]
    if len(sel) < 2:
        continue
    f = lambda k: [float(r[k]) for r in sel]
    dt = int(sel[-1]["epoch"]) - int(sel[0]["epoch"]) or 1
    rx = (float(sel[-1]["rx_bytes"]) - float(sel[0]["rx_bytes"])) / dt / 1024
    tx = (float(sel[-1]["tx_bytes"]) - float(sel[0]["tx_bytes"])) / dt / 1024
    m, c = f("mem_mib"), f("cpu_pct")
    print(f"| {name} | {end - start} | {sum(m)/len(m):.0f} / {max(m):.0f} | {sum(c)/len(c):.1f} / {max(c):.1f} | "
          f"{max(f('rss_kib'))/1024:.0f} | {max(f('threads')):.0f} | {max(f('tcp_est')):.0f} | {rx:.1f} | {tx:.1f} | "
          f"{float(sel[-1]['db_bytes'])/1024:.0f} |")
