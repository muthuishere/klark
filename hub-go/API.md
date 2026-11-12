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
| `HUB_DATA_DIR` | `hub.db` (SQLite, sqlite mode) and `session.key`, default `data` |
| `HUB_MACHINES` | `name=<sha256 of token>[,fleet*=<sha256>]` machines accepted without admin UI |
| `HUB_USERS` | `alice=<bcrypt or sha256:hex>[:admin],bob=...` |
| `HUB_ADMIN_PASSWORD` | the built-in `admin` account's password (default `adminadmin`) |
| `HUB_SESSION_KEY` | cookie signing secret (else generated into the data dir). **Required with `HUB_DB=postgres`** (the same on every pod); the hub refuses to start without it |
| `HUB_OIDC_ISSUER` `_CLIENT_ID` `_CLIENT_SECRET` `_SCOPES` `_ADMIN_GROUP` `_NAME` | ADFS / OIDC (code + PKCE; ID token checked against the jwks, RS256/384/512) |
| `HUB_AUTH_MODE` | sign-in directory: `local`, `ldap` or `mockldap`. Empty = `ldap` when `HUB_LDAP_HOST` is set, else `local`. Sign-in is ONE built-in local admin (plus `HUB_USERS` and admin-created accounts) and a configurable LDAP; the mock LDAP is opt-in only (`HUB_AUTH_MODE=mockldap`) |
| `HUB_MOCK_USERS` | only with `HUB_AUTH_MODE=mockldap`: `name=password[:admin][:group1\|group2],...` seeds the mock LDAP (added when missing on each start; replaces the default `user1/user1pass`, `user2/user2pass`, which are seeded once). Passwords cannot contain `,` or `:` |
| `HUB_LDAP_HOST` `_PORT` `_SSL` `_STARTTLS` `_SKIP_CERT_CHECK` `_TIMEOUT_SECONDS` | real LDAP connection (port default 389, 636 with SSL; timeout default 10) |
| `HUB_LDAP_BIND_DN` `_BIND_PASSWORD` | service account for user/group searches (else anonymous). The password is never returned by any API |
| `HUB_LDAP_USER_DN_TEMPLATE` | direct bind, `{0}` = user name, e.g. `uid={0},ou=people,dc=example,dc=com` or `{0}@corp.example.com` |
| `HUB_LDAP_USER_SEARCH_BASE` `_USER_FILTER` | search-then-bind (wins over the template); filter default `(uid={0})`, must match exactly one entry |
| `HUB_LDAP_GROUP_SEARCH_BASE` `_GROUP_FILTER` `_USE_MEMBEROF` | groups: filter `{0}` = the user's DN, `{1}` = user name (default `(\|(member={0})(uniqueMember={0})(memberUid={1}))`); and/or the user's `memberOf` |
| `HUB_LDAP_DISPLAY_NAME_ATTR` | display name attribute, then `displayName`, then `cn` |
| `HUB_LDAP_ADMIN_GROUP` `_ALLOWED_GROUPS` | members of the admin group are admins; allowed groups (comma list, empty = everyone who binds) gate sign-in |
| `HUB_OIDC_PROVIDERS` | JSON array of more OAuth/OIDC providers: `[{"id","name","issuer","clientId","clientSecret","scopes","adminGroup","groupsClaim","usernameClaim"}]`. `id` is `[a-z0-9_-]{1,32}`, not `adfs`; `clientSecret` `"env:NAME"` reads that variable; defaults: name = id, scopes `openid profile email`; `usernameClaim` default `preferred_username`/`upn`/`unique_name`/`email`/`sub`; `groupsClaim` default `groups`/`roles`/`group`. Invalid JSON stops the start |
| `HUB_STATS_INTERVAL_SECONDS` | analytics: the least time between two `system.stats` calls to one machine (default 30, min 5) |
| `HUB_STATS_RETENTION_DAYS` | analytics history kept in `hub.db` (default 14; purged hourly) |
| `HUB_BACKUP` | the ONE storage target: `none` (default; the container image sets `folder`), `folder` or `s3`. Used for the SQLite backups (sqlite mode only) and the analytics files (both modes) |
| `HUB_BACKUP_PATH` | the parent: a local folder for `folder` (image: `/data/backup`), a key prefix for `s3` (e.g. `klarkhub/prod`) |
| `HUB_BACKUP_S3_BUCKET` `_REGION` `_ENDPOINT` `_ACCESS_KEY_ID` `_SECRET_ACCESS_KEY` `_PATH_STYLE` | one set of S3 credentials: region default `us-east-1`; `_ENDPOINT` empty = AWS, else an S3-compatible server (Cloudflare R2, Garage, SeaweedFS: `http://host:8333`); `_PATH_STYLE=true` for `endpoint/bucket/key` URLs. The secret comes from the environment or the admin page and is never returned |
| `HUB_SQLITE_BACKUP_EVERY` | a Go duration, default `6h`, minimum `5m` (e.g. `30m`, `1h`, `24h`): how often a SQLite snapshot is uploaded |
| `HUB_SQLITE_BACKUP_KEEP` | snapshots kept, default 7 |
| `HUB_DB` | `sqlite` (default) or `postgres` |
| `HUB_DATABASE_URL` | postgres URL (`postgres://user:pw@host:5432/db?sslmode=...`); environment only, never logged or returned |
| `HUB_POD_ID` `HUB_POD_URL` `HUB_POD_SECRET` | several pods (postgres): this pod's id (default hostname + random), the address the other pods use to reach it (`http://10.0.0.5:7470`; no base path), and the shared secret (>= 16 chars) of pod-to-pod requests |
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
`/m/:machine/f?path=&pane=`, `/go?machine=&pane=`, `/connect?code=`, `/messages`, `/admin/users`, `/admin/machines`, `/admin/directory`, `/admin/analytics`, `/admin/storage` (UI routes; the API is below).

