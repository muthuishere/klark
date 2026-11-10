---
name: klark
description: "Control Klark, a terminal multiplexer for coding agents: panes, tabs, workspaces, commands and agents; agents across klark sessions; messaging other agents (klark msg); exposing this machine or pane to a klark hub (klark expose, --agents, share links); offering files; the hub (login, chat/transcript view). Use when the user mentions Klark, or, running inside a Klark pane, says: share this, give me a link, expose, which agents are running, message/ask/tell another agent, tell all agents / broadcast, reply to that agent, send a file / send this to my phone, show me a file, hub. Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires KLARK_ENV=1."
---

# Klark

Klark organizes terminals into workspaces, tabs, and panes, recognizes coding agents running inside panes, and exposes the current session through the `klark` CLI.

Before issuing any control command, verify that this agent is running inside a Klark-managed pane:

```bash
test "${KLARK_ENV:-}" = 1
```

If the check fails, say that you are not running inside Klark and stop. Do not inspect or control the focused Klark session from outside Klark.

When the check passes, the `klark` binary in `PATH` talks to the current session. Use it to inspect neighboring work, create terminal layout, start agents and commands, read output, and wait for state changes.

## I want to … → command

| I want to … | command |
|---|---|
| see the panes / agents of this session | `klark pane list`, `klark agent list` |
| see which agents are running in ALL sessions | `klark agent list -a` (JSON, `session` + `address`) or `klark msg ls` (table) |
| start an agent next to me | `klark pane split --current --direction right --cwd "$PWD" --no-focus`, then `klark agent start <name> --kind <kind> --pane <id>` |
| prompt an agent and wait | `klark agent prompt <target> "<text>" --wait --timeout 120000` |
| read / wait on an agent in another session | `klark agent read work/coder --lines 40`, `klark agent wait work/coder --timeout 600000` |
| know my own address | `klark msg whoami` → `laptop/work/reviewer` |
| name myself | `klark msg name reviewer` |
| message another agent (any session or machine) | `klark msg send work/coder "<text>"` |
| message an agent only once it is idle (not mid-turn, not on a dialog) | `klark msg send --wait 300 work/coder "<text>"` |
| message every agent in this session (worktrees included) | `klark msg send --all "<text>"` (add `--wait <secs>` to skip busy ones until idle) |
| message every agent in every session on this machine | `klark msg send --all-sessions "<text>"` |
| list every agent, with its worktree | `klark msg ls --local` (`--json`; drop `--local` to include the hub's machines) |
| answer a message I got | `klark msg reply "<text>"` |
| see recent messages | `klark msg log` (`-n 50`, `--json`) |
| share this pane ("share this", "give me a link") | `klark expose url` |
| put this machine on a hub | `klark hub connect --url http://host:7470` (owner approves in the browser; owner only; keeps running in the background, `--foreground` to stay in the terminal) |
| take this machine off the hub | `klark hub stop` (one hub at a time: the last one connected is the default) |
| expose only some agents | `klark expose … --agents reviewer,builder` (or `KLARK_EXPOSE_AGENTS`) |
| send a file to the person / their phone | `klark offer <path>` |
| give me a link to a file / files | `klark link <file>...` (prints a hub viewer link per file) |
| show me / send me a file (any file you mention) | write its **full absolute path** (`/Users/me/Downloads/a.mp3`), never `~/` or a bare name: the hub turns it into a viewer + download link |
| reach agents on other machines | `klark msg login --hub http://hub.lan:7470 --user <name>` (a person types the password) |
| check hub, token, sessions | `klark msg status` |

## Learn the current CLI

The installed binary is the authority for command syntax. Start with:

```bash
klark --help
```

Then print the relevant command group by running the group without a subcommand:

```bash
klark agent
klark pane
klark workspace
klark tab
klark worktree
klark terminal
klark notification
klark integration
klark session
klark machine
```

Do not run bare `klark` for discovery; it launches or attaches the TUI. Do not probe a mutating nested command by omitting arguments. Commands such as `klark workspace create` are valid with defaults and will execute.

Most control commands return JSON. Read identifiers and state from those responses instead of predicting them.

## Understand layout, panes, and agents

Choose the primitive that matches the job:

- Workspace, tab, and pane topology organize terminal locations.
- Pane commands control raw terminals, shells, tests, servers, input, and output.
- Agent commands control the recognized coding agent currently occupying a pane.

A pane exists whether or not it contains an agent. `agent start` requires an existing available shell pane and never creates, splits, or moves layout. Use pane commands for ordinary processes. Use agent commands when Klark must validate agent identity or interpret `idle`, `working`, `blocked`, `done`, and `unknown` lifecycle states.

Agent commands accept either a unique live agent name or the pane ID currently hosting that agent. They do not accept terminal IDs or bare agent-kind labels. Names must match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents. A name follows the current pane occupant and is cleared when that agent exits, is released, or is replaced. Suspending an agent with Ctrl+Z and running a non-agent command preserves its identity while the exact process remains alive. This does not preserve the original identity across another recognized agent taking over. A retained identity does not mean the agent is in the foreground; inspect the pane before sending input.

`idle` and `done` normally indicate the agent is ready for input; neither guarantees that an unfamiliar approval dialog was recognized or that a retained agent is in the foreground. The CLI/API uses the server's seen state to distinguish them; explicit focus commands mark the target seen, while reads do not. Each TUI client tracks viewed completions independently, so its Done badge can differ from the CLI or another client's badge. Antigravity and Grok can be idle while background tasks or subagents keep running, so a settled-state wait is not proof those jobs have finished. `blocked` means Klark recognized an approval or question UI. `unknown` means an agent is present but Klark cannot classify it confidently; it does not prove completion.

## Use IDs and caller context

Public IDs are opaque stable handles:

- workspace: `w1`
- tab: `w1:t1`
- pane: `w1:p1`

Closed tab and pane IDs are not reused. A pane moved into another workspace receives a new workspace-qualified pane ID. After `pane move`, continue with `.result.move_result.pane.pane_id` or the live agent name. The old value is reported as `.result.move_result.previous_pane_id`; only the moved process's inherited caller context keeps resolving that old ID, so do not use it as a general agent target.

Klark injects the caller's context into each managed pane:

```bash
printf '%s\n' "$KLARK_WORKSPACE_ID" "$KLARK_TAB_ID" "$KLARK_PANE_ID"
```

Prefer `--current` when a pane command should target the calling pane. An omitted `pane split` target uses the calling pane when `KLARK_PANE_ID` is available, otherwise the focused pane. Other commands may use the UI-focused pane, which can belong to the user or another client.

Discover live state with:

```bash
klark workspace list
klark tab list --workspace "$KLARK_WORKSPACE_ID"
klark pane current --current
klark pane list --workspace "$KLARK_WORKSPACE_ID"
klark agent list
```

Creation responses expose the IDs to use next. `workspace create` returns `.result.workspace`, `.result.tab`, and `.result.root_pane`. `tab create` returns `.result.tab` and `.result.root_pane`. `pane split` returns the new pane as `.result.pane`.

Other klark sessions on this machine: `klark agent list -a` lists the agents of every running session, each with `session` and `address`. Agent TARGETs accept `session/agent` (e.g. `klark agent read work/reviewer`); a bare name missing from your session is looked up in the others. To message another agent, use `klark msg send` (see "Agents across sessions and messaging" below).

IDs and live agent names are scoped to one server. Two saved SSH machines can both have `w1:p1` or an agent named `reviewer`. Selecting a machine in the TUI does not retarget commands running in your pane: without `--machine`, they still use the inherited session and socket context.

To control a saved SSH machine, use the same global prefix for discovery and every later command:

```bash
klark --machine <label-or-id> agent list
klark --machine <label-or-id> pane list
klark --machine <label-or-id> agent prompt <remote-agent-name> "Reply with your current status." --wait --timeout 120000
```

The selector must be an enabled saved profile ID or a unique, case-sensitive label, not an arbitrary SSH hostname. Commands use that profile's remote session without an open TUI. Do not combine `--machine` with `--session` or `--remote`. Discover IDs on that machine; inherited local IDs and `--current` do not identify remote panes.

Both installations must support machine API forwarding, and the remote server must already be running and API-compatible. Forwarding never installs, starts, or restarts a server and never falls back to Local. Local configuration, session management, installation commands, and interactive attachment are not forwarded. Remote worktree paths must be absolute, `~`, or start with `~/`; plugin link paths must be absolute. A connection failure does not prove a mutation was not applied: inspect remote state before retrying.

`klark machine list` lists saved connection profiles, not a cross-machine pane inventory; add `--json` for scripts. Only add, remove, enable, or disable profiles when the user asks. Removing a profile disconnects the client but does not stop remote sessions. Interactive `machine add` discovers running remote sessions and may ask the user to pick one; non-interactive setup uses the remote default session unless `--remote-session` is explicitly supplied. Use `klark machine status [<label-or-id>] --json` to diagnose a failing machine. `klark machine reconnect` needs the user to complete SSH authentication in their terminal; do not run it for them. Setup asks before stopping an incompatible server and defaults to No; do not approve replacement without the user's consent. Experimental handoff is not part of `machine add`.

## Start and coordinate an agent

Default to a sibling pane in the current tab and the current working directory. Do not create a workspace, tab, worktree, or different cwd unless the user explicitly requests that topology or location.

Honor a direction requested by the user. Otherwise inspect the caller pane:

```bash
klark pane layout --pane "$KLARK_PANE_ID"
```

Split a wide pane to the right and a narrow or tall pane down. Avoid repeated same-direction splits that create unusably narrow columns or short rows. Keep the user's focus in the calling pane and explicitly preserve the caller's working directory:

```bash
klark pane split --current --direction right --cwd "$PWD" --no-focus
```

Replace `right` with `down` when appropriate. Read the new pane ID from `.result.pane.pane_id`.

An available shell pane must be at its interactive prompt, with the shell itself in the foreground and no foreground command, editor, or agent running. Start a supported agent in that pane with a useful unique name:

```bash
klark agent start reviewer --kind codex --pane <returned-pane-id>
```

Use the kind requested by the user. Run `klark agent` to inspect the installed kind list and options. Pass native agent arguments only after `--`:

```bash
klark agent start reviewer --kind codex --pane <returned-pane-id> -- <agent-args...>
```

A successful `agent start` returns only after Klark detects the expected agent in the same pane and considers it ready for interactive input. If the agent is blocked during startup, the command returns `agent_not_ready` immediately but keeps the name available for `agent read` and `agent send-keys`. Wait until the agent becomes idle before prompting it. Startup defaults to a 30-second timeout.

Submit work through the agent surface:

```bash
klark agent prompt reviewer "Review the current diff and report only actionable findings." --wait --timeout 120000
```

`agent prompt` honors the pane's live bracketed-paste mode and sends text followed by encoded Enter as one ordered submission. It reports successful submission only after both have been written; that alone does not prove the agent started a turn. For Codex on Windows, Klark sends a paste boundary before Enter so submission does not depend on prompt size. It rejects an agent already waiting at an approval or question dialog with `agent_blocked` before sending any input. Inspect the blocked UI and ask the user before answering it. For normal agent work, `--wait` is enough: it waits for the first settled `idle`, `done`, or `blocked` state. Do not repeat those defaults with `--until`.

With `--wait`, a prompt sent from a non-working state must produce observed `working` or `blocked` activity. After submission, Klark waits up to five seconds for that activity; unrelated `idle`, `done`, or session changes do not satisfy this gate. It returns `agent_prompt_stalled` if no activity is observed, or `timeout` if the caller's timeout expires first. The caller timeout includes submission time. Without a timeout, the settled-state wait is indefinite after activity is observed. This wait tracks lifecycle state, not an individual turn; if the agent is already working, completion of the active turn may satisfy it.

Use `--until` only for a state-specific workflow, such as waiting for an already-running agent to request input:

```bash
klark agent wait reviewer --until blocked --timeout 120000
```

Without `--until`, standalone `agent wait` uses the same settled-state defaults as `agent prompt --wait`.

Use logical keys for interactive agent UI controls:

```bash
klark agent send-keys reviewer esc
klark agent send-keys reviewer ctrl+c
```

Klark validates all keys before writing any bytes. Read the result through the resolved agent:

```bash
klark agent get reviewer
klark agent read reviewer --source recent-unwrapped --lines 120
```

If a wait fails or returns `blocked`, inspect `agent get` and `agent read` before deciding what input to send. A timeout or stalled response does not prove the prompt was never delivered; do not blindly submit it again. Use the pane surface only when raw terminal control is intentional.

## Run an ordinary command in another pane

Create a sibling pane with the same geometry rule, preserve the caller's working directory, and keep user focus unchanged:

```bash
klark pane split --current --direction right --cwd "$PWD" --no-focus
```

Read the new pane ID from `.result.pane.pane_id`, then run and inspect the command:

```bash
klark pane run <returned-pane-id> "just test"
klark pane wait-output <returned-pane-id> --match "test result" --timeout 120000
klark pane read <returned-pane-id> --source recent-unwrapped --lines 120
```

`pane run` queues command text and Enter together; success acknowledges admission, not command execution. On macOS, `pane run` and `agent start` reject a command that exceeds the terminal's current canonical line-input limit before queuing it. Wait for the interactive shell prompt before retrying. The check covers this submission, not an earlier incomplete line or a later terminal-mode change. `pane wait-output` searches the selected snapshot immediately, so output that already exists can match. Use `--match <text>` for a literal substring or `--regex <pattern>` for a Rust regular expression. Omitting `--timeout` allows an indefinite wait.

Use the read source that matches the task:

- `visible`: the currently rendered viewport.
- `recent`: recent rendered output, including soft wraps.
- `recent-unwrapped`: recent output with soft wraps joined; prefer it for logs and transcripts.
- `detection`: the plain-text bottom-buffer snapshot used for agent detection.

Use `--format ansi` when colors and terminal styling are evidence. Otherwise use text.

`--lines` asks Klark for more rows from the pane's available screen and host scrollback. Alternate-screen rows do not enter ordinary host scrollback. For supported idle agents, Klark can collect application-owned history and restore the viewport afterward, but not every application or response can be recovered this way.

If a larger recent read still does not reveal the completed response, ask the agent to write it as Markdown in a temporary directory and reply only with the file path, then read that file on the same machine. Use this only as a fallback; do not request file output in the initial prompt.

## Agents across sessions and messaging

Every agent has an address `<machine>/<session>/<agent>` (`laptop/work/reviewer`). `agent` is its name, or its pane id (`w1:p1`) if it has none. On this machine the short forms `work/reviewer` and `reviewer` work too.

Every `klark agent` TARGET (`get read prompt send-keys wait focus rename`) accepts `session/agent` and `machine/session/agent` for this machine. A bare name is looked up in your own session first, then in the other running sessions. `klark agent attach` stays in one session. `klark session list` shows sessions.

`klark msg send` vs `klark agent prompt`: `msg send` tags the text with your address, logs it, offers a reply hint and crosses machines; `agent prompt` types raw text into a local agent. To ask another agent something, use `msg send`. The other agent sees:

```
[klark msg from laptop/default/reviewer] Run the tests in crates/x; reply with failing names.
(reply: klark msg reply "<text>")
```

Answer with `klark msg reply "<text>"`; it goes to whoever last messaged your pane. Options go before the address (`klark msg send --json work/coder <text...>`); everything after the address is the message, verbatim. In `klark msg ls`, STATUS `-` means klark cannot classify that agent's screen.

Ask, then wait:

```bash
klark msg send work/coder "Fix the failing test in src/a.rs; reply 'done' or the blocker."
klark agent wait work/coder --timeout 900000
klark agent read work/coder --lines 30
```

Etiquette: one ask per message, say what you need back and in what form, include paths/branch/PR the other agent cannot see. Check `klark msg ls` first: don't message a `blocked` agent (it waits on a person). If the target is `working` and the message can wait, use `--wait <secs>` so it lands between turns instead of mid-turn. Remote targets are not waited on. No "thanks"/"ok" messages.

## Linking to a hub (`klark hub`)

The hub (klarkhub, the `hub/` image) is deployed separately. To link this machine (owner only; it puts the machine on the hub):

```bash
klark hub connect --url http://hub.lan:7470   # prints "Open <url> … (code ABCD-EFGH)"; the owner approves in the browser
klark hub connect                             # later: the default hub, saved token
klark hub connect lab --agents reviewer       # a saved hub by name, only some agents
klark hub stop                                # stop the background connection (log: <config dir>/hub-<name>.log)
klark hub ls | use <name> | rm <name> | status [<name>]
```

Saved hubs live in `<config dir>/hubs.json` (0600; name defaults to the URL's host, `--as <name>` overrides; the first is the default). Scripts skip the browser with `--token <t>`, `--token-stdin` or `KLARK_HUB_TOKEN`. Never print or paste the token. `https://` hubs work; a private-CA ingress needs `KLARK_HUB_CA_FILE=<ca.pem>`. `klark msg` and `klark offer` use the default hub when `KLARK_HUB_URL` is not set.

## Exposing and sharing ("share this")

`klark expose` dials out to a klark hub (no inbound port) and serves this machine: by default every pane of every running session; panes stream only while someone on the hub watches them. Don't start it yourself unless the owner asks: it puts the machine on the hub.

```bash
klark expose --hub http://hub.lan:7470 --token "$KLARK_HUB_TOKEN"            # everything
klark expose --hub http://hub.lan:7470 --token "$KLARK_HUB_TOKEN" --agents reviewer,builder
klark expose … --agent reviewer --agent work/builder                             # repeatable
KLARK_EXPOSE_AGENTS=claude klark expose …                                         # every claude
```

It prints `klark expose: exposing agents: reviewer, builder (all sessions)` or `klark expose: exposing everything (every pane of every session)`. `--agents` entries are case-insensitive: `session/name` matches that named agent (or pane id) in that session; a bare `x` matches agents NAMED `x` in any session, and only if no running agent is named `x` does it match the agent type (`claude`, `devin`, `codex`). A name beats a type. The list is re-checked on every hub pull: an agent that starts later or is renamed to a listed name appears; one that exits disappears. Everything else is hidden from the tree and agent list, and input, reads, transcripts, files and messages to it are refused.

When the person says "share this" / "give me a link" / "open this on my phone", print your pane's link and hand it over as-is:

```bash
klark expose url
# http://hub.lan:7470/go?machine=laptop&pane=w1%3Ap1
```

Options: `--pane <id>` (default `KLARK_PANE_ID`), `--hub <url>` (default `KLARK_HUB_URL`), `--name <machine>` (default `KLARK_MACHINE_NAME`, else the hostname), `--agents` (default `KLARK_EXPOSE_AGENTS`; pass the same list the running `klark expose` uses so an unexposed pane is caught). The URL is built locally; nothing is sent. Share only your own pane unless the person names another.

## Files

```bash
klark offer ./report.pdf            # appears on this pane's hub page as a download/image
klark offer out.png --pane w1:p2    # another pane; defaults to KLARK_PANE_ID
klark offer .env --force            # credential-looking paths are refused without --force
```

Needs `klark expose` running with a hub (`KLARK_HUB_URL`, `KLARK_HUB_TOKEN`). Max 100 MB. Images or files the person uploads on the hub arrive in your pane as a prompt `[image from <user>] <path>`; read the file at that path.

## The hub

- The person signs in to the hub and lands on the pane; access follows hub groups, and a read-only user can watch but not type.
- A pane page has two tabs: **Terminal** (the live screen at the pane's real size) and **Transcript** (a cleaned, chat-style view of the conversation; phones open it by default). They can type replies and upload images there.
- Cross-machine messages need a hub login: `klark msg login --hub <url> --user <name>` (the person types the password, or `--password-stdin`); `klark msg logout` forgets the token; `klark msg status` shows hub, token expiry and running sessions.
- Nothing streams until someone opens a page; it stops when they leave.

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
| `klark msg: reply works inside a klark pane (KLARK_PANE_ID is not set)` | `reply` and `name` only work from inside a pane. |
| `no hub configured: pass --hub or set KLARK_HUB_URL` (`klark expose url`) | This machine has no hub. Tell the person; don't guess a URL. |
| `no pane: run this inside a klark pane or pass --pane <pane-id>` | Ask which pane, then `--pane <id>`. |
| `pane P is not exposed: klark expose shares only agents L (--agents / KLARK_EXPOSE_AGENTS); add this agent's name or type to --agents and restart klark expose` | Your pane is filtered out; tell the owner, or `klark msg name` yourself to a listed name. |
| `bad --agents entry "a/b/c": use <name>, <agent-type> or <session>/<name>` | Fix the `--agents` list. |
| hub shows `… is not exposed: this machine exposes only agents L (klark expose --agents)` | The pane/agent is outside the running expose's `--agents`. |
| `klark offer: P looks like a credential; refusing to offer it (pass --force if you are sure)` | Don't offer secrets; `--force` only if the person insists it is safe. |
| the hub page says the machine is not linked | `klark expose` isn't running, or uses another name: retry `klark expose url --name <name>`. |

## Safety and coordination rules

- Never print, paste or log a token (`KLARK_HUB_TOKEN`, `KLARK_HUB_JWT`, `hub.json`). Never ask for a password in a message or put one on a command line. The hub may be plain HTTP: no secrets in messages or offered files.

- Use `--no-focus` for background work unless the user asked to switch context.
- Use `--current`, an explicit pane ID, or a unique agent name. Do not rely on another client's focused pane.
- Parse IDs from JSON responses. Do not derive them from sidebar order or examples.
- Do not close workspaces, tabs, panes, or sessions you did not create unless the user explicitly asked. Ordinary CLI/API closes do not inherit the TUI's last-pane confirmation. `workspace close --group` closes the primary workspace and its linked worktree workspaces; never add it merely to bypass `workspace_group_close_required`.
- Use `--trust-repository` only after the user has verified the repository. It grants per-request Git trust; it is not a routine retry for a failed worktree command.
- Client and server versions can differ after an update. Check `klark status` before relying on new server features. A missing method is not permission to stop or upgrade a server.
- Never run `klark server stop` from an active session unless the user explicitly intends to stop the server and its pane processes.
- Never kill the main Klark process. Use named test sessions for experiments that need an isolated server.
- CLI server errors are JSON on stderr with exit status 1. CLI syntax errors exit with status 2.

## Showing or sending files

Inside a klark pane, the person may be reading you through klarkhub. Whenever you show, list, create or are asked to send a file, write its full absolute path on its own (for example `/Users/me/Downloads/ganesha-speech.mp3`). The hub makes that path a link that opens the file viewer and downloads it from this machine (up to 10 MB). Do not shorten it to `~/…` or a bare file name, and do not route it through another channel (email, chat bots) unless asked. When the person asks for a link, or may be reading elsewhere (phone, chat), run `klark link <file>...` and paste the links it prints: each opens the hub's viewer (code, markdown, images, PDF) with a download button; nothing is uploaded. For a file the person should get right away as a download on the pane page, run `klark offer <path>`.

## How a message is delivered

`klark msg send` types the message into the target agent and presses Enter, and then:
- If the person was typing in that agent's input, their unsent draft is lifted out first, the message goes in alone, and the draft is typed back without Enter. If they are mid-edit (arrow keys, history), delivery waits up to a minute for them to finish.
- If the agent is still idle 2 s later (the Enter was swallowed after a long paste), Enter is pressed once more. Never when the agent is showing a dialog.
- With `--wait <secs>` it waits until the agent is idle before sending, so it never lands mid-turn or answers a permission dialog by accident.

