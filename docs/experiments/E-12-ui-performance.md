# E-12: UI performance (chat shell, file viewer)

Date: 2026-11-07. ADR 0011 E2, F, G.

## Setup

Hub from this branch (`dotnet run`, Production, MockLdap) on 127.0.0.1:8099; a
debug `klark server` + `klark expose` on the same Mac (M-series) with XDG dirs
under `/tmp/kx`; three panes (fake Claude screen, fake Devin screen, plain bash
running a coloured cargo-test script). Browser: the chrome-agent Chromium fork,
1920x873 window, numbers read with `chrome-agent evalAsync` from
`performance.getEntriesByType('navigation' | 'paint' | 'resource')`. Sizes are
the files on disk, gzip -9. Static files are not compressed on the wire by
the hub today (transfer size equals body size), so "gz" is what a compressing
proxy or a precompressed publish would send.

## Initial JavaScript (excluding `blazor.web.js`)

| | raw | gz |
|---|---:|---:|
| before (a35764e2): chat.js, terminal.js, idle.js, xterm.js + 2 addons, highlight.min.js | 443.1 KB | 120.6 KB |
| after: chat.js, terminal.js, idle.js | 26.3 KB | 9.3 KB |

CSS: before 24.9 KB raw / 7.2 KB gz (app.css + xterm.css + hljs theme), after
28.1 KB / 7.0 KB (app.css only; token colours are themed in it).

Loaded only when needed:

| asset | gz | when |
|---|---:|---|
| `lib/hljs-11.11.1/core.min.js` | 8.0 KB | first code block in a chat, or a file in the viewer |
| one grammar, e.g. `languages/rust.min.js` | 1.5 KB | first block / file in that language |
| `lib/xterm.js` (+ css, addons) | 65.7 KB + | first time the Terminal toggle is opened |
| `viewer.js` + `viewer.css` | 7.2 KB | only on `/m/{machine}/f` |
| `viewer-hl.js` (worker) | 1.0 KB | a text file under 2 MB in the viewer |

Confirmed in the browser: a chat with one Rust block fetched exactly
`app.css, chat.js, terminal.js, idle.js, blazor.web.js, hljs core.min.js,
languages/rust.min.js` (no viewer, no xterm). The viewer page fetched
`viewer.css, viewer.js` and the file API, and no Blazor.

## Chat page

Fresh load of a Claude chat (8 items) on localhost: DOMContentLoaded 79 ms,
first contentful paint 124 ms. Items are rendered once each (markdown on
arrival, in the hub), rows re-render only when their stamp changes, and at most
150 rows are in the DOM (unchanged from before).

## File fetch (hub → machine `get_file` → hub)

Debug `klark expose`, first fetch (no hub cache):

| file | size | time |
|---|---:|---:|
| README.md | 3 KB | 60 ms |
| logo.png | 612 KB | 142 ms |
| mid.rs | 2.0 MB | 195 ms |
| big.rs | 8.7 MB (95,000 lines) | 660 ms |

Before the fix in `Session::handle` (get_file not tracked as pending), the same
2 MB file took 21.6 s: an idle link slept until the hub's next event, so the
answer waited for a heartbeat. Machine side alone (read + base64 + encode +
gzip) is 136 ms in debug and 8 ms in release for 2 MB.

## Viewer, 8.7 MB / 95,000-line file

Metadata (cached) 22 ms, raw 51 ms after navigation. 119 rows in the DOM.
120 scroll steps of 300 px, one per animation frame: frame time p50 16.7 ms,
p95 16.8 ms, max 17 ms (60 fps). Files over 2 MB are not highlighted, so they
open at once; smaller files are highlighted in a worker after first paint.

## Not measured (budgets from the owner, still open)

- Sidebar with 500 agents at 60 fps: groups use `content-visibility: auto`, but
  no 500-pane bed was run.
- Chat with 1,000 items to interactive in < 300 ms: not run (the screen beds
  here produce under 20 items).
- Fingerprinted, immutable caching: `app.css`/`chat.js` go through Blazor's
  `@Assets[...]` (fingerprinted); `lib/hljs-11.11.1/`, `viewer.*` are
  versioned by path only and get ETags, not `immutable`.
