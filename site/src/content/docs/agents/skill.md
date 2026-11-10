---
title: The klark skill
description: Teach Claude Code (or any agent with skills) to drive klark.
---

klark ships one agent skill, `skills/klark/SKILL.md`. It teaches an agent
running inside a klark pane to:

- list and start agents in panes next to it, prompt them and wait;
- see agents in other sessions and message them (`klark msg`);
- hand back a hub link for its own pane ("share this" → `klark expose url`);
- offer a file to the person on the hub (`klark offer`);
- read the common errors and fix them.

The same file is compiled into the binary, so it always matches your klark.

## Installed automatically

klark installs the skill when `klark server` starts. It writes to:

- `~/.claude/skills/klark/SKILL.md` (Claude Code)
- `~/.agents/skills/klark/SKILL.md` (the shared agents folder)

It writes only where that folder already exists. It installs when `SKILL.md` is
missing or out of date. Each install is recorded in `versions.txt` in klark's
config directory (`~/.config/klark/versions.txt`), so you can see which klark
version installed the skill. It never replaces a `SKILL.md` that is not
klark's own. Set `KLARK_NO_SKILL_INSTALL=1` to turn this off.

To install it by hand, or to see the exact file this binary ships:

```sh
klark --skill > ~/.claude/skills/klark/SKILL.md
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
