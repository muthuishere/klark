# klarkhub (Go + React) contract

One binary, one container, one port (default `7470`). The Go server is the API and also serves the
built React app (embedded). The Blazor hub in `../hub` stays untouched until this one reaches parity.
This file is what the server does today; the tests in `internal/app` pin it.

## Configuration (environment)

| variable | meaning |
|---|---|
| `HUB_ADDR` | listen address, default `:7470` |
| `HUB_BASE_PATH` | mount prefix, e.g. `/klark` (default none) |
| `HUB_PUBLIC_URL` | public origin (`https://host`, no path) used for OIDC redirects; else derived from the request (`X-Forwarded-*`) |
| `HUB_DATA_DIR` | `hub.db` (SQLite) and `session.key`, default `data` |
| `HUB_MACHINES` | `name=<sha256 of token>[,fleet*=<sha256>]` machines accepted without admin UI |
| `HUB_USERS` | `alice=<bcrypt or sha256:hex>[:admin],bob=...` |
| `HUB_ADMIN_PASSWORD` | the built-in `admin` account's password (default `adminadmin`) |
| `HUB_SESSION_KEY` | cookie signing secret (else generated into the data dir) |
| `HUB_OIDC_ISSUER` `_CLIENT_ID` `_CLIENT_SECRET` `_SCOPES` `_ADMIN_GROUP` `_NAME` | ADFS / OIDC (code + PKCE; ID token checked against the jwks, RS256/384/512) |
| `HUB_MOCK_ADFS=1`, `HUB_MOCK_ADFS_USERS="alice=pw:admin,bob=pw"` | dev only: the hub serves an IdP at `/mock-adfs/` and uses it |

`klarkhub healthcheck` (exit 0 when `/healthz` answers), `klarkhub hash-password`, `klarkhub hash-token` (read stdin).

## URLs and the base path

Everything is mounted under `HUB_BASE_PATH` (`/healthz` also answers at the root). `index.html` is served for every
path that is not an API/file path (React Router routes, `/go?machine=&pane=`, ...), with `no-cache`; files under
`assets/` get `Cache-Control: public, max-age=31536000, immutable`. In the built `index.html` the server replaces the
`<base href="...">` tag (or the literal `__HUB_BASE_HREF__`) with `<base href="{HUB_BASE_PATH}/">`. All app URLs are relative
to it. Responses carry `X-Content-Type-Options`, `X-Frame-Options: DENY`, `Referrer-Policy`, and a CSP
(`script-src 'self'` plus the sha256 of the inline theme script found in `index.html`; no other inline script).
Text responses are gzipped; event streams and the machine link never are.

Browser routes (served as `index.html`): `/`, `/login`, `/m/:machine`, `/m/:machine/p/:pane`,
`/m/:machine/f?path=&pane=`, `/go?machine=&pane=`, `/messages`, `/admin/users`, `/admin/machines`, `/admin/directory`.

## Machine link (ADR 0013 protocol v4/5, unchanged)

`GET /agent`, `POST /agent/session`, `GET /agent/events`, `POST /agent/push`, `GET /healthz`.

## Auth

Session = HttpOnly, SameSite=Lax cookie `klark_session` (HMAC-signed, 12 h; Secure behind https). JSON everywhere under
`/api`; errors are `{"error":"message"}`. State-changing requests (POST/PUT/DELETE) are refused with 403 when their `Origin`
is not the hub's own host (or `Sec-Fetch-Site: cross-site`). Login failures are rate limited (5 per user / 15 min and 20
per IP; then `429` + `Retry-After`).

| request | reply |
|---|---|
| `GET /api/auth/providers` | `{"static":true,"adfs":{"enabled":true,"name":"ADFS","loginUrl":"auth/adfs/login"}}` (`adfs:{enabled:false}` when off) |
| `POST /api/auth/login` `{username,password}` | `200 {username,displayName,admin}` + cookie, `401`, or `429` |
| `POST /api/auth/logout` | `204` |
| `GET /api/auth/me` | `200 {username,displayName,admin}` or `401` |
| `GET auth/adfs/login[?return=/path]` | 302 to the IdP (state, nonce, PKCE S256) |
| `GET auth/adfs/callback` | verifies state, exchanges the code, verifies the ID token, sets the cookie, 302 to `{base}/` (or `return`); on failure 302 to `login?error=adfs_failed` (or `adfs_unavailable`) |

ADFS users are not stored; their session carries the name and `admin` (member of `HUB_OIDC_ADMIN_GROUP`; for the mock: group `klark-admins`).

## Data API (all need the cookie; 401 otherwise)

Errors from the machine link: offline `503`, no answer `504`, the machine's own refusal `502`, unknown machine `404`.

