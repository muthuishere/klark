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

### E. Chat view (amended; the layout is now E2)
- The Transcript tab is a chat, modelled on the Claude Code mobile view: one
  full-height column (centred, max ~760px, on desktop; see E2), a compact header
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

### E2. The app shell (amended 2026-11-07, supersedes E's layout)
Owner feedback: the pane page was a terminal in a 760 px column with a "Stream
ended" banner. It is now laid out like the Claude desktop app.
- **Shell.** A left sidebar and a main area, on every signed-in page except
  admin and account pages. The sidebar is collapsible on desktop (remembered
  per browser) and a drawer on screens up to 860 px; on a phone the list page
  *is* the sidebar. Main area: a top bar (agent badge, title, session pill, a
  small status word: working…, needs you, idle, offline, disconnected, stream
  ended; the Terminal toggle icon; a menu), the content, and the composer. The
  reading column is centred inside the main area, max 900 px (1100 px for a
  shell), not inside the whole page.
- **Chat is the default** on every screen size, always. The terminal opens only
  from its toggle (or `?view=terminal`), never because of a saved preference.
- **Sidebar.** Only agent panes (an agent name, or a real agent status) are
  listed; plain shells and empty panes are hidden unless "Show all panes" is on
  (per browser). Grouped by project folder (cwd basename, `~` for home, full
  path on hover), collapsible groups (remembered), blocked agents first, then
  newest activity. Each entry: status dot, agent badge (Claude, Devin, Codex,
  other, shell), title, last activity ("5m"), and a one-line preview of the
  latest message (or machine · session). Search (Cmd/Ctrl+K) is AND over
  words in name, agent, status, folder, full path, session, machine and pane
  id (herdr-expose `paneSearch`), plus a "Messages" section that searches the
  text of every chat this hub holds in memory, shows a snippet with the hit
  marked, and opens the chat scrolled to that item (`?hit=<item id>`).
