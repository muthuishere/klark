"""Sum the token usage of the load test's Claude Code sessions from their JSONL logs.

  python3 cost.py <claude projects dir> <substring of the work-dir project names>

Prices are Haiku 4.5 list prices per million tokens (input 1, output 5,
cache write 1.25, cache read 0.10); an estimate, not a bill.
"""
import glob, json, os, sys

root, needle = sys.argv[1], sys.argv[2]
tot = {"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
seen = set()
models = set()
for d in glob.glob(os.path.join(root, "*" + needle + "*")):
    for f in glob.glob(os.path.join(d, "*.jsonl")):
        for line in open(f):
            try:
                o = json.loads(line)
            except ValueError:
                continue
            m = o.get("message") or {}
            u = m.get("usage")
            if not u or m.get("id") in seen:
                continue
            seen.add(m.get("id"))
            models.add(m.get("model"))
            for k in tot:
                tot[k] += u.get(k) or 0
usd = (tot["input_tokens"] * 1 + tot["output_tokens"] * 5 + tot["cache_creation_input_tokens"] * 1.25
       + tot["cache_read_input_tokens"] * 0.10) / 1e6
print(json.dumps({"messages": len(seen), "models": sorted(x for x in models if x), **tot, "usd_estimate": round(usd, 3)}))
