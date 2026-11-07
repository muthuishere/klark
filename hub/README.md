# klark hub

A Blazor Server app that many people sign in to in order to watch and answer
the coding agents on many klark machines from one place.

```
klark machine ──(outbound http(s), machine token)──▶ klark hub ◀──(browser, sign-in)── people
```

- **Machines dial out.** `klark expose` holds one HTTP request open to
  `/agent/events` (NDJSON, hub → machine) and POSTs short segments to
  `/agent/push` (machine → hub); no WebSocket anywhere (ADR 0013). Nothing
  listens on the machine and no tunnel is needed.
- **Always pull.** A machine never sends anything unasked. The hub pulls pane
  trees only while someone has the machines page open, and gets "unchanged"
  when nothing changed. A pane's output flows only while at least one person
  has that pane open: the first viewer starts it and the last one to leave
  stops it.
- **Shared.** Every signed-in user sees every linked machine. Panes whose agent
  is blocked are listed first.
- **Bounded.** From the hub, a machine accepts only reads (`pane.read`,
  `agent.read`, `session.snapshot`, …), text input and keys. It never accepts
  pane-closing or other state-changing calls.

## Run

```bash
cd hub
dotnet run -- hash-password 'a-password'     # -> PasswordHash for a user
dotnet run -- hash-token "$(openssl rand -hex 32)"   # -> TokenSha256 for a machine (keep the token)
cp appsettings.Development.example.json appsettings.Development.json   # fill in the hashes
dotnet run
```

Or with Docker: put your LDAP settings and machine hashes in `hub/appsettings.Production.json` (or `KLARK_HUB__...` variables), then run `cd hub && docker compose up -d --build`.
The hub is plain HTTP on port 8080; machines connect with `http://` (no
WebSockets, for machines or browsers: ADR 0013). Run it on a trusted network or
VPN. Tokens and passwords cross the wire unencrypted, so for public exposure put
a TLS proxy in front and use `https://` (the link supports both).

## Sign-in

`Hub:Auth:Mode` (environment: `KLARK_HUB__AUTH__MODE`):

- `Ldap` (default): your LDAP or Active Directory checks every password by a
  bind as the user; the hub stores no passwords. Set `Hub:Auth:Ldap`:
  - direct bind: `UserDnTemplate`, e.g. `uid={0},ou=people,dc=example,dc=com`
    or the AD form `{0}@corp.example.com`;
  - search-then-bind: `UserSearchBase`, `UserFilter`, `BindDn` / `BindPassword`
    (give the password through the environment);
  - `UseSsl` (LDAPS, port 636) or `StartTls`; `SkipCertificateCheck` for test servers only;
  - groups from `GroupSearchBase` and/or `memberOf` (`UseMemberOf`).
  To try it without a directory of your own:
  `docker compose --profile ldap up -d --build` signs in alice/alice or bob/bob against a test OpenLDAP.
- `MockLdap`: users kept in hub.db (hashed), managed by the admin at
  `/admin/users`. First start seeds `user1/user1pass` and `user2/user2pass`
  (or `Hub:Auth:MockUsers` if set). This is the mode of a hub with no
  `Hub:Auth:Mode` and no `Hub:Auth:Ldap:Host`, so a bare `dotnet run` works.
- `Local`: `Hub:Users` with `dotnet run -- hash-password` hashes.

A built-in `admin` account works in every mode (first-start password
`adminadmin`, or `KLARK_HUB__ADMIN__PASSWORD`); change it at `/account/password`.
The admin can switch the mode and edit LDAP settings at `/admin/directory`
without a restart. Deployment: [docs/deploy.md](../docs/deploy.md).

`Hub:Auth:AllowedGroups`, for example `["klark-users"]`, limits who can sign in.

### Environment variables

Every setting can come from the environment with the `KLARK_` prefix and `__`
for each level: `KLARK_HUB__AUTH__MODE`, `KLARK_HUB__AUTH__LDAP__HOST`,
`KLARK_HUB__JWT__KEY`, `KLARK_HUB__MACHINES__0__NAME`,
`KLARK_HUB__MACHINES__0__TOKENSHA256`, `KLARK_HUB__DATADIR`. Command-line
arguments still win over the environment.

### Who sees and drives what

- `Hub:Machines[].Groups`: only members of these groups see that machine.
  Empty means everyone signed in. Fleet members inherit their entry's groups.
- `Hub:Auth:AdminGroups` (default `klark-admins`): its members see every machine.
- `Hub:Auth:ReadOnlyGroups` (default `klark-readonly`): its members can watch
  but cannot type. This is enforced on the server, not only by hiding the
  reply box.
- The mock directory's `viewer/viewer` user is read-only, for trying this out.
- Sign-in is limited to 10 attempts per minute per address.

### Pane page

`/m/<machine>/p/<pane>?view=chat|terminal` (ADR 0011). One full-height
column, phone first; desktops centre it. Phones open on **Chat**, desktops on
**Terminal**, and each browser remembers the last tab per pane. The header is
just back, the agent's name (else agent and folder, never a raw pane id), a
status dot, and a ⋯ menu holding Terminal/Chat, Search and the machine,
session and folder.