## Machine link (ADR 0013 protocol v4/5, unchanged)

`GET /agent`, `POST /agent/session`, `GET /agent/events`, `POST /agent/push`, `GET /healthz`.

## Auth

Session = HttpOnly, SameSite=Lax cookie `klark_session` (HMAC-signed, 12 h; Secure behind https). JSON everywhere under
`/api`; errors are `{"error":"message"}`. State-changing requests (POST/PUT/DELETE) are refused with 403 when their `Origin`
is not the hub's own host (or `Sec-Fetch-Site: cross-site`). Login failures are rate limited (5 per user / 15 min and 20
per IP; then `429` + `Retry-After`).

| request | reply |
|---|---|
| `GET /api/auth/providers` | `{"static":true,"directory":{"mode":"local","name":"Local"},"adfs":{"enabled":true,"name":"ADFS","loginUrl":"auth/adfs/login"},"oidc":[{"id":"google","name":"Google","loginUrl":"auth/oidc/google/login"}]}`. `directory.mode` is `local` (name `Local`: the default; no directory, only the built-in admin and `HUB_USERS`/admin-created accounts), `ldap` (`LDAP`) or `mockldap` (`Mock LDAP`, opt-in only); `adfs:{enabled:false}` when off; `oidc` is `[]` when none. The login page shows the username/password form always, and a button per `adfs`/`oidc` entry (links are relative to the base tag) |
| `POST /api/auth/login` `{username,password}` | `200 {username,displayName,admin}` + cookie, `401` (wrong name/password, empty password, or not in an allowed group), `429`, or `503 {"error"}` when the LDAP server cannot be reached (not counted as a failed attempt) |
| `POST /api/auth/logout` | `204` |
| `GET /api/auth/me` | `200 {username,displayName,admin}` or `401` |
| `GET auth/adfs/login[?return=/path]` | 302 to the IdP (state, nonce, PKCE S256) |
| `GET auth/oidc/{id}/login[?return=/path]`, `GET auth/oidc/{id}/callback` | the same flow for an `HUB_OIDC_PROVIDERS` entry (`404` for an unknown id); a state minted for one provider is refused by another; failure: 302 to `login?error=oidc_failed&provider={id}` (or `oidc_unavailable&provider={id}`) |
| `GET auth/adfs/callback` | verifies state, exchanges the code, verifies the ID token, sets the cookie, 302 to `{base}/` (or `return`); on failure 302 to `login?error=adfs_failed` (or `adfs_unavailable`) |

