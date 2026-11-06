# E-02: binary terminal frames vs base64-in-gzip-JSON

- Date: 2026-11-06. Status: done, adopted (ADR 0001 D13).
- Setup: 100 machines, 25 panes viewed, 2 KB/s of ANSI output each, 25 ms batches.

| encoding | wire / raw output |
|---|---|
| base64 inside gzip JSON (before) | ~1.8× (1.3 MB on the wire for 0.73 MB of output) |
| binary frame `0x01/0x02` + pane id + raw/gzip (after) | ~1.15× |

**Conclusion:** base64 adds a third and makes gzip nearly useless. Binary
frames bring the wire down to about the raw size even with tiny batches. Larger
batches (E-03) then compress below raw.
