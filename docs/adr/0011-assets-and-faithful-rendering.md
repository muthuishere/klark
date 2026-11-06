# ADR 0011: assets in an embedded MinIO, faithful pane rendering, transcript view

- Status: accepted (owner, 2026-11-06). Link protocol **v3**.
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

### A. One container: hub + MinIO
- The hub image also holds a pinned MinIO server binary. The entrypoint
  starts MinIO on **127.0.0.1:9000 only**, with data in `/data/minio`, and
  then starts the hub. If either process exits, the container exits.
- MinIO root credentials come from `KLARK_HUB__ASSETS__ACCESSKEY` /
  `__SECRETKEY`. If those are unset, the entrypoint generates them once into
  `/data/minio.env` (mode 0600). They are never logged.
- The hub talks S3 to it: bucket `klark-assets`, created at startup with a
  lifecycle rule that expires objects after `Hub:Assets:RetentionHours`
  (default 24).
- `Hub:Assets:Endpoint` can point at any external S3/MinIO instead, in which
  case the embedded one is not started (`KLARK_HUB__ASSETS__EMBEDDED=false`).
- Nothing outside the container reaches MinIO. Every byte passes through the
  hub, which enforces auth, caps and types. Only one port is published (8080).

### B. Assets
- Object key: `sha256/<hex>`. An identical upload is stored once (the hub
  checks with HEAD before writing).
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
be able to see the asset's machine. The hub streams the object from MinIO.

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

## Consequences
- One image now runs two processes. MinIO is AGPL-3.0, but it is used
  unmodified as a separate process, so the hub's own license is not affected.
- MinIO stopped publishing community binaries and images in 2025. The
  Dockerfile pins the last downloadable release, or builds one from source in
  a build stage, so the image does not depend on a moving tag.
- Kitty graphics drawn inside the terminal (`ServerMessage::Graphics`,
  dropped today) can later flow through the same asset store. That is not
  part of this ADR.
