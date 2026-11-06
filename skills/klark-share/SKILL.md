---
name: klark-share
description: "Hand back the klark hub link for the pane you are running in, so a person can watch and answer this agent from a browser or phone. Trigger: share this, give me a link to this session, open this on my phone."
---

# klark-share

You are running inside a klark pane. When the person asks to share this
session, give them the hub link for your pane. That is all this skill does.

```bash
klark link url
```

It prints one URL, for example `http://hub.lan:8080/go?machine=laptop&pane=w1-3`.
Give that URL to the person as it is.

## How it works, so you can explain it

- This machine is linked to a klark hub (`klark link`, usually started by the
  owner). The hub is a web app where signed-in people see the panes of every
  linked machine.
- The link opens this pane on the hub after the person signs in. Who can see
  it follows the hub's groups; a read-only user can watch but not type.
- Nothing is streamed until someone opens the link, and streaming stops when
  they leave.

## When it fails

| message | what to tell the person |
|---|---|
| `no hub configured` | This machine isn't set up for a hub. The owner sets `KLARK_HUB_URL` (and runs `klark link`). Do not guess a URL. |
| `no pane` | You are not inside a klark pane. Ask which pane to share, then run `klark link url --pane <id>`. |
| the link opens but says the machine is not linked | `klark link` isn't running on this machine, or its name differs. Run `klark link url --name <name>` with the name the owner uses. |

## Rules

- Never print, paste or log the hub token (`KLARK_HUB_TOKEN`).
- Don't start `klark link` yourself unless the owner asks. It connects the whole
  machine to the hub.
- Don't share other panes than your own unless the person names them.
