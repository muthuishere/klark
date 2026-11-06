"""Concurrent JWT API clients against the hub (E-06).

  python3 api.py --clients 10 --seconds 30 [--hub http://127.0.0.1:5100] [--send-to load:sim-000/default/claude-0]

Each client signs in once (POST /api/token), then loops GET /api/agents and
GET /api/messages?limit=50 (and POST /api/messages when --send-to is given,
which should name a simulated agent, never a real one) with --think-ms
between requests. Prints latency percentiles per endpoint as JSON.
Stdlib only.
"""
import argparse, json, threading, time, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("--hub", default="http://127.0.0.1:5100")
ap.add_argument("--clients", type=int, default=10)
ap.add_argument("--seconds", type=float, default=30)
ap.add_argument("--think-ms", type=float, default=500)
ap.add_argument("--send-to", default="")
a = ap.parse_args()

def req(method, path, token=None, body=None):
    data = json.dumps(body).encode() if body is not None else None
    r = urllib.request.Request(a.hub + path, data=data, method=method)
    r.add_header("content-type", "application/json")
    if token:
        r.add_header("authorization", "Bearer " + token)
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(r, timeout=30) as resp:
            out = resp.read()
            code = resp.status
    except urllib.error.HTTPError as e:
        out, code = e.read(), e.code
    return code, (time.perf_counter() - t0) * 1000, out

lat = {}
errors = {}
nbytes = {}
lock = threading.Lock()

def client(i):
    users = ["alice", "bob", "admin"]
    code, _, out = req("POST", "/api/token", body={"username": users[i % 3], "password": users[i % 3]})
    if code != 200:
        with lock:
            errors["token"] = errors.get("token", 0) + 1
        return
    tok = json.loads(out)["token"]
    end = time.time() + a.seconds
    k = 0
    while time.time() < end:
        calls = [("GET", "/api/agents", None), ("GET", "/api/messages?limit=50", None)]
        if a.send_to:
            calls.append(("POST", "/api/messages", {"to": a.send_to, "text": f"load {i}/{k}"}))
        for m, p, b in calls:
            code, ms, out = req(m, p, tok, b)
            key = f"{m} {p.split('?')[0]}"
            with lock:
                lat.setdefault(key, []).append(ms)
                nbytes[key] = nbytes.get(key, 0) + len(out)
                if code >= 300:
                    errors[f"{key} {code}"] = errors.get(f"{key} {code}", 0) + 1
            time.sleep(a.think_ms / 1000)
        k += 1

ts = [threading.Thread(target=client, args=(i,)) for i in range(a.clients)]
t0 = time.time()
for t in ts: t.start()
for t in ts: t.join()
el = time.time() - t0

def pct(xs, p):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(p / 100 * len(xs)))], 1)

print(json.dumps({"clients": a.clients, "seconds": round(el, 1), "errors": errors, "endpoints": {
    k: {"n": len(v), "rps": round(len(v) / el, 1), "p50_ms": pct(v, 50), "p95_ms": pct(v, 95), "max_ms": round(max(v), 1),
        "avg_bytes": nbytes[k] // len(v)} for k, v in lat.items()}}, indent=1))
