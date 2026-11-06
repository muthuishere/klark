# E-01: lazy pull with 100 machines

- Date: 2026-11-06. Status: done.
- Setup: `klark-loadtest`, 100 machines × 8 panes, 10–12 s. The stand-in hub
  sends `get_tree {known_rev}` every 2 s per machine and subscribes to 1 pane on
  every 4th machine.

| result | value |
|---|---|
| connected / failed | 100 / 0 |
| full trees sent | 100 (one per machine, the first pull) |
| `tree_unchanged` replies | ~400–500 |
| unasked messages from machines | only `hello` |
| panes streaming | exactly the 25 subscribed |

**Conclusion:** after the first pull, the pane-list requests cost one small
reply each, and nothing is pushed. The fake trees change every 30 s, so a real
machine whose agents change state more often will send more full trees. It is
still one tree per change per machine, and only while someone is looking.

**Follow-up:** measure a real klark machine's tree size (E-06 notes).
