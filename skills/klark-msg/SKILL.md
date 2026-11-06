---
name: klark-msg
description: "Talk to other coding agents running in klark — in this session, in any other klark session on this machine, or on another machine linked to the same hub. Find agents, know your own address, send a message, reply, read another agent's screen, wait for it to finish, offer a file to the person. Trigger: ask the other agent, message the reviewer, tell the agent in session work, which agents are running, what is the coder doing, wait for the reviewer, reply to that agent, send this file to my phone."
---

# klark-msg

You run in a klark pane. Other agents run in other panes, other klark
sessions, other machines. Every agent has an **address**:

```
<machine>/<session>/<agent>       laptop/work/reviewer
```

`agent` is its name, or its pane id (`w1:p1`) if it has none. On this machine
the short forms work too: `work/reviewer` (session `work`) or `reviewer`.

## I want to … → command

| I want to … | command |
|---|---|
| see who I am | `klark msg whoami` → `laptop/work/reviewer` (`--json` for fields) |
| give myself a name | `klark msg name reviewer` (lowercase, digits, `-`, `_`) |
| list every agent I can reach | `klark msg ls` (table; `--local` skips the hub; `--json`) |
| list agents of all sessions here, full detail | `klark agent list -a` (JSON, each with `session` and `address`) |
| list sessions and how many agents each has | `klark session list` |
| send a message | `klark msg send work/coder "Run the tests in crates/x; reply with failing names."` |
| send to another machine | `klark msg send buildbox/default/ci "Is main green? reply yes/no"` |
| answer a message I got | `klark msg reply "3 failures, all in upstream::tests"` |
| see recent messages | `klark msg log` (`-n 50`, `--json`) |
| read another agent's screen | `klark agent read work/coder --lines 40` |
| wait until it is done | `klark agent wait work/coder --timeout 600000` (idle, done or blocked) |
| check its state | `klark agent get work/coder` → `result.agent.agent_status` |
| give a person a file | `klark offer <path>` (see below) |
| reach other machines | `klark msg login --hub http://hub.lan:8080 --user alice` (a person types the password) |
| check hub / token / sessions | `klark msg status` |

Every `klark agent` TARGET (`get read prompt send-keys wait focus rename`)
accepts `session/agent` and `machine/session/agent` for this machine. A bare
name is looked up in your own session first; if it is not there, in the other
running sessions. `klark agent attach` stays in one session.

Options go before the address: `klark msg send --json work/coder <text...>`.
Everything after the address is the message, verbatim, `--flags` included.
In `klark msg ls`, STATUS `-` means klark cannot classify that agent's screen
(`--json` shows the raw `unknown`).

`msg send` vs `agent prompt`: `msg send` tags the text with your address
(`[klark msg from laptop/default/you] …` plus a reply hint), logs it, and
crosses machines. `agent prompt` types raw text into a local agent. To ask
another agent something, use `msg send`.

## What the other agent sees

```
[klark msg from laptop/default/reviewer] Run the tests in crates/x; reply with failing names.
(reply: klark msg reply "<text>")
```

When you receive one, answer with `klark msg reply "<text>"`; it goes to
whoever last messaged your pane.

## Ask, then wait for the answer

```bash
klark msg send work/coder "Fix the failing test in src/a.rs; reply 'done' or the blocker."
klark agent wait work/coder --timeout 900000   # returns when it is idle/done/blocked
klark agent read work/coder --lines 30          # if no reply arrived, look at its screen
```

## Offer a file to a person (ADR 0011)

```bash
klark offer ./report.pdf            # appears on this pane's hub page, as a download/image
klark offer out.png --pane w1:p2    # another pane; defaults to KLARK_PANE_ID
klark offer .env --force            # credential-looking paths are refused without --force
```

Needs `klark expose` running with a hub (`KLARK_HUB_URL`, `KLARK_HUB_TOKEN`).
Max 100 MB. If it prints `unknown command: offer`, this klark build predates it.

## Etiquette

- One ask per message. Say what you need back and in what form ("reply yes/no",
  "reply with the failing test names").
- Include what the other agent cannot see: paths, branch, PR number.
- Reply with `klark msg reply`, never by typing into its pane.
- Check `klark msg ls` first. Don't message a `blocked` agent: it waits on a
  person. Don't resend while it is `working`; `klark agent wait` instead.
- No "thanks"/"ok" messages: every message interrupts a turn.

## When it fails

| it prints | fix |
|---|---|
| `no agent X in any running session on this machine (see: klark msg ls)` | Wrong name, or its session is stopped. `klark msg ls`, `klark session list`. |
| `X is ambiguous; use one of: work/w1:p1, ops/w1:p1` | Use the `session/agent` form from the list. |
| `agent target X not found in session S, nor in any other running session (see: klark agent list -a)` | Same, from `klark agent`. |
| `agent target X is not in this session and is ambiguous across sessions; use one of: …` | Use a full address from the list. |
| `session S is not running (see: klark session list)` | Typo, or the session is stopped. |
| `machine M is not this machine (…); … klark msg send M/S/A "<text>"` | `klark agent` is local only; message it with `klark msg send`. |
| `no hub configured: run \`klark msg login --hub <url> --user <name>\` or set KLARK_HUB_URL` | Cross-machine needs a hub; ask the owner to log in. |
| `m/s/a: no such agent (agent_not_found)` | The hub reached that machine; no such agent there. `klark msg ls` for real addresses. |
| `m/s/a: hub refused: …` | Machine offline/not linked, or no permission to type into it. |
| `nobody has messaged this pane yet; use klark msg send <to> <text>` | Nothing to reply to; use `send`. |
| `… a shell outside any klark pane, so there is no agent to reply to` | The sender was a person's shell; there is no one to answer. |
| `reply works inside a klark pane (KLARK_PANE_ID is not set)` | `reply`/`name` only work from inside a pane. |

## Rules

- Never print, paste or log a token (`KLARK_HUB_TOKEN`, `KLARK_HUB_JWT`, `hub.json`).
  Never ask for a password in a message or put one on a command line.
- The hub is plain HTTP: no secrets in messages or offered files.
