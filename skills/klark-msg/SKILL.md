---
name: klark-msg
description: "Message other agents running in klark: in this session, in another klark session on this machine, or on another machine linked to the same hub. Find them, know your own address, send, reply, log in to a hub. Trigger: ask the other agent, message the reviewer, tell the agent in session work, which agents are running, reply to that agent."
---

# klark-msg

You are running inside a klark pane. Other agents run in other panes, other
klark sessions, or on other machines. `klark msg` lets you send one of them a
message; it arrives as a prompt in that agent's pane, and it can answer you
the same way.

## Know yourself and find others

```bash
klark msg whoami        # your address, e.g. laptop/default/reviewer
klark msg ls            # every agent you can reach: ADDRESS STATUS AGENT WHERE
klark msg ls --local    # only this machine (no hub)
```

An address is `<machine>/<session>/<agent>`. Short forms work on this machine:
`reviewer` (searched in every running session; fails if two match) or
`work/reviewer` (the agent `reviewer` in session `work`). `agent` is the
agent's name, or its pane id if it has no name.

Give yourself a name so others can address you:

```bash
klark msg name reviewer
```

## Send and reply

```bash
klark msg send work/coder "Please run the tests in crates/klark-link and tell me which fail."
klark msg send buildbox/default/ci "Is main green? Reply with the last run's result."
```

The other agent sees:

```
[klark msg from laptop/default/reviewer] Please run the tests ...
(reply: klark msg reply "<text>")
```

When you receive a message like that, answer with:

```bash
klark msg reply "3 failures, all in upstream::tests; logs in /tmp/t.log"
```

`reply` goes to whoever last messaged your pane. `klark msg log` shows recent
sent and received messages.

## Another machine: the hub

Messages to another machine go through the klark hub. The machine needs a hub
URL and a token: either `klark link` is set up (`KLARK_HUB_URL` and
`KLARK_HUB_TOKEN`), or a person logs in once:

```bash
klark msg login --hub http://hub.lan:8080 --user alice    # asks for the password
klark msg status                                          # hub, token expiry, sessions
klark msg logout
```

Only a person types a password. Never ask for one in a message, and never put
one on the command line.

## Etiquette

- One message per request. Say exactly what you need back and in what form
  ("reply with the failing test names", "reply yes or no").
- Include the context the other agent cannot see: paths, branch, PR number.
- Answer with `klark msg reply`, not by typing into its pane.
- Don't message an agent whose status is `blocked`; it is waiting on a person,
  and delivery will fail.
- Don't loop: if you got an answer, don't send "thanks".

## When it fails

| message | what it means |
|---|---|
| `no agent X in any running session` | Wrong name, or its session isn't running. Run `klark msg ls`. |
| `X is ambiguous; use one of: ...` | Two agents share the name. Use `session/agent` from the list. |
| `no hub configured` / `no hub token` | Cross-machine needs a hub. Ask the owner to run `klark msg login` or set `KLARK_HUB_URL`. |
| `hub refused: ...` | The hub said no: the machine is offline, the agent doesn't exist there, or your user may not type into that machine. |
| `nobody has messaged this pane yet` | `reply` has no one to answer; use `send`. |

## Rules

- Never print, paste or log a token (`KLARK_HUB_TOKEN`, `KLARK_HUB_JWT`, or `hub.json`).
- Message text crosses the network unencrypted (plain HTTP hub): no secrets in messages.
