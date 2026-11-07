---
title: What a machine exposes
description: klark expose, --agents to share only some agents, and share links for one pane.
---

`klark hub connect` runs `klark expose` for you. You can also run it directly
with a token:

```sh
klark expose --hub http://hub.lan:8080 --token "$KLARK_HUB_TOKEN" [--name laptop]
```

Or set `KLARK_HUB_URL`, `KLARK_HUB_TOKEN` and `KLARK_MACHINE_NAME`.

## What it shares

By default: every pane of every running session on this machine. It prints:

```text
klark expose: exposing everything (every pane of every session)
```

The hub pulls. A machine sends nothing unasked, and a pane's output streams only
while someone has that pane open. When the last viewer leaves, it stops.

From the hub, a machine accepts only reads, text input and keys. It never
accepts calls that close panes or change state.

## Only some agents: `--agents`

```sh
klark expose … --agents reviewer,builder
klark expose … --agent reviewer --agent work/builder     # repeatable
KLARK_EXPOSE_AGENTS=claude klark expose …                # every Claude Code pane
klark hub connect --agents reviewer                      # same flag on connect
```

It then prints `klark expose: exposing agents: reviewer, builder (all sessions)`.

Matching, case-insensitive:

- `session/name` matches that named agent (or pane id) in that session.
- A bare `x` matches agents **named** `x` in any session.
- Only if no running agent is named `x` does it match the agent **type**
  (`claude`, `devin`, `codex`). A name beats a type.

The list is checked again on every hub pull. An agent that starts later, or is
renamed to a listed name, appears. One that exits disappears. Everything else is
hidden, and input, reads, transcripts, files and messages to it are refused.

## A link to one pane: `klark expose url`

Inside a pane:

```sh
klark expose url
# http://hub.lan:8080/go?machine=laptop&pane=w1%3Ap1
```

The link is built locally; nothing is sent. Options: `--pane <id>` (default
`KLARK_PANE_ID`), `--hub <url>`, `--name <machine>`, `--agents <list>`. Pass the
same `--agents` list as the running `klark expose`, so a hidden pane is caught
with an error instead of a dead link.

## Offer a file to the person: `klark offer`

```sh
klark offer ./report.pdf          # shows up on this pane's hub page
klark offer out.png --pane w1:p2
```

It uploads with the machine token (100 MB at most) and refuses credential-looking
paths (`.env`, `*.pem`, `id_*`, `.ssh/`, `credentials`, `*.key`) unless you add
`--force`. Files expire on the hub after 24 hours. See [Files](/app/files/).
