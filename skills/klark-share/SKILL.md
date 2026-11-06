---
name: klark-share
description: "Give the person a hub link to the klark pane you are running in, so they can watch and answer you from a browser or phone (terminal or chat-style transcript). Trigger: share this, give me a link to this session, open this on my phone, let me follow along, send me the link."
---

# klark-share

You run in a klark pane. When the person asks to share it, print the hub
link for your pane and hand it over as-is:

```bash
klark expose url
# http://hub.lan:8080/go?machine=laptop&pane=w1%3Ap1
```

Options: `--pane <id>` (default `KLARK_PANE_ID`), `--hub <url>` (default
`KLARK_HUB_URL`), `--name <machine>` (default `KLARK_MACHINE_NAME`, else the
hostname). The URL is built locally; nothing is sent.

## What the person gets (so you can explain it)

- They sign in to the hub and land on this pane. Access follows hub groups;
  a read-only user can watch but not type.
- Two tabs (ADR 0011): **Terminal**, the live screen at the pane's real size,
  and **Transcript**, a cleaned, chat-style view of the conversation that
  phones open by default. They can type replies and upload an image; you get
  it as a prompt `[image from <user>] <path>`.
- Files you send with `klark offer <path>` show up on that page.
- Nothing streams until someone opens the page; it stops when they leave.

## When it fails

| it prints | what to do |
|---|---|
| `no hub configured: pass --hub or set KLARK_HUB_URL` | This machine has no hub. Tell the person; the owner sets `KLARK_HUB_URL` and runs `klark expose`. Don't guess a URL. |
| `no pane: run this inside a klark pane or pass --pane <pane-id>` | You are not in a pane. Ask which pane, then `--pane <id>`. |
| the page says the machine is not linked | `klark expose` isn't running here, or uses another name: retry with `--name <name>` the owner uses. |

## Rules

- Never print or paste the hub token (`KLARK_HUB_TOKEN`).
- Don't start `klark expose` yourself unless the owner asks: it puts the
  whole machine on the hub.
- Share only your own pane unless the person names another.
