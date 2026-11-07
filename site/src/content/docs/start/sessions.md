---
title: Sessions
description: Named, persistent sessions, and klark -s for one session per project folder.
---

A session is a set of workspaces, tabs and panes kept alive by a background
server. Each session has its own server and socket. Detaching leaves every
process running.

## The default session

```sh
klark            # start or attach the default session
```

## One session per folder: `klark -s`

`-s` creates a session or attaches to it, so the same command works the first
time and every time after.

| command | session name | starts in |
|---|---|---|
| `klark -s` | the current folder's name | the current folder |
| `klark -s ~/code/api` | `api` | `~/code/api` |
| `klark -s .` / `klark -s ../web` | that folder's name | that folder |
| `klark -s review` | `review` (if no folder called `review` exists here) | the current folder |

Rules, from the code:

- A value with a `/`, or `.`, `..`, or starting with `~`, is a folder. It must
  exist, or klark says `<value>: no such folder`.
- Any other value that is an existing directory is also a folder. Otherwise it
  is the session name itself.
- A folder name becomes a session name by turning every character other than
  letters, digits, `_` and `-` into `-`, trimming `-` from the ends, and
  cutting it at 64 bytes. `my app.v2` becomes `my-app-v2`.
- A klark command word after `-s` is not a name: `klark -s pane list` runs
  `pane list` in the folder's session.
- `klark -s=<name>` and `klark --session <name>` also work.

## Managing sessions

```sh
klark session list              # running and saved sessions
klark session attach work       # attach to "work"
klark session stop work         # stop its server and panes
klark session delete work
```

Use `default` as the name to target the default session with `stop`.

## Remote sessions

`klark --remote <ssh-target>` attaches to a klark server on another machine over
SSH. `klark machine add <ssh-target>` saves a machine so `klark --machine <label>
<command>` can run API commands there. See the
[CLI reference](/reference/cli/#klark-machine).
