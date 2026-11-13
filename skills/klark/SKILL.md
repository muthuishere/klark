---
name: klark
description: "Control Klark, a terminal multiplexer for coding agents: panes, tabs, workspaces, commands and agents; agents across klark sessions; messaging other agents (klark msg); feeds (klark feed: a command, log, file or URL whose new output is sent to an agent); exposing this machine or pane to a klark hub (klark expose, share links); offering files; the hub. Use when the user mentions Klark or a klark command, or, running inside a Klark pane, asks about its panes, agents or sessions, says share this / give me a link / expose / send this to my phone / show me a file / hub. Use messaging and feeds ONLY when the user asks for klark msg or klark feed (or says in klark / via klark): another skill that also sends messages, alerts, mail, WhatsApp or watches things (notifier, herdr-message, a monitor or cron skill) takes precedence. Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires KLARK_ENV=1."
---

# Klark

Klark organizes terminals into workspaces, tabs and panes, recognizes coding agents in panes, and exposes the session through the `klark` CLI.

First check you run inside a Klark pane; if not, say so and stop:

```bash
test "${KLARK_ENV:-}" = 1
```

## Precedence

`klark msg` and `klark feed` are used only when the user asks for them (by name, or "in klark" / "via klark"). If another skill or tool also fits (sending a message, mail, WhatsApp or an alert; watching or polling something; scheduling), that one wins. Never start a feed or message an agent on your own initiative.

## Routing

Read only the reference the request needs; it sits next to this file (`klark --skill <name>` prints it).

<routes>
  <route ask="panes, tabs, workspaces, IDs; start, prompt, read or wait on an agent; run a command in a pane; another machine (--machine)" read="references/operations.md"/>
  <route ask="klark msg: message, reply, broadcast, who am I, list agents across sessions; an error message from klark" read="references/messaging.md"/>
  <route ask="klark feed: watch a command, log, file or URL and send its new output to an agent; manage feeds; a [klark feed …] message arrived" read="references/feed.md"/>
  <route ask="a ready-made feed: GitHub PRs, CI, reviews (gh); files, folders, git; web or API; mail; pods, containers, disk" read="references/feed-recipes.md"/>
  <route ask="share this / give me a link, expose, hub connect or stop, offer or link a file, hub login" read="references/hub.md"/>
</routes>

Most used, without reading further:

| I want to … | command |
|---|---|
| see panes / agents here; in all sessions | `klark pane list`, `klark agent list`; `klark msg ls` |
| prompt an agent and wait | `klark agent prompt <target> "<text>" --wait --timeout 120000` |
| message an agent (when asked) | `klark msg send <session/agent> "<text>"`; answer one: `klark msg reply "<text>"` |
| feed new output to an agent (when asked) | `klark feed add <agent> "<instruction>" --stream "<cmd>"` (or `--follow <file>`, `--poll "<cmd>"`) |
| share this pane | `klark expose url` |

## Learn the current CLI

The installed binary is the authority for syntax: `klark --help`, then a group without a subcommand (`klark agent`, `klark pane`, `klark feed`, `klark msg`, …). Do not run bare `klark` (it opens the TUI), and do not probe a mutating command by leaving out arguments (`klark workspace create` runs with defaults). Most commands return JSON: read IDs and state from it.

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

## Files

Inside a klark pane, the person may be reading you through klarkhub. Whenever you show, list, create or are asked to send a file, write its full absolute path on its own (for example `/Users/me/Downloads/ganesha-speech.mp3`). The hub makes that path a link that opens the file viewer and downloads it from this machine (up to 10 MB). Do not shorten it to `~/…` or a bare file name, and do not route it through another channel (email, chat bots) unless asked. When the person asks for a link, or may be reading elsewhere (phone, chat), run `klark link <file>...` and paste the links it prints: each opens the hub's viewer (code, markdown, images, PDF) with a download button; nothing is uploaded. For a file the person should get right away as a download on the pane page, run `klark offer <path>`.