- **Per pane type.** Claude panes: Claude-style chat (serif prose, real tables,
  "Read 3 files · Ran 2 commands ✓" tool rows). Devin and Codex: the same chat
  with their own accent. Other agents: the generic transformer as chat. A plain
  shell: Warp-style blocks, each command a header row and its output under it
  (collapsible, coloured), because a shell has no "turns" to speak of but does
  have command/output pairs (compared with Claude desktop, Devin's web app,
  Warp and herdr-expose: Warp's block is the only model that fits a shell).
- **Composer** is one large rounded box ("Type a message", attach +, send).
  It is client-side: typing never calls the server (the browser circuit is
  long polling, ADR 0013); Send takes the text in one call.
- **Files the agent offered** (`klark offer`) are attachment cards at the end
  of the chat (thumbnail or icon, name, size, download).

### F. Screen transformers (2026-11-07)
When there is no structured log (Claude jsonl, Devin sqlite), the machine sends
the pane's `recent_unwrapped` text **with ANSI** and the hub turns it into chat
items. They live in the hub (`hub/Services/ScreenTransformers.cs`, `AnsiHtml.cs`,
`TranscriptClean.cs`), not in `klark transcript`, because: the hub also serves
machines below v3 that never run the Rust command; the hub already owns the
markdown renderer they feed; and a transformer fix ships by redeploying one hub
rather than every machine. The pick: the pane's agent name, else the screen's
own markers.
- **Claude**: chrome cleaning (D), then `>`/`❯` prompts, `⏺`/`●` turns, `⎿`
  results.
- **Devin** (and Codex): `›` prompts, `•` turns, `└` results.
- **Generic** (any other agent or a shell): chrome cleaning; shell prompts
  (`user@host:dir$`, zsh `%`, bare `$`, oh-my-zsh `➜`, starship `❯`,
  PowerShell) split command turns, and the output until the next prompt stays
  as one block with its SGR colours rendered as escaped spans; an idle prompt
  is not a turn; `> ` / `› ` lines are the person's prompts to an agent; other
  text is shaped (herdr-expose `shapeTranscript`: blank runs collapse, the
  pane's hard and soft wraps are undone, long rules become rules, code-shaped
  runs become fenced blocks, bullets become lists) into markdown.
- All three turn box-drawn tables (┌┬┐ │ ├┼┤ └┴┘, wrapped cells included) into
  markdown tables, so they render as real tables.
- Tool results read from a screen have no exit status; one that starts with
  "Error" or says FAILED/`error[`/`panicked at` marks the call failed.
- Provenance: behaviour ported from herdr-expose (MIT, same author)
  `internal/core/clean.go`, `web/src/components/transcriptText.ts` and
  `web/src/ansi/ansiToHtml.ts`; re-implemented in C#, no code copied. Changes
  from the original: prose bullets ("- retries: 3") are not diff lines, and
  `@@ … @@` hunk headers are code.

### G. File links and the file viewer (2026-11-07, protocol v5)
- **Links.** Paths in assistant text, tool rows and command output (absolute,
  `~/`, relative to the pane's cwd, with `:line[:col]`) become links to
  `/m/{machine}/f?pane=&path=#L<line>`. A relative path needs a known extension
  or two separators ("and/or" is not a path). Text inside code blocks and
  existing links is left alone. Plain click opens the viewer in the same tab
  (the chat's scroll is saved, so Back returns to the same spot);
  Cmd/Ctrl/middle click opens a tab. The link works for anyone who can see the
  machine.
- **Fetch.** `GET /api/files/{machine}?pane=&path=` (metadata),
  `/raw` (bytes; `&download=true` as an attachment) and `/markdown` (the
  preview, rendered by the chat's Markdig pipeline: raw HTML off, safe links
  only, no images). The hub needs a signed-in person with CanSee, refuses
  credential-shaped paths (the `klark offer` list) and paths outside the
  pane's cwd and its home, then sends hub → machine
  `get_file {id, pane_id, path, max_bytes}` (link protocol **5**; older
  machines get "update klark"). The machine checks again on its own
  (`crates/klark-link/src/files.rs`): resolves `~/` and relative paths,
  canonicalises (so a symlink is judged by its target), only under the pane's
  cwd or `$HOME`, never credential-shaped, regular files only, and refuses
  above the limit **before reading**, saying "file is X MB; limit 10 MB".
  The limit is `Hub:Files:MaxBytes` (default 10485760) on the hub, and 10 MB
  on the machine (`KLARK_FILES_MAX_BYTES` may lower it). Every fetch and every
  refusal is logged with the person's name. The hub keeps fetched bytes for
  2 minutes (64 MB cap) so metadata, raw and preview cost one machine trip.
- **Responses** are `nosniff`, with `Content-Security-Policy: default-src
  'none'; sandbox` (a PDF gets `object-src 'self'` without sandbox so the
  browser's viewer can load), so a file can never run in the hub's origin.
- **Viewer** (`wwwroot/viewer.html`, `viewer.js`, `viewer.css`,
  `viewer-hl.js`): a static page, no Blazor circuit. GitHub-style header
  (path breadcrumb, size, line count, language, Raw, Copy, Download), code
  with line numbers, click a number for a `#L42` link, `path:line` scrolls to
  and marks that line, word-wrap toggle, find in file. Lines are virtualized
  (fixed 20 px rows, only the visible window plus 40 rows each side in the
  DOM); highlighting runs in a module worker that loads the highlight.js core
  and only that file's language (all 192 grammars are bundled, one file
  each, in `wwwroot/lib/hljs-11.11.1/`); files over 2 MB stay plain. Language
  from extension, file name (Dockerfile, Makefile, Justfile, .gitignore…) or
  shebang, else highlightAuto over a common set. Markdown: Preview/Code.
  CSV/TSV: a table (first 5000 rows). JSON: a collapsible tree built lazily.
  Images (SVG only as `<img>`, never inline): inline with click-to-zoom. PDF:
  the browser's viewer. Diff/patch: red/green rows. Binary: "Binary file,
  X KB" and Download.
- **Lazy loading.** No viewer, highlight.js or xterm code loads with the chat
  or the list: xterm loads on the first terminal open, highlight.js core plus
  one grammar per language on the first code block in that language. See
  E-12 for the numbers.

## Consequences
- The image stays one process with no third-party server and no AGPL
  component. Assets share the /data volume with hub.db; the retention sweep
  bounds them (at most 24 h of uploads by default).
- One hub node only: the file store is local. A multi-node hub would need a
  shared `IAssetStore` (for example S3), which this ADR does not add.
- Kitty graphics drawn inside the terminal (`ServerMessage::Graphics`,
  dropped today) can later flow through the same asset store. That is not
  part of this ADR.
