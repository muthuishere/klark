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

## Install it for Claude Code

```sh
mkdir -p ~/.claude/skills/klark
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
