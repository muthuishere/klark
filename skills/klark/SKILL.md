---
name: klark
description: "Control Klark, a terminal multiplexer for coding agents: panes, tabs, workspaces, commands and agents; agents across klark sessions; messaging other agents (klark msg); exposing this machine or pane to a klark hub (klark expose, --agents, share links); offering files; the hub (login, chat/transcript view); feeds (klark feed: a log, command, file or URL's new output sent to an agent). Use when the user mentions Klark, or, running inside a Klark pane, says: share this, give me a link, expose, which agents are running, message/ask/tell another agent, tell all agents / broadcast, reply to that agent, send a file / send this to my phone, show me a file, hub, watch this log / tell me when X changes. Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires KLARK_ENV=1."
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
| hear about new output of a log, command, inbox or URL without polling it myself | `klark feed add <agent> "<instruction>" --stream "<command>"` (or `--follow <file>`, `--poll "<command>"`; see `references/feed.md`) |
| share this pane ("share this", "give me a link") | `klark expose url` |
| put this machine on a hub | `klark hub connect --url http://host:7470` (owner approves in the browser; owner only; keeps running in the background, `--foreground` to stay in the terminal) |
| take this machine off the hub | `klark hub stop` (one hub at a time: the last one connected is the default) |
| expose only some agents | `klark expose … --agents reviewer,builder` (or `KLARK_EXPOSE_AGENTS`) |
| send a file to the person / their phone | `klark offer <path>` |
| give me a link to a file / files | `klark link <file>...` (prints a hub viewer link per file) |
| show me / send me a file (any file you mention) | write its **full absolute path** (`/Users/me/Downloads/a.mp3`), never `~/` or a bare name: the hub turns it into a viewer + download link |
| reach agents on other machines | `klark msg login --hub http://hub.lan:7470 --user <name>` (a person types the password) |
| check hub, token, sessions | `klark msg status` |

## References (read the one you need)

| file | covers |
|---|---|
| `references/operations.md` | panes, tabs, workspaces, IDs, caller context, `--machine`, starting and prompting agents, running commands in a pane, read sources |
| `references/messaging.md` | agents across sessions, `klark msg` (send, reply, `--wait`, `--all`), how a message is delivered, error messages and fixes |
| `references/feed.md` | `klark feed`: send a log, command, file or URL's new output to an agent |
| `references/hub.md` | `klark hub`, `klark expose` and share links, `klark offer` / `klark link`, the hub's pages |

They sit next to this file (`<this skill's folder>/references/…`). `klark --skill <name>` prints one (`klark --skill feed`).

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

