# Klark: hub, exposing, sharing and files

Part of the klark skill; the rules in `SKILL.md` (KLARK_ENV check, safety) apply. Error messages: `messaging.md`, "When it fails".

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

## Links to files

`klark link <file>...` prints a hub viewer link per file (code, markdown, images, PDF, with download); nothing is uploaded. `klark offer <path>` puts a file on the pane's hub page as a download.
