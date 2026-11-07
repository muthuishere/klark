# ADR 0011: temporary assets on the hub's own file store, faithful pane rendering, chat view

- Status: accepted (owner, 2026-11-06), amended the same day (owner): no
  MinIO; the Transcript tab is a chat view fed by `get_transcript`. Link
  protocol **v3**.
- Ports ideas from herdr-expose (`internal/serve/upload.go`, `offer.go`,
  `offerapi.go`, `internal/core/clean.go`, `transcript.go`). No code is shared
  (ADR 0008).

## Context
A comparison with herdr-expose (2026-11-06) found three gaps:
1. **Rendering bug.** `klark expose` observes panes at the default 120x40
   (`src/cli.rs:642`), and it drops the frame's `width`, `height` and `full`
   fields (`crates/klark-link/src/upstream.rs:270`). The browser then
   `fit()`s xterm to the window (`hub/wwwroot/terminal.js:22`). As a result,
   TUIs garble and lines wrap in the wrong places.
2. **No reading view.** herdr-expose's phone-first, cleaned transcript (with
   markdown) has no counterpart in klark.
3. **No files either way.** People cannot send a phone image to an agent,
   and an agent cannot offer a file to the browser.

## Decision

### A. One process: the hub's own content-addressed file store (amended: no MinIO)
- The first draft embedded a MinIO server in the hub image. The owner
  dropped it: MinIO stopped publishing community binaries (dl.min.io
  answers `410 Gone`), the project is archived, and a second process with its
  own credentials buys nothing a single-node hub needs.
- Assets live in the hub's **own content-addressed file store on the /data
  volume; no MinIO**: `<DataDir>/assets/sha256/<aa>/<hex>`. A write streams
  to a temp file (`<DataDir>/assets/tmp/`, same volume), is fsynced, and is
  renamed into place atomically. Directories are 0700, files 0600. An object
  that already exists is not written again.
- The code talks to an `IAssetStore` interface (`FileAssetStore` today), so
  another backend can be added later without touching the routes.
- **Every asset is temporary.** A background sweep (every 10 minutes) deletes
  `assets` rows older than `Hub:Assets:RetentionHours` (default 24) and then
  every object no remaining row names. There is no permanent storage path,
  and the UI says "files expire after 24 h".
