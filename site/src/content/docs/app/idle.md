---
title: Idle pause
description: A tab you are not using stops costing the hub and the machines anything.
---

After **60 seconds** with no mouse, touch, key or scroll input, the page pauses
itself. A hidden tab gets no input, so it pauses too.

While paused, the server drops that tab's session. Every tree pull, chat poll
and pane stream for that tab stops. A pill says:

```text
Paused while you're away · last updated N minutes ago
```

On your next input, or when the tab is shown again, it says
"Last updated N ago · refreshing…" and reloads. The list's search and scroll,
the chat's scroll position, open tool rows and your draft all survive the
reload.

A tab that dies without closing (a crashed browser, a phone off Wi-Fi) is
released after **30 seconds**.

This is why an open-but-forgotten hub tab costs nothing: lazy pull extends all
the way to the browser.