- **Chat.** User and agent turns, markdown (tables scroll inside the
  message, code blocks have a language label, copy button and highlighting),
  tool calls and thinking folded into one-line rows, a live
  working/idle/blocked line, and a "Jump to latest" pill once you scroll up.
  A blocked agent's question shows with buttons for its numbered or yes/no
  options. Search (⋯ menu) covers the loaded messages. Data comes from the
  machine's `get_transcript` (Claude Code logs; everything else is the
  cleaned screen) about once a second, only while a chat is open, shared by
  all viewers of the pane; older machines fall back to `agent.read` /
  `pane.read`. Only the newest 150 rows are drawn ("Show earlier" for more).
  A screen is cleaned of the agent's TUI chrome (rules, prompt-box borders,
  context meters, mode banners, key hints, spinner/timing lines, version
  banners) and split into turns: `> `/`❯ `/`› ` is the person, `⏺ Tool(args)`
  a collapsed tool row, `⎿ ` its result. Structured items never show
  system/meta items, `<system-reminder>` or local-command wrappers, or empty
  items; `<command-name>/x</command-name>` shows as `/x`.
- **Terminal.** Drawn at the pane's real size (`pane_size` from the
  machine; 120x40 before one arrives) and scaled to the width, scrolling
  sideways below 55 %. A viewer whose browser falls behind (more than ~2.5 s
  of queued output) gets a fresh screen. xterm 5.5, its Unicode 11 and
  web-links addons and highlight.js 11.11.1 are bundled in `wwwroot/lib`, so
  the hub needs no CDN.
- **Composer.** Enter sends, Shift+Enter is a new line; Esc and Ctrl-C on the
  terminal tab.
- **Back and state.** Back (browser, phone gesture or the header arrow)
  returns to the list at the same scroll position and search, with the chat
  you came from marked. The arrow on a chat opened directly (link, bookmark,
  after sign-in) replaces it with the list, so back never leaves the app or
  bounces. Tab switches replace the history entry. Per chat the browser keeps
  the scroll position (or "at latest"), opened tool rows, the draft, the
  search and its current match, the chosen quick reply and the file panel's
  state, through reloads, reconnects and app switches. Sign-in keeps the
  return URL's query string and replaces its own history entry.
- **Images and files.** See *Assets* below.
- **Offline machines** are listed with their last-seen time. Expanding one
  shows its last recorded pane list, and opening a pane shows the output that
  was recorded while someone watched it.

## Tokens for the CLI and scripts (JWT)

Signed-in users open **CLI token** (`/token`) for a fresh JWT with a copy
button, or let the CLI fetch one: `klark msg login --hub http://<hub>:8080 --user <name>`.
Scripts can ask directly (same directory, same 10-per-minute limit as the login form):

```bash
curl -s -X POST http://hub:8080/api/token -H 'content-type: application/json' \
  -d '{"username":"alice","password":"..."}'
# {"token":"eyJ...","expires_at":"...","user":"alice","groups":["..."]}
```

Tokens are HS256, signed with `Hub:Jwt:Key` (`KLARK_HUB__JWT__KEY`, at least
32 bytes). Without it the hub creates a random key once in `<DataDir>/jwt.key`
(mode 0600), so tokens survive restarts; delete that file to revoke every token.
Lifetime is `Hub:Jwt:Hours` (default 12). Claims: `name`, `display_name`, one `role` per group.

## HTTP API