- One process, one port (8080); the image's entrypoint is `dotnet
  KlarkHub.dll` as before. Every byte passes through the hub, which enforces
  auth, caps and types.

### B. Assets
- Object key: `sha256/<aa>/<hex>`. An identical upload is stored once (the
  store checks whether the object exists before renaming a new one in).
- The hub stores `assets(sha256, size, mime, name, ts, machine, pane_id,
  direction, by)` in `hub.db`. `direction` is `to_agent` or `from_agent`.
- Type is decided from magic bytes, never from the name or the client's
  claimed type. `text/html`, `image/svg+xml` and `*/xml` are served as
  `application/octet-stream`. Every response sends `nosniff`, and anything
  that is not an image gets `Content-Disposition: attachment`.
- Caps: 20 MB for images from people, 100 MB for files offered by agents.

**Person to agent (an image from the phone):**
1. In the pane page, the person pastes, drops, picks or takes a photo. They
   must be allowed to type (`HubAccess.CanType`).
2. The hub checks the cap and type, stores the object and records the asset.
3. The hub sends `fetch_asset {id, sha256, name, mime, size, pane_id, caption}`.
4. The machine downloads `GET /api/assets/<sha256>` with its machine token
   (allowed only for assets addressed to that machine). It writes the file
   to `<klark data dir>/uploads/<sha256>.<ext>` (directory 0700, file 0600,
   swept after 24 h) and checks the sha256.
5. The machine sends `agent.prompt` to the pane with
   `[image from <user>] <absolute path>` plus the caption, and replies with
   `result {id, result:{path}}`.

**Agent to person (`klark offer <path> [--pane <id>] [--force]`):**
- The command refuses credential-looking paths (`.env`, `*.pem`, `id_*`,
  `.ssh/`, `credentials`, `*.key`) unless `--force` is given, and files over
  100 MB.
- `PUT <hub>/api/assets?pane=<pane>&name=<name>` with the machine token, and
  the bytes as the body. This uses plain HTTP, not the expose socket, so a
  big file never blocks pane streams. The pane defaults to `KLARK_PANE_ID`.
- The hub records the asset as `from_agent` for that machine and pane. The
  pane page lists offered files, shows images inline, and offers the rest as
  downloads.

**Serving:** `GET /assets/<sha256>` (cookie, or Bearer JWT). The caller must
be able to see the asset's machine. The hub streams the object from its file store.

### C. Faithful rendering (protocol v3)
- The exposer observes a pane **at the pane's own size**, never at a default
  (check how `terminal session observe` picks its geometry, and pass the
  pane's real size or none).
- Machine to hub, only while the pane is subscribed:
  `pane_size {pane_id, cols, rows}`. It is sent on subscribe and whenever an
  observe frame reports a new size.
- The hub keeps the last size per pane and passes it to viewers. The browser
  calls `term.resize(cols, rows)` and never `fit()`. The terminal is scaled
  with CSS to fit the width, and scrolls horizontally on very narrow screens.
- The first paint comes from the observe stream's full frame when there is
  one. The `pane.read` paint is written raw, without `\n` to `\r\n` rewriting.
- xterm gets the Unicode 11 addon, the web-links addon (safe schemes only)
  and a dark theme. Its scripts are bundled in `wwwroot/lib/` rather than
  loaded from a CDN, so the hub works offline on a LAN.

### D. Transcript view
(Amended: the tab is now **Chat**, see E. On v3 machines its data comes from
`get_transcript`; what follows is the fallback for machines below v3 and for
`source:"screen"`.)
- Each pane page has two tabs, **Terminal** and **Transcript**. Phones (narrow
  screens) default to Transcript; desktops default to Terminal.
- Transcript text comes from `agent.read`, falling back to
  `pane.read {source:"recent_unwrapped"}`. It is pulled about once a second,
  and only while that tab is open and visible (lazy, ADR 0002). Several
  viewers of one pane share one poll.
- A server-side cleaning pass is ported from herdr-expose `clean.go`: drop
  the agent's status and footer lines, collapse runs of blank lines, strip
  ANSI. The result is rendered as markdown with safe links only.
- The image upload and the offered files appear on both tabs.

### E. Chat view (amended)
- The Transcript tab is a chat, modelled on the Claude Code mobile view: one
  full-height column (centred, max ~760px, on desktop), a compact header
  (back arrow, agent name, live status dot, machine and session), person
  and agent turns as distinct blocks, markdown with code blocks (language
  label, copy button, highlighting from a bundled highlight.js 11.11.1),
  tool calls and thinking collapsed into one-line rows (a run of calls
  folds into one "N tool calls" row), a live working/idle/blocked line, a
  sticky composer (Enter sends through the same CanType-checked input
  path, Shift+Enter is a newline, an attach button uses the upload flow),
  auto-scroll that stops when the reader scrolls up, with a "Jump to latest"
  pill. A blocked agent's question is read from its screen and shown with
  quick-reply buttons for numbered or yes/no options.
- Data (protocol v3): hub to machine `get_transcript {id, pane_id, after?}`;
  the machine replies `result {id, result:{agent, cursor, items:[{id,
  role, kind, text?, tool?, input?, ok?, ts?}], source:"jsonl"|"screen"}}`.
  Only Claude Code and Devin have structured logs; every other agent arrives
  as `source:"screen"`, one assistant item that the hub cleans with the
  clean.go port. The hub passes `cursor` back as `after`, so a poll returns
  only newer items. Machines below v3 fall back to `agent.read`, then
  `pane.read {source:"recent_unwrapped"}`, cleaned, as one assistant block.
- One poll per pane, about once a second, shared by every viewer and only
  while a chat is open. The items stay in hub memory after the last viewer
  leaves, so coming back paints at once and then fetches only what is newer.
  Markdown is rendered once per item on arrival, and only the newest 150 rows
  are in the DOM ("Show earlier" adds more).
- Navigation: `/m/{machine}/p/{pane}?view=chat|terminal` are real routes.
  Per browser (localStorage/sessionStorage) the hub keeps the list's search
  text and scroll, each pane's tab, chat scroll position (or "at latest"),
  opened tool rows and the composer draft.
- (Amended) Back never leaves the app: the header arrow goes back when the
  chat was opened from the list in this tab (same path, nothing pushed since),
  otherwise it replaces the entry with the list. Tab switches replace the
  history entry. Also kept per tab: the open search and its current match,
  the chosen quick reply and the file panel's state; the list marks the chat
  you came from. The sign-in form replaces its own history entry and keeps
  the return URL's query string.
- (Amended) Screen text (`source:"screen"` and machines below v3) is split
  into turns after cleaning: `> `/`❯ `/`› ` at column 0 is the person,
  `⏺ Tool(args)` (or `• Tool(args)`) a collapsed tool row, `⎿ `/`└ ` lines its
  result, other `⏺ `/`• ` blocks the agent. Cleaning also drops prompt-box
  borders, "Context left until auto-compact", "? for shortcuts", "esc to
  interrupt", "(ctrl+r to expand)", plan/accept-edits/bypass banners, spinner
  and cost/token lines, and Claude Code/Codex/Devin version banners; nothing
  inside a fenced code block is touched, and table borders and short `---`
  rules are content. Structured items drop system/meta items, harness
  wrappers (`<system-reminder>`, `<local-command-stdout>`) and empty items.
- (Amended) Idle pause (`wwwroot/idle.js`): after 60 s with no mouse,
  touch, key or scroll input (a hidden tab gets none), the page calls
  `Blazor.pauseCircuit()`. The server disposes the circuit, so every tree
  pull, chat poll and pane stream for that tab stops. A pill says "Paused
  while you're away · last updated N minutes ago". On the next input, or when
  the tab is shown again, it says "Last updated N ago · refreshing…" and
  reloads; view state survives the reload (storage, see Navigation). A tab
  that dies without closing is released after 30 s
  (`DisconnectedCircuitRetentionPeriod`).
- Chat search covers the items that are loaded; `get_transcript` only goes
  forward (`after`), so older history than the hub holds cannot be fetched.

## Consequences
- The image stays one process with no third-party server and no AGPL
  component. Assets share the /data volume with hub.db; the retention sweep
  bounds them (at most 24 h of uploads by default).
- One hub node only: the file store is local. A multi-node hub would need a
  shared `IAssetStore` (for example S3), which this ADR does not add.
- Kitty graphics drawn inside the terminal (`ServerMessage::Graphics`,
  dropped today) can later flow through the same asset store. That is not
  part of this ADR.
