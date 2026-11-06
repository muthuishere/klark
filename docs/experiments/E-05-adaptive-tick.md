# E-05: Adaptive merge interval

- Status: **to do**

**Question:** Use 25 ms while output is under ~1 KB per tick (typing, prompts), then stretch to 100 ms while flooding (builds, logs).

**How:** Run the E-03 matrix with mixed traffic: 10% of panes flooding at 50 KB/s and the rest interactive. Measure wire bytes and the 95th-percentile delay from output to frame.

**Pass / decision rule:** Latency for interactive panes ≤ 25 ms, and flood panes at E-03's 100 ms ratio.
