# ADR 0003: hub storage on SQLite, 10 MB per machine

- Status: accepted, implemented (`hub/Services/HubStore.cs`); not yet run (needs a build)

## Context
The hub needs a little persistence: which machines exist, their last known
panes (to show offline machines), and recent output. It must not need a
database server, and one noisy machine must not starve the others.

## Decision
- `DataDir` (`/data` in Docker) holds:
  - `hub.db`: `machines(id, host, version, first_seen, last_seen, connected)`;
  - `machines/<id>.db`, one file per machine:
    - `tree(id=1, ts, json)`: the latest pulled tree;
    - `pane_output(ts, pane_id, bytes)`: output captured while someone watched;
    - `events(ts, kind)`: connect and disconnect.
- WAL mode, `synchronous=NORMAL`, `auto_vacuum=INCREMENTAL`.
- One background writer drains a bounded channel (50k items, drop-oldest) and
  writes one transaction per machine per ~100 ms batch.
- **Cap:** after writing output, if `page_count × page_size` is over
  `MaxBytesPerMachine` (10 MB), the writer deletes the oldest 25% of
  `pane_output`, trims `events` to 1000 rows, checkpoints and runs an
  incremental vacuum. It repeats up to 8 rounds.

## Consequences
- 100 machines take at most ~1 GB. Removing a machine means deleting its file.
- Output is stored only while someone watches, because that is the only time it
  flows (ADR 0002). The store is a "recently seen" log, not a full recording.
- The WAL file can briefly exceed the cap between checkpoints.
- Nothing reads `pane_output` yet. Showing history for offline machines is
  S-07 in STATUS.
