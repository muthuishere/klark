# Klark: agents across sessions and messaging

Part of the klark skill; the rules in `SKILL.md` (KLARK_ENV check, safety) apply.

## Agents across sessions and messaging

Every agent has an address `<machine>/<session>/<agent>` (`laptop/work/reviewer`). `agent` is its name, or its pane id (`w1:p1`) if it has none. On this machine the short forms `work/reviewer` and `reviewer` work too.

Every `klark agent` TARGET (`get read prompt send-keys wait focus rename`) accepts `session/agent` and `machine/session/agent` for this machine. A bare name is looked up in your own session first, then in the other running sessions. `klark agent attach` stays in one session. `klark session list` shows sessions.

`klark msg send` vs `klark agent prompt`: `msg send` tags the text with your address, logs it, offers a reply hint and crosses machines; `agent prompt` types raw text into a local agent. To ask another agent something, use `msg send`. The other agent sees:

```
[klark msg from laptop/default/reviewer] Run the tests in crates/x; reply with failing names.
(reply: klark msg reply --to laptop/default/reviewer "<text>")
```

Answer with the hint's command, `klark msg reply --to <sender> "<text>"`: it goes to that sender even if someone else messaged you since. Without `--to` it goes to whoever messaged your pane last. Options go before the address (`klark msg send --json work/coder <text...>`); everything after the address is the message, verbatim. In `klark msg ls`, STATUS `-` means klark cannot classify that agent's screen.

Ask, then wait:

```bash
klark msg send work/coder "Fix the failing test in src/a.rs; reply 'done' or the blocker."
klark agent wait work/coder --timeout 900000
klark agent read work/coder --lines 30
```

Etiquette: one ask per message, say what you need back and in what form, include paths/branch/PR the other agent cannot see. Check `klark msg ls` first: don't message a `blocked` agent (it waits on a person). If the target is `working` and the message can wait, use `--wait <secs>` so it lands between turns instead of mid-turn. Remote targets are not waited on. No "thanks"/"ok" messages.


## Commands

| I want to … | command |
|---|---|
| know / set my own address | `klark msg whoami` → `laptop/work/reviewer`; `klark msg name reviewer` |
| ask an agent and get the answer back when you are NOT in a klark pane (`KLARK_PANE_ID` unset: a plain shell, a script, a coding agent outside klark) | `klark msg send --wait-reply 600 work/coder "<question>"`; it prints the answer. Never invent a `--from` name: nothing can be typed into it, so the answer only reaches `klark msg log`. |
| message an agent only once it is idle | `klark msg send --wait 300 work/coder "<text>"` |
| message every agent in this session; on this machine | `klark msg send --all "<text>"`; `klark msg send --all-sessions "<text>"` |
| list every agent, with its worktree | `klark msg ls --local` (`--json`; without `--local` also the hub's machines) |
| recent messages | `klark msg log` (`-n 50`, `--json`) |
| agents on other machines | `klark msg login --hub <url> --user <name>` (a person types the password); `klark msg status` |

## How a message is delivered

`klark msg send` types the message into the target agent and presses Enter, and then:
- If the person was typing in that agent's input, their unsent draft is lifted out first, the message goes in alone, and the draft is typed back without Enter. If they are mid-edit (arrow keys, history), delivery waits up to a minute for them to finish.
- If the agent is still idle 2 s later (the Enter was swallowed after a long paste), Enter is pressed once more. Never when the agent is showing a dialog.
- With `--wait <secs>` it waits until the agent is idle before sending, so it never lands mid-turn or answers a permission dialog by accident.

## When it fails

| it prints | fix |
|---|---|
| `klark msg: no agent X in any running session on this machine (see: klark msg ls)` | Wrong name, or its session is stopped. `klark msg ls`, `klark session list`. |
| `klark msg: no agent X in session S on this machine (see: klark msg ls)` | Same, for a `session/agent` address. |
| `X is ambiguous; use one of: work/w1:p1, ops/w1:p1` | Use the `session/agent` form from the list. |
| `agent target X not found in session S, nor in any other running session (see: klark agent list -a)` | `klark agent list -a` for real names. |
| `agent target X is not in this session and is ambiguous across sessions; use one of: …` | Use a full address from the list. |
| `session S is not running (see: klark session list)` | Typo, or the session is stopped. |
| `M/S/A: machine M is not this machine (H); klark agent reaches sessions on this machine only. To message it via the hub: klark msg send M/S/A "<text>"` | `klark agent` is local only; use `klark msg send`. |
| ``klark msg: no hub configured: run `klark msg login --hub <url> --user <name>` or set KLARK_HUB_URL`` | Cross-machine needs a hub; ask the owner to log in. |
| `m/s/a: no such agent (agent_not_found)` | The hub reached that machine; no such agent there. |
| `m/s/a: hub refused: …` | Machine offline/not linked, not exposed by its `--agents`, or no permission to type into it. |
| `nobody has messaged this pane yet; use klark msg send <to> <text>` | Nothing to reply to; use `send`. |
| `reply` prints `… is not a klark pane, so the reply was saved for it` | Done: the sender is outside klark and reads the answer with `--wait-reply` or `klark msg log`. Do not resend it, and do not look for it on another machine. |
| `klark msg: reply works inside a klark pane (KLARK_PANE_ID is not set)` | `reply` and `name` only work from inside a pane. |
| `no hub configured: pass --hub or set KLARK_HUB_URL` (`klark expose url`) | This machine has no hub. Tell the person; don't guess a URL. |
| `no pane: run this inside a klark pane or pass --pane <pane-id>` | Ask which pane, then `--pane <id>`. |
| `pane P is not exposed: klark expose shares only agents L (--agents / KLARK_EXPOSE_AGENTS); add this agent's name or type to --agents and restart klark expose` | Your pane is filtered out; tell the owner, or `klark msg name` yourself to a listed name. |
| `bad --agents entry "a/b/c": use <name>, <agent-type> or <session>/<name>` | Fix the `--agents` list. |
| hub shows `… is not exposed: this machine exposes only agents L (klark expose --agents)` | The pane/agent is outside the running expose's `--agents`. |
| `klark offer: P looks like a credential; refusing to offer it (pass --force if you are sure)` | Don't offer secrets; `--force` only if the person insists it is safe. |
| the hub page says the machine is not linked | `klark expose` isn't running, or uses another name: retry `klark expose url --name <name>`. |