| request | reply |
|---|---|
| `GET /api/machines` | `[{id,name,online,version,protocol,lastSeen}]` (configured/stored machines appear offline until they link; `lastSeen` RFC 3339 or null) |
| `GET /api/machines/{id}/tree` | the machine's `session.snapshot` as the body. `ETag: "<rev>"`, `X-Tree-Rev`; `If-None-Match` gives `304`. The hub pulls it with `get_tree{known_rev}` and keeps it on `tree_unchanged` |
| `GET /api/machines/{id}/events` | SSE: `status {online}`, `tree {rev,snapshot}` (only when the revision changes). The hub polls the machine every 2 s only while a stream is open |
| `POST /api/machines/{id}/call` `{method,params?,timeoutMs?}` | `{result}`; only the allow-listed reads (`ping session.snapshot pane.read agent.read pane.get workspace.list tab.list pane.list`), else `403` |
| `GET /api/machines/{id}/panes/{pane}/chat` | `{rows,rev,agent,status,source,reader,error?,blocked?,lastActivity?}` (see rows) |
| `GET /api/machines/{id}/panes/{pane}/events[?frames=1]` | SSE, below |
| `POST .../panes/{pane}/input` `{text,enter?}` | `204`, types text |
| `POST .../panes/{pane}/keys` `{keys:[..]}` | `204` (1-32 keys: `enter`, `esc`, `ctrl+c`...) |
| `POST .../panes/{pane}/send` `{text}` | `204`, the chat composer: adds the hub note (agents name files by absolute path) and presses enter; a slash command to an agent whose log does not record commands is chatted locally with its output read off the screen |
| `POST .../panes/{pane}/upload` (multipart `file`) | **`501`: not implemented yet** (the asset store is not ported) |
| `GET /api/agents` | `[{machine,session,agent,name,pane_id,status,address}]` |
| `GET /api/messages[?limit=]` | `[{ts,from,to,text,ok,error?,sender,id}]` newest first, `ts` RFC 3339 |
| `POST /api/messages` `{to:"machine/session/agent",text,from?}` | `200 {id,ok}`; `400` bad address/empty, `404` machine not linked, `413` over 16 KB, `502/504` delivery failed. Every attempt is recorded |

### Chat rows

`rows: [{id, role, kind, html?, text?, time?, tool?, summary?, input?, ok?, toolUseId?, lines?}]`
- `role` user | assistant | tool; `kind` text | tool_use | tool_result | thinking | command | output.
- `html` is server-rendered and safe: assistant markdown (raw HTML dropped, only http/https/mailto links, images become alt text;
  code blocks are `<pre><code class="language-x">`) or ANSI command output (`<pre class="ansi">`, colours as classes `af0..15`,
  `ab0..15`, `a-b a-d a-i a-u`, or inline `rgb()`); file paths are links `<a class="fpath" href="m/{machine}/f?pane=&path=#L12">`
  (relative to the base tag). `text` is the plain text. `lines` is the line count of `output` rows. `time` is RFC 3339.
- `source` is `jsonl` (the machine's structured transcript; rows are immutable) or `screen` (read off the screen; the last row can grow).
  `reader` is claude | devin | codex | agy | generic for screens. `blocked` is `{question, options:[{label,send,enter}]}` when the agent waits for an answer.

### Pane SSE: `GET .../panes/{pane}/events`

- `chat` `{rows, replace?, rev, agent, status, source, error?, blocked?}`: rows are **upserted by id**. When `replace` is true the
  rows are the whole chat (drop any you hold that are missing); jsonl chats send only new rows after the first event.
- `status {online}`, `tree {rev,snapshot}`: as for the machine stream.
- With `?frames=1` only: `frame {data:<base64>, reset?:true}` (`reset` = clear the terminal and paint this, sent first and after
  a slow client lost output), `size {cols,rows}`, `closed {reason}`. The machine streams the pane only while at least one
  `frames=1` stream is open (first viewer subscribes, last one out unsubscribes; chat-only streams never do).
- A `: ping` comment every 15 s.

## Files (viewer)

`GET /api/files/{machine}?pane=&path=` -> `{path,name,view,lang,mime,size,size_label,lines,raw,download}` where `view` is
code | markdown | csv | tsv | json | diff | image | pdf | binary and `raw`/`download` are URLs relative to the base tag.
`GET /api/files/{machine}/raw?...` the bytes (text as `text/plain`, sandboxed by CSP; `&download=1` forces a download).
`GET /api/files/{machine}/markdown?...` -> `{html}`. Refused before the machine is asked: credential-shaped names (`.env`, keys,
`.ssh/`...), `..` escapes, absolute paths outside the pane's folder and its home (`403`); files over 10 MB (`413`); machines below protocol 5 (`409`).

## Admin (admin cookie only)

| request | reply |
|---|---|
| `GET /api/admin/users` | `[{username,displayName,admin,source,created}]` (`source`: builtin, env, local) |
| `POST /api/admin/users` `{username,displayName?,password?,admin}` | creates or updates (password >= 8 chars, required for a new user) |
| `DELETE /api/admin/users/{username}` | `204`; not yourself, and not `env`/`builtin` accounts (`409`) |
| `GET /api/admin/machines` | `[{name,status,approvedBy,created?,source}]` (`status` online/offline; `source` env/admin) |
| `POST /api/admin/machines` `{name,fleet?}` | `201 {name,token}`: the token is shown once, only its SHA-256 is stored |
| `DELETE /api/admin/machines/{name}` | `204`: removes a stored token and drops the session at once (an `HUB_MACHINES` machine only loses its session) |
| `GET /api/admin/directory`, `PUT /api/admin/directory` | any JSON object, stored and returned whole |
