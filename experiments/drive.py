import json, os, resource, subprocess, sys, time
S = os.path.dirname(os.path.abspath(__file__))
def run(every, tick, rate, machines=100, secs=10):
    env = dict(os.environ, EVERY=str(every))
    hub = subprocess.Popen([sys.executable, f"{S}/mockhub.py", str(secs + 3)], env=env, stdout=subprocess.PIPE, text=True)
    time.sleep(1)
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    subprocess.run([os.path.join(os.path.dirname(S), "target/release/klark-loadtest"), "--hub", "ws://127.0.0.1:18080/agent", "--token", "fleet",
                    "--machines", str(machines), "--seconds", str(secs), "--tick-ms", str(tick), "--rate", str(rate)],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    out, _ = hub.communicate()
    st = json.loads(out.strip().splitlines()[-1])
    cpu = (after.ru_utime + after.ru_stime) - (before.ru_utime + before.ru_stime)
    viewed = machines // every
    print(json.dumps(dict(viewers_every=every, viewed=viewed, tick_ms=tick, rate=rate, conn=st["conn"],
        full_trees=st["tree"], unchanged=st["unchanged"], frames=st["frames"],
        raw_kb_s=round(st["frame_bytes"]/1024/secs,1), wire_kb_s=round(st["wire_bytes"]/1024/secs,1),
        ratio=round(st["wire_bytes"]/max(1,st["frame_bytes"]),2), sim_cpu_pct=round(100*cpu/secs,1),
        sim_maxrss_mb=round(after.ru_maxrss/1024,1))))
    sys.stdout.flush()
for args in [(4,25,2000),(4,50,2000),(4,100,2000),(1,50,2000),(1,50,20000),(1,100,20000)]:
    run(*args)