Sign-in order on `POST /api/auth/login` (default mode `local`: only the built-in admin and `HUB_USERS`/admin-created accounts sign in; `ldap` adds the LDAP server; `mockldap` is for demos and tests): the built-in `admin` and accounts in the users table with source `builtin`/`env`/`local` first (a wrong password there is final); otherwise the directory: `mockldap` checks the users table rows with source `mockldap` (bcrypt hashes); `ldap` asks the LDAP server (a bind as the user, so the hub never sees it stored); `local` has no directory. Mock rows are ignored while the mode is not `mockldap`. `allowedGroups` also gates mock users (their `groups`). The same rate limiter (per user and per IP) covers every path. Empty passwords are refused before any LDAP call. LDAP users are not stored: the session carries `username` (as typed), `displayName` and `admin` (member of the admin group), and lasts 12 h.

ADFS and `oidc` users are not stored; their session carries the name and `admin` (member of `HUB_OIDC_ADMIN_GROUP`; for the mock: group `klark-admins`).

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
| `POST .../panes/{pane}/upload` (multipart `file`, optional field `caption`, `?prompt=1`) | `200 {name,path,sha256,size,mime}`: the image (png, jpeg, gif, webp, heic, avif, decided from its bytes; at most 10 MB) is stored by SHA-256 in the database, the machine is sent `fetch_asset {sha256,name,mime,size,pane_id,caption,prompt}` and `path` is where it wrote the file. By default the machine only stores it (the composer then sends `[image from hub] <path> <text>`); `prompt=1` makes the machine prompt the pane. `400` no file, `413` over 10 MB, `415` not an image, `503` offline, `409` protocol < 3, `504` not fetched within 60 s |
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

## Assets

`GET /api/assets/{sha256}`: with `Authorization: Bearer <machine token>` only an asset addressed to that machine (what
`fetch_asset` downloads); otherwise a signed-in cookie. `401` no credentials or unknown token, `403` not yours or past the
24 h retention. Sent with `nosniff`, `Content-Security-Policy: default-src 'none'; sandbox`, `Content-Disposition: inline`.
Records older than 24 h, and then bytes no record names, are swept every 10 minutes.

## Machine approval (`klark hub connect`, ADR 0012)

| request | reply |
|---|---|
| `POST /api/connect/start` `{name}` (no sign-in) | `{code:"ABCD-EFGH",poll_id,url:"{public base}/connect?code=..",expires_in:600,interval:2}`; `400` bad name (`[A-Za-z0-9._-]{1,64}`), `429` + `Retry-After` past 20 starts per address per 10 min, `503` over 1000 pending |
| `POST /api/connect/poll` `{poll_id}` (no sign-in) | `{status:"pending"\|"denied"\|"expired"}` or, once, `{status:"approved",token,machine}`; `404 {status:"unknown"}` afterwards; `429` past 120 polls per address per minute |
| `GET /api/connect/requests/{code}` (cookie) | `{code,machine,status,expiresAt,existing,owner?}`; `404` unknown or expired |
| `POST /api/connect/requests/{code}/approve` \| `/deny` (cookie) | `200 {status,machine}`; `404` unknown/expired, `409` already decided or a `HUB_MACHINES`/fleet name, `403` re-keying a machine someone else approved (admins may) |

Anyone signed in may approve and is recorded as `approvedBy`. Only the SHA-256 of the poll id is stored; the token
(`km_` + 64 hex) is minted when the approved request is collected and only its SHA-256 is kept (`machines` table, listed
in `/api/admin/machines`, revoked with `DELETE /api/admin/machines/{name}`). `HUB_MACHINES` tokens work as before.
Browser route: `/connect?code=` (sign-in required, returns there after login).

## Files (viewer)

`GET /api/files/{machine}?pane=&path=` -> `{path,name,view,lang,mime,size,size_label,lines,raw,download}` where `view` is
code | markdown | csv | tsv | json | diff | image | pdf | binary and `raw`/`download` are URLs relative to the base tag.
`GET /api/files/{machine}/raw?...` the bytes (text as `text/plain`, sandboxed by CSP; `&download=1` forces a download).
`GET /api/files/{machine}/markdown?...` -> `{html}`. Refused before the machine is asked: credential-shaped names (`.env`, keys,
`.ssh/`...), `..` escapes, absolute paths outside the pane's folder and its home (`403`); files over 10 MB (`413`); machines below protocol 5 (`409`).

## Admin (admin cookie only)

