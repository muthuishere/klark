# ADR 0010: the command is `klark expose`

- Status: accepted (owner decision, 2026-11-06). Renames the `klark link`
  command from ADRs 0001, 0002, 0005, 0008 and 0009; those keep their wording
  as history.

## Decision
The command that puts this machine's sessions, tabs and panes on a hub is
`klark expose`, the same name as the herdr-expose idea it follows:

- `klark expose --hub ws://<hub>:8080/agent --token <token> [--name <machine>]`
- `klark expose url`: the hub link for the current pane.

The internal crate keeps its name, `crates/klark-link`. The wire protocol
does not change.

## What expose does and does not send (restates ADR 0002)
- It dials one WebSocket out to the hub. Unasked, it sends only `hello`.
- It never pushes pane data. Output for a pane flows only between the hub's
  `subscribe` and `unsubscribe`, which means only while at least one signed-in
  user has that pane's page open. The tree (workspaces, tabs, panes) is sent
  only when the hub asks with `get_tree`, which happens only while someone has
  the machines page open.
- Messages (ADR 0009) are also hub → machine, sent only when someone sends
  one.