Every `/api` call except `/api/token` takes `Authorization: Bearer <token>`:
a user JWT (the user's groups decide what they see, as on the pages), or a
machine token (the caller acts as that machine and may reach every machine;
fleet tokens also need `X-Klark-Machine`). No or a bad token gives 401.

| call | answer |
|---|---|
| `POST /api/token` `{username, password}` (JSON or form) | `{token, expires_at, user, groups}`; 401 on a bad password |
| `GET /api/agents` | `[{machine, session, agent, name, pane_id, status, address}]` from every online machine you may see (asked concurrently, 10 s timeout each) |
| `POST /api/messages` `{to, text, from?}` | `200 {id, delivered:true}`, else `{id, error}` with the status below |
| `GET /api/messages?limit=100` | the newest messages (max 500) whose target or source machine you may see |
| `POST /api/panes/{machine}/{pane}/images?name=&caption=` (user JWT, raw image body) | `200 {sha256, size, mime, path}` once the machine fetched it; 403 read-only, 404 offline, 409 machine below v3, 413 too big, 415 not an image |
| `PUT /api/assets?pane=<pane>&name=<name>` (machine token, raw body) | `200 {sha256, size, mime, url}`; 413 over 100 MB |
| `GET /api/assets/{sha256}` (machine token) | the bytes, only for an asset sent to that machine; 403 otherwise |

## Messaging between agents (ADR 0009)

An address is `machine/session/agent`: the machine's name on this hub, a klark
session (`default` by default) and an agent name or pane id. `klark msg` on a
machine sends through the hub when the target is on another machine; the hub
pushes it over the target's link (protocol v2: `list_agents`, `deliver`), and
the agent receives it as a prompt.

`POST /api/messages` answers:

- `400` the address is not `machine/session/agent`, or the text is empty; `413` the text is over 16 KB;
- `403` you may not see the machine or may not type into it (read-only groups);
- `404` the machine is not online on this hub;
- `409` the machine links with protocol 1 (update klark there); it is also left out of `/api/agents`;
- `502` the machine refused (for example, no such agent, or it is blocked); `504` it did not answer within 10 s.

`from` defaults to `user:<name>` for users and the machine name for machine
tokens. Every attempt is stored in `hub.db` (`messages`), with `sender` set to
who really called (`user:<name>` or `machine:<id>`), whatever `from` says.
The **Messages** page (`/messages`) lists them and sends as the signed-in user.

## Tests

```bash
dotnet test hub.Tests     # xUnit: screen cleaning (Claude Code, Codex, Devin fixtures), turns, structured items
```

## Load test

```bash
# hub config: "Machines": [{ "Name": "load", "Fleet": true, "TokenSha256": "<hash-token fleet-secret>" }]
cargo run --release -p klark-link --bin klark-loadtest -- \
  --hub http://localhost:8080 --token fleet-secret --machines 100 --seconds 60
```

Then sign in and open a few panes: only those panes start streaming.

## Assets: images to agents, files from agents (ADR 0011)

All assets are **temporary**: they expire after `Hub:Assets:RetentionHours`
(24 by default) and are then deleted. The hub keeps them in its own
content-addressed file store on the data volume (`assets/sha256/<aa>/<hex>`,
files 0600, directories 0700, written to a temp file, fsynced and renamed).
There is no second process and no object store: the image runs only the hub,
on one port.

- **Person to agent.** On a pane page, paste, drop, pick (＋) or photograph
  (◎, phones) an image. You must be allowed to type there. The hub checks the
  type from the bytes (png, jpeg, gif, webp, heic, avif; 20 MB at most),
  stores it and sends the machine `fetch_asset`; the machine downloads it
  and hands the path to the agent. Scripts can do the same with
  `POST /api/panes/<machine>/<pane>/images?name=x.png&caption=...` and a JWT.
- **Agent to person.** `klark offer <file>` uploads with the machine token
  (100 MB at most). The pane page lists offered files live: images inline,
  everything else as downloads.
- **Serving.** `GET /assets/<sha256>` with the sign-in cookie or a JWT, if you
  may see the asset's machine. Every response sends `nosniff`; HTML, SVG and
  XML go out as `application/octet-stream`, and anything that is not an
  image is an attachment.

| setting | env | default |
|---|---|---|
| `Hub:Assets:RetentionHours` | `KLARK_HUB__ASSETS__RETENTIONHOURS` | 24 |
| `Hub:Assets:MaxImageBytes` | `KLARK_HUB__ASSETS__MAXIMAGEBYTES` | 20971520 |
| `Hub:Assets:MaxFileBytes` | `KLARK_HUB__ASSETS__MAXFILEBYTES` | 104857600 |

## Storage

SQLite, in `Hub:DataDir` (`/data` in Docker):

- `hub.db` lists every machine that has connected, with its host, version and when it was first and last seen, and every agent message.
- `jwt.key` is the generated JWT signing key, when `Hub:Jwt:Key` is not set.
- `assets/` holds temporary images and files (see *Assets*); `hub.db` has their `assets` table.
- `machines/<name>.db` holds one machine's data: its latest pane tree, recent pane output and connect/disconnect events. Each file is capped at `Hub:MaxBytesPerMachine` (10 MB by default). The oldest output is dropped first.

Writes are batched by one background writer, so disk never slows the machine sockets.

## Link a machine

```bash
klark expose --hub http://hub-host:8080 --token <machine token> [--name laptop]
# or KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME
```

The machine shows up on the hub under the name configured for its token. The
link reconnects with backoff when the connection drops.

## Wire protocol

JSON messages `{"type": ...}`. Machines send them gzip-compressed in binary frames, with terminal output merged per pane every ~25 ms. The hub sends plain JSON text. The architecture and status are tracked in `docs/adr/0001-klark-hub-architecture.md`. It is defined in `crates/klark-link/src/protocol.rs`.

| machine → hub | hub → machine |
|---|---|
| `hello` (protocol 1, 2 or 3), `tree` / `tree_unchanged` (only when asked), binary output frames, `closed`, `result`, `pong`; v3: `pane_size {pane_id, cols, rows}` while subscribed | `get_tree`, `subscribe`, `unsubscribe`, `input`, `keys`, `call`, `ping`; v2+: `list_agents`, `deliver`; v3: `fetch_asset {id, sha256, name, mime, size, pane_id, caption}`, `get_transcript {id, pane_id, after?}` |
