---
title: Quick start
description: Install klark with one line, open a terminal, add Claude, Codex, Devin and other agents, and message between them.
---

From nothing to agents working side by side, in five steps.

## 1. Install (one line)

macOS and Linux:

```sh
curl -fsSL https://muthuishere.github.io/klark/install.sh | sh
```

or with wget:

```sh
wget -qO- https://muthuishere.github.io/klark/install.sh | sh
```

Windows (Command Prompt):

```bat
curl -fsSL https://muthuishere.github.io/klark/install.cmd -o "%TEMP%\klark-install.cmd" && "%TEMP%\klark-install.cmd"
```

The scripts download the right build from the latest GitHub release, checks its
checksum, installs it for your user (no admin, no root) and puts it on your PATH. More
options, and a manual download, are on the [Install](/start/install/) page.

## 2. Open klark

```sh
klark
```

This starts the background server and attaches to your session. Detach any time with
**`ctrl+b q`**: your agents keep running, and `klark` attaches again.

Want a separate session for each project? Run `klark -s` (or `klark --session`) in the
project folder. The session is named after the folder, created the first time and
attached after that.

## 3. Add your agents

You do not register agents. Start them in a pane, the way you always do, and klark
recognises them:

```sh
claude
```

Open more panes and start the others:

| you want | press |
|---|---|
| a second pane, side by side | `ctrl+b v` |
| a second pane, below | `ctrl+b -` |
| a new tab | `ctrl+b c` |
| a new workspace | `ctrl+b shift+n` |
| move between panes | `ctrl+b h` `j` `k` `l` |

In each pane run `codex`, `devin`, `agy`, `gemini`, `opencode` or any other coding
agent. The sidebar lists every agent with its name and state (`working`, `blocked`,
`idle`, `done`), blocked ones first. `ctrl+b ?` shows every key.

For more exact states, install the agent's hooks once:

```sh
klark integration install claude    # also: codex, devin, opencode, ...
```

## 4. Let them talk to each other

```sh
klark msg ls                                     # every agent, in every session
klark msg send reviewer "please review PR 12"    # message one agent
klark msg send --all "rebased on master, pull before pushing"
```

Run these from a shell or from inside an agent. The message arrives as the other agent's
prompt, says who sent it, and the receiver can answer with `klark msg reply`. See
[Messaging](/agents/messaging/) for the rest.

## 5. Optional: watch from a browser or your phone

Run the hub once, then connect this machine to it:

```sh
docker run -d --name klarkhub -p 7470:7470 -v klarkhub-data:/data ghcr.io/muthuishere/klarkhub:latest
klark hub connect --url http://localhost:7470
```

Approve the code it shows in the hub (open `http://localhost:7470`, sign in as `admin`,
first password `adminadmin`, and change it when asked). Your agents then appear in the
browser, with a phone layout. `klark hub stop` disconnects. The full guide is
[Run the hub](/hub/run/).

---

klark is a fork of [herdr](https://github.com/herdrdev/herdr). Its panes, tabs,
workspaces and agent detection work the way herdr's do; the messaging and the hub are
what klark adds.
