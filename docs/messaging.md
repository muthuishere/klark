# Messaging between agents (`klark msg`)

Agents message each other as prompts. A message lands in the other agent's pane as
`[klark msg from <machine>/<session>/<agent>] <text>`. The receiver can answer with `klark msg reply`.

## Addresses

| form | reaches |
|---|---|
| `reviewer` | the agent named `reviewer` in this session (a name or a pane id like `w2:p1`) |
| `work/reviewer` | that agent in the klark session `work` on this machine (the default session is `default`) |
| `laptop/work/reviewer` | that agent on another machine linked to the same hub |

`klark msg whoami` prints this agent's own address. `klark msg name reviewer` renames it.
`klark msg ls` lists every agent: those in all running sessions, plus the hub's (`--local` skips the hub). Worktrees are included.

## Sending

```sh
klark msg send reviewer "please review PR 12"
klark msg send --wait 300 reviewer "when you're done, run the tests"   # waits up to 300 s for it to go idle
klark msg send --all "rebased on master, pull before pushing"          # every other agent in this session
klark msg send --all-sessions "stand-up in 5"                          # every other agent on this machine
klark msg reply "done, 3 comments"                                     # answer the last agent that messaged you
```

- Options go before the address. Everything after it is the message, word for word.
- `--wait <secs>` sends only once the agent is idle: not working, and not showing a dialog. If it is still busy after `<secs>` seconds, the send fails.
- **Your typing is safe.** If the person has a half-typed prompt in the target pane, klark lifts that draft and sends the message on its own. It then puts the draft back, unsent.
- Long messages get a longer pause before Enter. A prompt that didn't submit is entered again.

## Across machines

Messages to other machines go through the hub. Each machine runs `klark hub connect` (it runs in the background; `klark hub stop` ends it).
To send from a plain shell rather than an agent pane, get a token first:

```sh
klark msg login --hub https://hub.example.com --user alice      # asks for the password
klark msg status                                                 # hub, token expiry, sessions
```

## Feeds: a command's new output as messages (`klark feed`)

A feed sends what a background command or a file newly prints to one or more agents, as a prompt, so the agent need not poll (ADR 0015):

```sh
klark feed add reviewer "An ERROR was logged; find the cause." --stream "tail -n0 -F app.log | grep --line-buffered ERROR"
klark feed add reviewer "New mail; what needs an answer?" --poll "himalaya envelope list" --interval 300
klark feed list                 # every feed: agents, state, last output, last sent, who created it
klark feed leave <id>           # unsubscribe the calling pane
klark feed stop <id>            # end it for everyone
```

It lands as `[klark feed <id> · <label>] <instruction>`, then the new lines marked as untrusted data, and a `klark feed leave <id>` line. Unlike `klark msg`:

- it is sent only when the agent is idle (output piles up while it works), at most one message per `--message-gap` (default 30 s), and at most 4 KiB; `klark feed batch <id> <n>` prints a cut batch whole;
- it is not written to `messages.jsonl` and does not become a `klark msg reply` target;
- it is local: subscribers are agents on this machine, and nothing goes through the hub.

The draft protection is the same as for `klark msg send`. Feeds survive restarts and stop themselves after too many messages in an hour (default 30) or 7 days of silence. See `klark feed --help` and the skill.

## History

`klark msg log` shows the recent messages sent and received (`-n 50`, `--json`). The log lives in `~/.config/klark/messages.jsonl`.

Environment: `KLARK_HUB_URL`, `KLARK_HUB_JWT`, `KLARK_HUB_TOKEN` (the machine token), and `KLARK_MACHINE_NAME`.
