---
title: Panes, tabs, workspaces
description: The layout model and the default keys.
---

klark organises terminals into **workspaces**, each with **tabs**, each split
into **panes**. A pane is a terminal. When a coding agent runs in a pane, klark
recognises it and shows its state.

IDs are stable and never reused: workspace `w1`, tab `w1:t1`, pane `w1:p1`.

## Keys

Keys work like tmux: press the prefix `ctrl+b`, then the action key. These are
the defaults (`klark --default-config` prints all of them).

| action | keys |
|---|---|
| help | `ctrl+b ?` |
| detach | `ctrl+b q` |
| split right / down | `ctrl+b v` / `ctrl+b -` |
| move between panes | `ctrl+b h` `j` `k` `l` |
| next pane / previous pane | `ctrl+b tab` / `ctrl+b shift+tab` |
| zoom a pane | `ctrl+b z` |
| close a pane | `ctrl+b x` |
| resize mode | `ctrl+b r` |
| new tab | `ctrl+b c` |
| next / previous tab | `ctrl+b n` / `ctrl+b p` |
| go to tab 1 to 9 | `ctrl+b 1` … `ctrl+b 9` |
| new workspace | `ctrl+b shift+n` |
| workspace picker | `ctrl+b w` |
| toggle the sidebar | `ctrl+b b` |
| settings | `ctrl+b s` |

Change them under `[keys]` in `~/.config/klark/config.toml`, then run
`klark server reload-config` (or `ctrl+b shift+r`).

## The same from the command line

Every layout action is also a command. They print JSON.

```sh
klark workspace list
klark tab create --label tests
klark pane split --current --direction right --no-focus
klark pane run w1:p2 "cargo test"
klark pane read w1:p2 --source recent-unwrapped --lines 50
```

`--current` means the pane the command runs in (`KLARK_PANE_ID`). Inside a
pane, klark sets `KLARK_ENV=1`, `KLARK_WORKSPACE_ID`, `KLARK_TAB_ID` and
`KLARK_PANE_ID`.

All commands: [CLI reference](/reference/cli/).
