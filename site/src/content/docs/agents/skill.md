---
title: The klark skill
description: Teach Claude Code (or any agent with skills) to drive klark.
---

klark ships one agent skill, `skills/klark/`: a short `SKILL.md` that the
agent always reads, and one file per kind of work under `references/` that it
reads only when needed (`operations.md`, `messaging.md`, `feed.md`, `hub.md`).
It teaches an agent
running inside a klark pane to:

- list and start agents in panes next to it, prompt them and wait;
- see agents in other sessions and message them (`klark msg`);
- have a background command's new output (a log, an inbox, a URL) sent to an
  agent (`klark feed`);
- hand back a hub link for its own pane ("share this" → `klark expose url`);
- offer a file to the person on the hub (`klark offer`);
- read the common errors and fix them.

The same files are compiled into the binary, so it always matches your klark.

## Installed automatically

klark installs the skill when you install klark with `install.sh`,
`install.cmd` or `install.ps1`, and again whenever `klark server` starts (so an
update brings the new skill). It writes the whole folder to:

- `~/.claude/skills/klark/` (Claude Code)
- `~/.agents/skills/klark/` (the shared agents folder)

It writes only where that folder already exists. It installs when `SKILL.md` is
or any reference is missing or out of date, and removes references
an older release shipped. Each install is recorded in `versions.txt` in klark's
config directory (`~/.config/klark/versions.txt`), so you can see which klark
version installed the skill. It never replaces a `SKILL.md` that is not
klark's own. Set `KLARK_NO_SKILL_INSTALL=1` to turn this off.

To install it by hand, or to read what this binary ships:

```sh
klark --install-skill     # the same install, now
klark --skill             # print SKILL.md
klark --skill feed        # print one reference: operations, messaging, feed, hub
```

Restart Claude Code. The skill triggers on phrases such as "share this",
"which agents are running", "ask the other agent", or "send this to my phone".

## It only acts inside klark

The skill first checks `KLARK_ENV=1`, which klark sets in every pane. Outside
klark it says so and stops.

## Safety rules it follows

- It never prints a token or puts a password on a command line.
- It does not run `klark expose` or `klark hub connect` unless the owner asks,
  because that puts the machine on a hub.
- It does not close panes, tabs or sessions it did not create.
- `klark offer` refuses credential-looking files unless `--force` is given.
