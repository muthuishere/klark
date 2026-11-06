# E-08: SQLite writer load and the cap

- Status: **to do**

**Question:** Can the single writer keep up with 100 streaming panes, and how often does the 10 MB cap prune?

**How:** Hub with E-04 traffic (20 KB/s × 100). Measure channel depth, batch time and cap rounds per minute. Check that files stay at or under 10 MB plus WAL.

**Pass / decision rule:** Batch time < 50 ms, no channel drops. If not: store output only for the last N KB per pane, or gzip blobs.
