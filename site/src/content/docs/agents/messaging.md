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
klark msg send --wait 300 work/coder "When you're done, run the tests."   # wait up to 300 s for idle, then send
klark msg send --all "Rebased on master; pull before pushing."            # every other agent in this session
klark msg send --all-sessions "Stand-up in 5."                            # every other agent on this machine
klark msg reply "3 failing: a, b, c"    # answer whoever messaged this pane last
klark msg log -n 50                     # recent sent and received messages
klark msg status                        # hub, token expiry, running sessions
```

More ways to send:

```sh
klark msg send --wait 300 reviewer "when you're done, run the tests"   # waits up to 300 s for it to be idle, then sends
klark msg send --all "rebased on master, pull before pushing"          # every other agent in this session
klark msg send --all-sessions "stand-up in 5"                          # every other agent on this machine
```

`--wait <secs>` sends only once the agent is idle: not working and not showing a dialog.
If it is still busy after that long, the send fails.

Options go before the address. Everything after the address is the message,
word for word: `klark msg send --json work/coder <text...>`.

The receiving agent sees:

```text
[klark msg from laptop/default/reviewer] Run the tests in crates/x; reply with failing names.
(reply: klark msg reply "<text>")
```

`whoami`, `name` and `reply` only work inside a klark pane.

## Delivery rules

- By default, a message is typed into the target pane as soon as it is sent,
  even if the agent is mid-turn.
- `--wait <secs>` holds the message until the target is idle: not `working`,
  and not showing a dialog. If it is still busy after `<secs>`, the send fails
  and nothing is typed. `--wait` applies to agents on this machine only; remote
  targets are sent without waiting.
- `--all` sends to every other agent in your session. `--all-sessions` sends to
  every other agent on the machine. Both skip your own pane.
- **Your typing is safe.** If a person has a half-typed prompt in the target pane, klark lifts that
  draft, sends the message on its own, then puts the draft back, unsent.
- Long messages get a longer pause before Enter, and a prompt that did not submit is entered again.
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
klark msg login --hub http://hub.lan:7470 --user alice    # asks for the password
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

One ask per message. Say what you need back. Do not message a `blocked` agent.
If the target is `working` and the message can wait, use `--wait` so it lands
between turns instead of in the middle of one. Skip "thanks" and "ok".
