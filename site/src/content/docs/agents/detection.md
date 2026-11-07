---
title: Agent detection and names
description: How klark recognises agents, what the states mean, and how to name an agent.
---

klark watches each pane and recognises the coding agent running in it. It reads
the screen and terminal title against built-in rules, one manifest per agent
(`src/detect/manifests/`). Nothing is fetched from the network.

Recognised agents include Claude Code, Codex, Devin, GitHub Copilot, Gemini,
Cursor, OpenCode, Amp, Cline, Droid, Grok, Kimi, Kiro, Qwen, pi and others.

## States

| state | meaning |
|---|---|
| `working` | the agent is busy on a turn |
| `blocked` | the agent shows an approval or question and waits for a person |
| `idle` | ready for input |
| `done` | finished a turn you have not looked at yet |
| `unknown` | an agent is there, but klark cannot classify its screen |

The sidebar and the hub list blocked agents first.

## Hooks for more exact states

Some agents can report their own state through hooks. Install them with:

```sh
klark integration install claude    # also: codex, copilot, devin, droid, kimi, opencode, pi, omp, …
```

`klark integration` lists every target.

## Names

An agent can have a name, which is also its address for messaging. Names match
`[a-z][a-z0-9_-]{0,31}` and are unique among live agents.

```sh
klark agent rename w1:p2 reviewer    # from anywhere
klark msg name reviewer              # from inside the agent's own pane
klark agent list                     # this session
klark agent list -a                  # every running session
```

A name follows the agent in that pane. It is cleared when the agent exits or
another agent takes over the pane.

## Driving an agent

```sh
klark agent start reviewer --kind codex --pane w1:p2
klark agent prompt reviewer "Review the current diff." --wait --timeout 120000
klark agent read reviewer --source recent-unwrapped --lines 120
klark agent wait reviewer --until blocked --timeout 120000
klark agent send-keys reviewer esc
```

`agent prompt` refuses a `blocked` agent (`agent_blocked`) before it types
anything. Agent targets accept a name, a pane id, `session/agent`, and
`machine/session/agent` for this machine.

## Transcripts

For Claude Code and Devin, klark reads the agent's own structured log (Claude's
JSONL, Devin's SQLite) and turns it into chat items. For every other agent it
uses the cleaned screen text. The hub uses this for its [chat view](/app/chat/).
The command behind it is `klark transcript --pane <id> --json`.
