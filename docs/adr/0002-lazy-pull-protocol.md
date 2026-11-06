# ADR 0002: link protocol v1, always pull

- Status: accepted, implemented (`crates/klark-link/src/protocol.rs`, `hub/Services/MachineRegistry.cs`)
- Supersedes: the "tree on connect and on change" part of ADR 0001 D4

## Context
With 100+ machines and only a few people looking at any moment, anything a
machine sends unasked is wasted. We want the hub to pull what the UI needs,
when the UI needs it.

## Decision
The connection is one WebSocket, which the machine dials:
`ws://hub:8080/agent`, with the headers `Authorization: Bearer <token>` and,
optionally, `X-Klark-Machine: <id>`.

Hub → machine (JSON text):

| message | when the hub sends it | machine reply |
|---|---|---|
| `get_tree {known_rev?}` | machines page open (every 3 s, only for trees older than 2 s), or a pane page opened | `tree {rev, snapshot}`, or `tree_unchanged {rev}` if `known_rev` matches |
| `subscribe {pane_id}` | the first viewer opens a pane | binary output frames until unsubscribe |
| `unsubscribe {pane_id}` | the last viewer leaves | stops streaming (kills the observer) |
| `call {id, method, params}` | screen read when a pane opens; later transcript reads | `result {id, result \| error}` |
| `input {pane_id, text}` / `keys {pane_id, keys}` | someone replies | none |
| `ping` | keepalive / diagnostics | `pong` |

Machine → hub, unasked: only `hello {protocol, machine_id, machine_name, klark_version}`, once.

`call` is limited to an allow-list: `ping`, `session.snapshot`, `pane.read`,
`agent.read`, `pane.get`, `workspace.list`, `tab.list`, `pane.list`. Anything
else gets an error reply. The hub cannot close panes or run commands.

Binary encoding, machine → hub:
- `0x1f…`: a gzip JSON message.
- `0x01 len id gzip(output)` and `0x02 len id output`: terminal output.
  Output is merged per pane per 25 ms tick.

## Consequences
- An idle machine costs one socket. An idle hub with no viewers sends nothing
  but WebSocket keepalives.
- The pane list a viewer sees is at most ~3 s old. That is fine for status;
  live output is still instant.
- The protocol version is in `hello`. Breaking changes bump `PROTOCOL_VERSION`,
  and the hub must refuse versions it doesn't understand (not done yet: S-10 in
  STATUS).
