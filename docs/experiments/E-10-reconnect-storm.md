# E-10: Reconnect storm

- Status: **to do**

**Question:** What happens when the hub restarts with 100 machines linked?

**How:** Kill the hub during a loadtest and restart it. The machines back off 1→30 s; count connects per second and time until all 100 are back.

**Pass / decision rule:** All back within 60 s, with no auth or SQLite errors. If there are spikes, add jitter to the backoff (S-14).