| request | reply |
|---|---|
| `GET /api/admin/users` | `[{username,displayName,admin,source,created,groups}]` (`source`: builtin, env, local, mockldap; `groups` is `[]` unless a mockldap user has some) |
| `POST /api/admin/users` `{username,displayName?,password?,admin,source?,groups?}` | creates or updates (password >= 8 chars, required for a new user). `source` (new users only): `local` (default) or `mockldap` (a user of the mock directory; it can sign in only while the mode is `mockldap`); an update keeps the source. `groups` (string array, names without `\|`) replaces a user's groups; absent keeps them. `400` for a bad source/group |
| `DELETE /api/admin/users/{username}` | `204`; not yourself, and not `env`/`builtin` accounts (`409`); `local` and `mockldap` accounts can be deleted. Deleted default mock users are not re-seeded |
| `GET /api/admin/machines` | `[{name,status,approvedBy,created?,source}]` (`status` online/offline; `source` env/admin) |
| `POST /api/admin/machines` `{name,fleet?}` | `201 {name,token}`: the token is shown once, only its SHA-256 is stored |
| `DELETE /api/admin/machines/{name}` | `204`: removes a stored token and drops the session at once (an `HUB_MACHINES` machine only loses its session) |
| `GET /api/admin/directory` | the sign-in setup in force (below) |
| `PUT /api/admin/directory` `{settings fields..., bindPassword?, clearBindPassword?}` | validates and stores the settings (they replace the environment's as a whole, `HUB_LDAP_BIND_PASSWORD` excepted: see below); `200` with the GET body, `400 {error}` when invalid |
| `DELETE /api/admin/directory` | drops the stored settings so the environment applies again; `200` with the GET body |
| `GET /api/admin/analytics[?range=1h\|6h\|24h\|7d]` | machine analytics (below) |
| `GET /api/admin/storage`, `PUT`, `DELETE`, `POST /api/admin/storage/test` | the storage target: SQLite backups and analytics files (below). Replaces the former `/api/admin/archive` |
| `POST /api/admin/directory/test` `{settings?, username, password}` | `200 {ok, message}`; nothing is stored. With `settings` (the PUT shape; mode is taken as ldap; a missing `bindPassword` uses the current one) it tests those, else the current ones (when the current mode is not `ldap` and no settings are sent: `ok:false` with an explanation). With a `username`: a full sign-in check (bind, display name, groups, allowed groups, admin). Without: connect and bind the service account only |

### Directory settings and the GET body

```
{
 "mode": "local|ldap|mockldap", "name": "Local|LDAP|Mock LDAP",
 "source": "env|admin",                      // admin = stored settings are in force
 "settings": { "mode","host","port","ssl","startTls","skipCertCheck","timeoutSeconds",
               "bindDn","bindPasswordSet":bool,"userDnTemplate","userSearchBase","userFilter",
               "groupSearchBase","groupFilter","useMemberOf","displayNameAttr","adminGroup","allowedGroups":[..] },
 "adfs": {"enabled":bool,"name","loginUrl","redirectUri"},   // name/loginUrl/redirectUri only when enabled
 "oidc": [{"id","name","issuer","clientId","clientSecretSet":bool,"scopes","adminGroup","groupsClaim","usernameClaim",
           "loginUrl":"auth/oidc/{id}/login","redirectUri":"https://host/base/auth/oidc/{id}/callback"}]
}
```

- `settings` is the effective set (resolved mode, i.e. never empty). A PUT body is the same object (send `settings` back, change fields),
  flat, plus `bindPassword` (string: replaces the secret; absent or empty keeps the current one) and `clearBindPassword:true` (removes the
  stored secret; `HUB_LDAP_BIND_PASSWORD` still applies when the bind DN is unchanged). The secret is never returned by any API, only `bindPasswordSet`.
- `mode` in a PUT may be empty (then `ldap` if `host` is set, else `local`); switching to `mockldap` seeds the default mock users when the
  directory never had them. `port` 0 = 389 (636 with `ssl`); `timeoutSeconds` 0 = 10; `userFilter` empty = `(uid={0})`; `userDnTemplate`/`userFilter` must contain `{0}`;
  `ssl` and `startTls` are exclusive. LDAP needs `host` and `userDnTemplate` or `userSearchBase`. Group names match case-insensitively on the cn or the full DN.
- `redirectUri` is `publicUrl + base path + path`: register it at the identity provider. The OIDC provider list itself comes from `HUB_OIDC_PROVIDERS` (read only here).
- Every admin route needs the admin cookie and the same-origin check as the other state-changing routes (`403` otherwise).

## Analytics (admin only; collected lazily)

**No traffic to idle machines.** The hub never polls machines on a timer. It asks a machine for `system.stats` (an ordinary
machine `call`, no new link message; `system.stats` is in the hub's call allow-list) only
(a) when an admin request to `GET /api/admin/analytics` arrives, for each online machine whose last attempt is older than
`HUB_STATS_INTERVAL_SECONDS` (so any number of open pages makes at most one call per machine per interval; calls run
concurrently, at most 4 at a time, 5 s timeout each; the request waits at most 2 s and answers with what is stored, a late
answer is stored for the next request), and (b) once when a machine links (hello). A machine that disconnects cannot be
asked: its last stored sample stays as the last known state (`latest`). **Stored history therefore only grows while an admin
looks (the UI page should call the endpoint every 30 s while visible) plus one point per machine connect.** A machine that
answers `system.stats` with an error (an old klark) or something that is not stats is remembered as having no stats
(`hasStats:false`) and asked again after 10 minutes. A timeout only misses that sample.

Samples are rows of `machine_stats` in `hub.db`, kept `HUB_STATS_RETENTION_DAYS`.

`GET /api/admin/analytics?range=1h|6h|24h|7d` (default `24h`; other values `400`):
```
{"range","intervalSeconds":30,"bucketSeconds":720,
 "totals":{"machines","online","panes","agents","klarkRssBytes"},   // panes/agents/rss: sum of the online machines' latest
 "machines":[{"id","name","online","hasStats",
   "latest":{"ts","klarkRssBytes","klarkPeakRssBytes","klarkCpuPercent","uptimeSeconds","version","hostMemTotalBytes",
             "hostMemAvailableBytes","load1","cpus","panes","agents","agentList":[{"paneId","name","state","rssBytes","cpuPercent"}]} | null,
   "peak":{"klarkRssBytes","klarkCpuPercent","hostMemUsedPercent","panes","agents"},   // max over the range
   "series":[{"t":<ms>,"klarkRssBytes","klarkCpuPercent","hostMemAvailableBytes","panes","agents"}]}]}
```
`intervalSeconds` is the minimum sampling interval; `bucketSeconds` is the width of one series point (0 when the series are
raw samples). `latest` is the newest sample ever stored (even outside the range or while offline); `null` and `hasStats:false`
when none. Series have at most 120 points: with more samples they are folded into buckets taking the max of rss, cpu, panes and
agents and the lowest available memory. Missing fields in a sample are 0. Machines are the ones the hub knows plus any with
history in the range.

### Storage: SQLite backups and the analytics files

ONE target (`HUB_BACKUP*`, or saved on the admin page) holds both, under a parent (`HUB_BACKUP_PATH`: a folder, or an S3 key prefix):

```
<parent>/sqlite/hub-<UTC yyyymmddThhmmssZ>.db.gz        SQLite backups (sqlite mode only)
<parent>/sqlite/owner.json                               owner marker: which hub backs up here
<parent>/analytics/YYYY/MM/DD/HH-<hubid>.ndjson.gz       analytics, one gzip NDJSON file per UTC hour (both modes)
```

**Analytics files**: one flat JSON object per sample: `ts_ms, ts, hub, machine, machine_id, klark_rss_bytes, klark_peak_rss_bytes,
klark_cpu_percent, klark_uptime_s, klark_version, host_mem_total_bytes, host_mem_available_bytes, host_load1, host_cpus, panes, agents,
agent_list`. Re-writing an hour overwrites the same object (idempotent). It only writes what exists (no timers call machines): 30 s
after start and then every 10 min it refreshes the current hour and writes finished hours it has not written (up to 48 h back after a
gap or first enable). With several pods every pod writes the same hour with the same `<hubid>` (kept in the database): same bytes,
same object.

**SQLite backups** (only in sqlite mode): a consistent snapshot (`VACUUM INTO`, gzip) is uploaded every `HUB_SQLITE_BACKUP_EVERY`
(default 6h; also once at start), skipped when the content did not change since the last upload, plus one on clean shutdown; the newest
`HUB_SQLITE_BACKUP_KEEP` (7) are kept. **Data-loss window after a crash or a lost disk = up to the interval** (a clean stop loses
nothing). On start, a missing hub.db, or one whose modification time is older than the newest snapshot, is replaced by that snapshot
(the old file is kept as `hub.db.before-restore-<time>`); restore always uses the environment's target (admin-saved settings live in
the database, which does not exist yet). One hub per location: the owner marker (renewed every 30 s, expires after 90 s) makes a second
hub on the same location wait up to 100 s and then refuse to start with a clear message; a clean stop removes it. Not applied with
`HUB_DB=postgres` (back up PostgreSQL with its own tools).

| request | reply |
|---|---|
| `GET /api/admin/storage` | `{"kind":"none|folder|s3","source":"env|admin","path","s3":{"bucket","region","endpoint","accessKeyId","secretAccessKeySet":bool,"pathStyle"},"sqlite":{"enabled":bool,"every":"6h0m0s","keep":7,"owner":"<pod id>","backups":<uploads since start>},"lastSqliteBackupMs","lastAnalyticsWriteMs","lastError","filesWritten"}` (`*Ms` 0 = never; `filesWritten` counts analytics files since start; `lastError` is the newest problem of either job, empty when the last attempt worked; `sqlite.owner`/`backups` only in sqlite mode; the secret is never returned) |
| `PUT /api/admin/storage` | body = `{"kind","path","s3":{"bucket","region","endpoint","accessKeyId","secretAccessKey"?,"pathStyle"},"clearSecretAccessKey"?}`. `secretAccessKey` replaces the secret; absent keeps the current one while `accessKeyId` is unchanged. Validates (`400 {error}`), creates a folder, stores the settings (they replace the environment's as a whole; `every`/`keep` stay environment-only), answers with the GET body |
| `DELETE /api/admin/storage` | drops the stored settings; the environment applies again; GET body |
| `POST /api/admin/storage/test` | optional body like PUT (tests those unsaved settings), else the current ones. Writes and deletes a marker object: `200 {ok,message}` (`ok:false` with the reason, never the secret) |

The folder kind never writes outside its folder. The S3 client is a small built-in SigV4 implementation (path-style or virtual-host,
any endpoint; PUT, GET, DELETE, ListObjectsV2).

## Several pods (HUB_DB=postgres)

`HUB_DB=postgres` + `HUB_DATABASE_URL` puts users, machines, messages, settings and analytics in PostgreSQL (pure-Go `pgx`; migrations
under `internal/store/migrations/postgres`, applied under an advisory lock). Several pods may then run behind a load balancer. What is
and is not shared:

- **Shared (database)**: accounts, machine tokens, messages, settings, analytics history, and `machine_presence`.
- **Per pod**: the machine links and the pane streams. `machine_presence(machine_id, pod_id, pod_url, session_id, name, version, protocol,
  heartbeat_ms)` says which pod holds a machine (heartbeat every 5 s, expires after 15 s, so a dead pod's machines reconnect to any pod).
  Lookups read the table on demand (LISTEN/NOTIFY is not used).
- **Forwarding**: a browser request for a machine held by another pod (`/api/machines/{id}/...` including the SSE streams, `/api/files/{id}...`,
  `POST /api/messages` to such a machine) is reverse-proxied to that pod's `HUB_POD_URL` with header `X-Klark-Pod-Secret`. The pod that
  received the request has already checked the cookie and the cross-origin rule; the target re-checks the cookie (same `HUB_SESSION_KEY`).
  A machine's own link requests (`/agent/events`, `/agent/push`) that land on another pod are proxied by session id the same way. The
  header must equal `HUB_POD_SECRET` (constant-time compare): any other value gets `403`, and `/internal/*` (`GET /internal/agents`,
  `POST /internal/stats/refresh`, `POST /internal/revoke?machine=`) answers only with it. Without `HUB_POD_URL` postgres runs as a single pod.
- `GET /api/machines`, `/api/agents`, admin machines and analytics merge the other pods' machines; revoking a machine also tells the other pods.
- **Analytics sampling stays lazy and local**: the pod that holds a machine is the only one that calls it; an analytics request on any pod
  asks the other pods to refresh their own machines.
- **Sign-in**: the cookie works on every pod (shared `HUB_SESSION_KEY`, required). The login rate limiter is per pod (a client can try
  `limit x pods` times).
- Each pod runs its own archive writer (same bytes, same objects) and purge; no SQLite backup in this mode.
