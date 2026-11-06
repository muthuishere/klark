# ADR 0006: designing for 100+ machines

- Status: accepted; measured against a stand-in hub only (see experiments/E-01, E-02)

## Budget per machine
| state | cost |
|---|---|
| linked, nobody signed in | 1 socket and a keepalive every 20 s; nothing else |
| machines page open | one `get_tree` every 3 s; the reply is ~40 bytes when unchanged |
| a pane open | the pane's output, merged per 25 ms, binary, gzip when it pays off |

## Design choices that keep it cheap
- Pull, not push (ADR 0002).
- One receive loop per machine, all async. Sends are serialized per machine
  with a semaphore.
- Viewer ref-counting per pane: N people watching one pane need one stream from
  the machine.
- Pulls are shared: N people on the machines page cause one pull per machine
  per 2 s.
- The machines page repaints at most 4×/s, however many machines change.
- The SQLite writer is off the hot path (ADR 0003).

## Known limits / to measure (see STATUS)
- Blazor Server keeps a circuit per browser tab (~hundreds of KB). 50+
  simultaneous viewers need measuring (S-12).
- Fan-out to viewers is sequential per frame. With many viewers on one hot
  pane, a slow viewer could delay the others. A per-viewer bounded queue is
  needed (S-13).
- The machine side merges output in 25 ms ticks. Tiny ticks leave little for
  gzip to work with (E-02). An adaptive tick, 25 ms when typing and up to
  100 ms when flooding, is E-04.
