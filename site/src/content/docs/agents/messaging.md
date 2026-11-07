---
title: Messaging between agents
description: klark msg sends a message from one agent to another, in any session or on any machine.
---

`klark msg` lets an agent (or you) send text to another agent. The receiver
gets it as a prompt, tagged with the sender's address, and can reply.

## Addresses

Every agent has an address `<machine>/<session>/<agent>`, for example
`laptop/work/reviewer`.

- `agent`: the agent's name, or its pane id (`w1:p1`) if it has none.
- `session`: the klark session name. The default session is `default`.
- `machine`: the name the hub knows the machine by: `--name` on
  `klark expose`, else `KLARK_MACHINE_NAME`, else the host name.

On this machine, `work/reviewer` and `reviewer` also work. A bare name is
searched in every running session and fails if it is ambiguous.

## Commands

```sh
klark msg whoami                        # my address, e.g. laptop/work/reviewer
klark msg name reviewer                 # give this pane's agent a name
klark msg ls                            # agents in every session (and on the hub, if logged in)
klark msg send work/coder "Run the tests in crates/x; reply with failing names."
klark msg reply "3 failing: a, b, c"    # answer whoever messaged this pane last
klark msg log -n 50                     # recent sent and received messages
klark msg status                        # hub, token expiry, running sessions
```

Options go before the address. Everything after the address is the message,
word for word: `klark msg send --json work/coder <text...>`.

The receiving agent sees:

```text
[klark msg from laptop/default/reviewer] Run the tests in crates/x; reply with failing names.
(reply: klark msg reply "<text>")
```

`whoami`, `name` and `reply` only work inside a klark pane.

## Delivery rules

- Delivery fails, and the sender is told, if the target is `blocked` or does
  not exist.
- Every send and receipt is appended to `messages.jsonl` in klark's data
  directory. `klark msg log` reads it.
- In `klark msg ls`, the status `-` means klark cannot classify that agent's
  screen.

## Same machine: no hub

Within one machine, `klark msg` talks to each session's socket directly. No
network is involved.

## Across machines: through the hub

The sender's CLI posts to the hub's `POST /api/messages`. The hub hands the
message to the target machine over that machine's existing link, and the target
delivers it locally.

The CLI needs a hub token. It uses, in order:

1. `KLARK_HUB_URL` with `KLARK_HUB_JWT` or `KLARK_HUB_TOKEN`.
2. The default hub saved by `klark hub connect` (its URL and machine token).
3. A user token from `klark msg login`:

```sh
klark msg login --hub http://hub.lan:8080 --user alice    # asks for the password
klark msg logout
```

The token is saved in `hub.json` in klark's config directory (mode 0600). A
`msg login` token is used only for the hub that issued it.

## Errors you may see

| message | what to do |
|---|---|
| `no agent X in any running session on this machine` | Check the name with `klark msg ls`, `klark session list`. |
| `X is ambiguous; use one of: work/w1:p1, ops/w1:p1` | Use the `session/agent` form. |
| `no hub configured` | Run `klark hub connect` or `klark msg login`, or set `KLARK_HUB_URL`. |
| `m/s/a: no such agent (agent_not_found)` | The hub reached the machine, but no agent has that name. |
| `m/s/a: hub refused: …` | The machine is offline, hidden by `--agents`, or you may not type into it. |
| `nobody has messaged this pane yet` | Use `send`, not `reply`. |

## Etiquette for agents

One ask per message. Say what you need back. Do not message a `blocked` agent,
and do not resend while it is `working`. Skip "thanks" and "ok": every message
interrupts a turn.
