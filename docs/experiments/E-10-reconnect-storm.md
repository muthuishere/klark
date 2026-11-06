# E-10: Reconnect storm

- Date: 2026-11-06. Status: **done**. Data: [`data/e06/e10/`](data/e06/e10/),
  script `experiments/hubload/e10.sh`.

**Question:** What happens when the hub restarts with 100 machines linked?

**How:** The real hub image on `--memory 512m --cpus 1`. 100 simulated
machines (`klark-loadtest --reconnect`: the real link's 1 s → 30 s backoff,
±50 % jitter) plus the real `klark expose` of 10 live sessions. After 20 s,
`docker restart -t 5 kload-hub`; connected machines sampled every 250 ms.

| t (s, load-test clock) | connected | total connects |
|---|---|---|
| 24.9 | 100 | 100 |
| 25.1 (hub process gone) | 0 | 100 |
| 25.9 | 25 | 125 |
| 26.1 | 52 | 152 |
| 26.4 | 76 | 176 |
| **26.7** | **100** | 200 |

- **All 100 back 1.6 s after the drop**, 0.8 s from the first reconnect to the
  last; exactly one reconnect per machine (200 connects, 100 failures = the
  100 dropped links). The jitter spreads the herd over ~0.8 s; no spike.
- The real `klark expose` was back 0.7 s after its connection reset.
- No auth, SQLite or protocol errors after the restart.
- `docker restart` took 5.3 s and `/healthz` answered 5.7 s after it began.

**Finding:** the hub did not exit on SIGTERM while machines were linked; Docker
killed it after the 5 s timeout (the drop is at +5 s). The machine WebSockets
run on `ctx.RequestAborted` (`hub/Program.cs:139`), which shutdown does not
cancel, so Kestrel waits for them. Fix: link that token with
`IHostApplicationLifetime.ApplicationStopping` and close each socket with
`EndpointUnavailable`, so restarts are graceful and machines reconnect at
once instead of after a forced kill.

**Pass / decision rule:** all back within 60 s with no auth or SQLite errors —
**passed** (1.6 s). Jitter (S-14) is enough; nothing else is needed.
