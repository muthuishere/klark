# klarkhub (Go API + React UI), in progress

The rewrite of the klark hub: a Go API with the React UI embedded, in one binary and one container on one
port (7470). Decision and reasons: [ADR 0014](../docs/adr/0014-go-react-hub.md). The contract:
[API.md](API.md).

**Status: in progress, not released. The released hub is still the C# Blazor one in [`../hub`](../hub)
(v0.9.5.6).** The Go and React parts listed as done below are implemented and were checked by the project
coordinator (2026-11-12); the gaps are listed under "Not done". Do not replace the released hub with this one yet.

| part | state |
|---|---|
| machine link (ADR 0013), machine tokens, fleet tokens | done; tests; a real `klark expose` machine shows online |
| config, `HUB_BASE_PATH` | done |
| SQLite storage (pure Go, no cgo) | done |
| static credentials (bcrypt or `sha256:`), built-in `admin` | done |
| OIDC / ADFS client, mock ADFS (HTTP-level end-to-end test) | done; the mock ADFS is the tested path (a real ADFS or Entra is not needed for this rewrite) |
| machines, session tree, calls, input, keys; per-pane SSE (lazy, by viewer count) | done |
| chat pipeline: Claude, Devin, Codex, Agy and generic screen readers, markdown, ANSI, command output, file links | done |
| file endpoints (10 MB cap, secrets refused), messages, admin users and machines | done |
| React UI: all routes, login, windowed chat, composer with `/` popup, terminal view, file viewer, messages, admin, seven themes | done; 51 UI tests |
| single-container image (25 MB distroless, non-root, healthcheck, base path at `/` and `/klark/`) | built by the backend agent; not rebuilt by the docs agent |
| subcommands `healthcheck`, `hash-password`, `hash-token`, `version`; Dockerfile, compose, Makefile | done |

Checks reported by the coordinator: `go vet ./... && go test -race ./...` passes (one timing flake seen once in
`TestPaneStreamIsLazyAndCountsViewers` under full parallel `-race` load, then 8 of 8 reruns passed);
`npm test` passes 51 tests; the built hub served login, both sign-in providers and the UI in a real browser.

### Not done (stubbed or missing)

- Agent-to-person files (`PUT /api/assets`, `klark offer`) are not ported; only person-to-agent images are.
- `offers` and the `offer` event (`klark offer`).
- Server-side slash-command lists (the UI carries its own list).
- Syntax highlighting; quick-reply buttons for a blocked pane.
- CLI token page and change-password page.
- Per-machine access groups.
- No load or soak test.
- Known UI nit: a short machine name is truncated to "m..." in the sidebar.

## Layout

| path | what |
|---|---|
| `cmd/klarkhub/` | the binary and its subcommands |
| `internal/app`, `auth`, `hub`, `chat`, `store` | HTTP routes, sign-in, machine state, chat pipeline, SQLite |
| `internal/server/` | HTTP handlers for the machine link (+ tests) |
| `internal/link/` | machine connections: registry, per-machine event queue, resume |
| `internal/tokens/` | machine token check (sha256) |
| `internal/config/` | reads every `HUB_*` setting from the environment (+ tests) |
| `internal/ui/` | embeds `dist/` (the built React app) and serves it |
| `ui/` | the React 18 + Vite + TypeScript source (`src/pages`, `src/components`, `src/lib`, `src/mock`) |
| `API.md` | URLs, auth, data API (what the server does today) |
| `Dockerfile`, `docker-compose.yml`, `Makefile` | image, example compose, `make dev|ui|test|build|docker` |

## Environment

Source of truth: `API.md`; code: `internal/config`.

| variable | default | meaning |
|---|---|---|
| `HUB_ADDR` | `:7470` | listen address |
| `HUB_BASE_PATH` | none | mount prefix behind a reverse proxy, e.g. `/klark` |
| `HUB_PUBLIC_URL` | from the request (`X-Forwarded-*`) | public origin, no path, for OIDC redirects |
| `HUB_DATA_DIR` | `data` (`/data` in the image) | `hub.db` (SQLite) and `session.key` |
| `HUB_MACHINES` | empty | `name=<sha256 of token>[,fleet*=<sha256>]`; a name ending in `*` is a fleet token |
| `HUB_USERS` | empty | `alice=<bcrypt or sha256:hex>[:admin],bob=...` |
| `HUB_ADMIN_PASSWORD` | `adminadmin` | the built-in `admin` account's password; set it |
| `HUB_SESSION_KEY` | generated into the data dir | signs the `klark_session` cookie |
| `HUB_OIDC_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET`, `_SCOPES`, `_ADMIN_GROUP`, `_NAME` | scopes `openid profile email`, name `ADFS` | ADFS / OIDC (code + PKCE) |
| `HUB_AUTH_MODE` | `local`, or `ldap` when `HUB_LDAP_HOST` is set | sign-in = the built-in local `admin` (plus `HUB_USERS`) and a configurable LDAP (`HUB_LDAP_*`, see API.md). `mockldap` (demo users `user1/user1pass`, `HUB_MOCK_USERS`) is opt-in only |
| `HUB_STATS_INTERVAL_SECONDS`, `HUB_STATS_RETENTION_DAYS` | 30 (min 5), 14 | analytics: least time between two calls to one machine; days of history kept |
| `HUB_BACKUP` (`none`/`folder`/`s3`), `HUB_BACKUP_PATH`, `HUB_BACKUP_S3_BUCKET`/`_REGION`/`_ENDPOINT`/`_ACCESS_KEY_ID`/`_SECRET_ACCESS_KEY`/`_PATH_STYLE` | `none` (the image: `folder`, `/data/backup`) | the ONE storage target for the SQLite backups and the analytics files; see "Storage and backups" |
| `HUB_SQLITE_BACKUP_EVERY`, `HUB_SQLITE_BACKUP_KEEP` | `6h` (min `5m`), `7` | SQLite snapshot interval and how many to keep |
| `HUB_POD_ID` | hostname + random | names this hub as the owner of the SQLite backup location |
| `HUB_MOCK_ADFS`, `HUB_MOCK_ADFS_USERS` | off | `1` serves a mock IdP at `/mock-adfs/`; users `alice=pw:admin,bob=pw`. Dev and tests only |

## Analytics

The hub has built-in analytics of the machines (admin only, `GET /api/admin/analytics`): the klark process's memory (RSS and
peak), CPU and uptime, host memory and load, panes and agents (with each agent's RSS and CPU), latest value, peak and a
series per machine (at most 120 points).

- **Collected lazily: no traffic to idle machines.** The hub asks a machine for `system.stats` (an ordinary machine call, so
  old machines and hubs keep working; a machine without it shows `hasStats:false` and is retried every 10 min) only while an
  admin has the Analytics page open (each request refreshes machines whose last sample is older than
  `HUB_STATS_INTERVAL_SECONDS`) and once when a machine connects. History therefore only grows while someone looks, plus one
  point per connect. When a machine disconnects its last known sample stays.
- **Storage: SQLite** (`hub.db`, pure Go), table `machine_stats`, kept `HUB_STATS_RETENTION_DAYS` (default 14).
- **Analytics files** (JSON per sample, one gzip NDJSON file per UTC hour) go to the storage target, see below, where DuckDB
  can read them.
- **DuckDB is not linked into the hub.** It needs cgo and would break the small static image. Query the files with DuckDB
  yourself, for example the peak memory per machine:

  ```sql
  INSTALL httpfs; LOAD httpfs;   -- for s3://; set s3_region / s3_endpoint / keys (CREATE SECRET) first
  SELECT machine, max(klark_rss_bytes)/1048576 AS peak_rss_mb, max(klark_cpu_percent) AS peak_cpu
  FROM read_json_auto('s3://bucket/klarkhub/prod/analytics/**/*.ndjson.gz')
  GROUP BY machine ORDER BY peak_rss_mb DESC;
  -- a folder target: read_json_auto('/data/backup/analytics/**/*.ndjson.gz')
  ```

  The S3 writer is a small built-in SigV4 client (no SDK). It was verified against SeaweedFS in Docker (put, get, list, delete,
  prefix, path style) and a signature-checking fake; AWS S3 itself, Cloudflare R2, Garage and virtual-host addressing were not
  tried (they speak the same API). DuckDB was not run against the files here.

## Storage and backups

One target, `HUB_BACKUP=none|folder|s3`, holds both under `HUB_BACKUP_PATH` (a folder, or a key prefix for S3):

```
<path>/sqlite/hub-<UTC time>.db.gz       SQLite snapshots, owner marker <path>/sqlite/owner.json
<path>/analytics/YYYY/MM/DD/HH-<hubid>.ndjson.gz
```

- **The image defaults to `HUB_BACKUP=folder`, `/data/backup`: the same volume as the database.** It survives a restart or a
  corrupt file, not a lost volume. For a real backup point it at S3 (or another disk).
- **SQLite backup** is a consistent snapshot (`VACUUM INTO`, gzip) every `HUB_SQLITE_BACKUP_EVERY` (default 6h, min 5m), skipped when
  nothing changed, plus one on clean shutdown; the newest `HUB_SQLITE_BACKUP_KEEP` (7) are kept. **Data-loss window after a crash or
  a lost disk: up to the interval** (6h by default). It is not continuous replication; set `30m` or `1h` for a smaller window.
  On start a missing `hub.db`, or one older than the newest snapshot, is restored from it. One hub per location (owner marker, 90 s
  expiry): a second hub on the same path refuses to start with a clear message. Restore uses the environment's settings (admin-saved
  settings live in the database).
- **Swap the storage** by setting only endpoint and keys: AWS S3 (leave `_ENDPOINT` empty, set region), Cloudflare R2
  (`_ENDPOINT=https://<account>.r2.cloudflarestorage.com`, region `auto`), Garage (its S3 endpoint, path style) or SeaweedFS
  (`http://host:8333`, path style). Only SeaweedFS was run here.
- **Try it locally**: `HUB_BACKUP=s3 HUB_BACKUP_PATH=klarkhub docker compose --profile s3 up -d` starts a pinned SeaweedFS
  (`chrislusf/seaweedfs:4.00`, Apache-2.0) with one access key and creates the bucket `klarkhub`. A compose profile cannot change the
  hub's own environment, so the two variables are given on the command line (or in a `.env` file).

## One instance

The hub is one instance with one SQLite database (backed up to a folder or S3, see above); teams that need more can build on it.

## Run locally

The binary embeds `internal/ui/dist`, which is **not committed** (only `.gitkeep`). On a fresh clone build the UI
first, or the binary serves a placeholder page:

```sh
cd hub-go/ui && npm ci && npm run build      # writes ../internal/ui/dist
cd .. && go run ./cmd/klarkhub               # http://localhost:7470, sign in admin / adminadmin
curl http://localhost:7470/healthz           # ok
```

`make dev` runs the hub with the mock ADFS and `./data`; `make ui` builds the UI; `make build` makes
`bin/klarkhub` (static, `CGO_ENABLED=0`).

UI development:

```sh
cd hub-go/ui && npm run dev        # Vite; proxies /api, /auth, /agent, /mock-adfs to the Go server on 7470
cd hub-go/ui && npm run dev:mock   # no Go server: the UI against fixtures
```

Link a machine (the token is a long random string; the hub keeps only its sha256):

```sh
T=$(openssl rand -hex 32)
HUB_MACHINES="mac=$(printf %s "$T" | shasum -a 256 | cut -d' ' -f1)" go run ./cmd/klarkhub
# on the machine: klark expose with --token "$T" (see docs/adr/0010 and the machines page of the docs)
```

`klarkhub hash-token` and `klarkhub hash-password` read stdin and print the hash to use in `HUB_MACHINES` and
`HUB_USERS`. Machines can also join with `klark hub connect --url <hub>` and a browser approval at `/connect`.

## Tests

```sh
cd hub-go && make test     # gofmt check, go vet, go test -race ./..., then npm test in ui/
```

## Single container

```sh
docker build -t klarkhub hub-go
docker run -d -p 7470:7470 -v klarkhub-data:/data -e HUB_ADMIN_PASSWORD='choose-one' klarkhub
# or: cd hub-go && docker compose up -d    (set HUB_ADMIN_PASSWORD in the file first)
```

The Dockerfile builds the UI with Node, builds the Go binary with `CGO_ENABLED=0`, and ships it alone on a
distroless non-root base with a healthcheck (`klarkhub healthcheck`). `/data` is a volume. Run one replica:
SQLite is a local file. The backend agent built it (25 MB); the docs agent did not rebuild it.

## Behind a reverse proxy

Set `HUB_BASE_PATH=/klark`. The hub serves everything under that prefix, so the proxy keeps the prefix (does
not strip it). Turn buffering off for the event streams.

nginx:

```nginx
location /klark/ {
    proxy_pass http://127.0.0.1:7470;     # no URI part: the prefix is kept
    proxy_http_version 1.1;
    proxy_buffering off;                  # /agent/events and the browser SSE streams
    proxy_read_timeout 1h;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

Caddy:

```caddy
example.com {
    handle /klark/* {
        reverse_proxy 127.0.0.1:7470 {
            flush_interval -1
        }
    }
}
```

The backend agent reports the base path works at `/` and `/klark/` in the container. The snippets themselves
were not run by the docs agent.

## Mock ADFS walkthrough

For development and tests only: never enable it on a reachable hub.

```sh
HUB_MOCK_ADFS=1 HUB_MOCK_ADFS_USERS="alice=pw:admin,bob=pw" go run ./cmd/klarkhub
```

1. Open `http://localhost:7470/login`. "Sign in with ADFS" appears next to the password form.
2. It goes to `/mock-adfs/authorize`, a small login form. Sign in as `alice` / `pw`.
3. The hub's `/auth/adfs/callback` exchanges the code at `/mock-adfs/token`, sets the `klark_session` cookie
   and returns to `/`. `alice` is an admin.
4. Discovery is at `/mock-adfs/.well-known/openid-configuration`.

The flow has an HTTP-level end-to-end test against the mock ADFS, which is the supported test path.

## Parity checklist against the Blazor hub

Derived from reading `hub/` (pages, endpoints, services), updated 2026-11-12 from the coordinator's report.
"done" = in the Go and React code and checked; "stub" = present but incomplete.

| Blazor feature (source) | state |
|---|---|
| Machine link: probe, session, events, push, resume, duplicate push, gzip (`Program.cs`, `MachineRegistry.cs`) | done |
| Machine token and fleet token check (`HubAccounts`) | done |
| Offline sweep, revoke a machine, admin machines page | done |
| Health check `/healthz` | done |
| Sign-in form, session cookie (`Login.razor`) | done |
| Built-in `admin` account, static users, hashed | done |
| Change password page (`Password.razor`) | not started |
| OIDC / ADFS, mock ADFS (new, not in Blazor) | done; mock ADFS is the tested path |
| Built-in analytics of the machines (lazy `system.stats`, history in the database, folder/S3 files for DuckDB) (new, not in Blazor) | done; unit tests, real SeaweedFS checked; DuckDB not run against the files here |
| SQLite snapshot backup + restore on start, one storage target (new) | done; folder and SeaweedFS tested; loss window = the interval |

| Sign-in: default `local` (built-in admin only); mock LDAP opt-in with `HUB_AUTH_MODE=mockldap` | done |
| LDAP (direct bind / search-then-bind, groups, allowed groups, test), mock LDAP, extra OIDC providers | done; unit-tested with a fake connection and tried by hand against OpenLDAP (osixia/openldap in Docker): search-then-bind, group search, admin group, wrong and empty password; direct bind, LDAPS, StartTLS and Active Directory not tried |
| Per-group machine visibility, read-only viewers (`HubAccess`) | not started |
| Sign-in rate limit | not checked |
| Machines and agents sidebar, status (`Sidebar.razor`, `Home.razor`) | done (short names truncate: UI nit) |
| Session tree per machine (`SessionTree.cs`) | done |
| Offline machines: last tree and recorded output | not checked |
| SQLite store, 10 MB per machine, retention (`HubStore.cs`) | done: pure-Go SQLite; per-machine cap not checked |
| Chat view of a pane, windowed rows (`ChatView.razor`, `ChatWindow.cs`) | done |
| Screen readers for Claude, Devin, Codex, Agy, generic (`ScreenTransformers.cs`) | done |
| Transcript feeds and cleaning (`TranscriptFeeds.cs`, `TranscriptClean.cs`) | done as the chat pipeline; not compared row by row with Blazor |
| Command-output capture (`CommandOutput.cs`) | done |
| Markdown, tables, ANSI (`ChatMarkdown.cs`, `AnsiHtml.cs`) | done |
| Syntax highlighting | not started |
| Terminal view toggle, frames | done |
| Reply, keys, Esc, Ctrl-C (`PaneMain.razor`) | done |
| Quick-reply buttons for a blocked pane | not started |
| Slash-command popup | stub: the UI carries its own list; server-side `SlashCommands` not ported |
| Hub note to agents (`HubNote.cs`, `Hub:AgentNote`) | not checked |
| File viewer, markdown, raw, 10 MB limit, secrets refused (`FileEndpoints.cs`, `FileLinks.cs`) | done |
| Image upload from the phone, assets store (`Assets.cs`) | done: images (png/jpeg/gif/webp/heic/avif by magic bytes, 10 MB) stored in hub.db by SHA-256, kept 24 h, `fetch_asset` to the machine; paste / drop / pick with preview. Verified end to end with klark 0.9.5 (file landed at `<klark dir>/uploads/<sha>.png`, 0600, same sha256) |
| `klark offer`: `offers`, `offer` event | not started |
| Agent messages page and API (`Messaging.cs`, `Messages.razor`) | done |
| User JWTs, `/token` page, `POST /api/token` (`ApiAuth.cs`, `Token.razor`) | not started |
| Machine approval in the browser (`/connect`, `MachineConnect.cs`) | done: same `/api/connect/start` + `/poll` contract; requests in the database, 10 min expiry, rate-limited; token minted on collection, only its SHA-256 stored. Verified with an unmodified `klark hub connect` and a real browser approval, then revoke |
| Admin users, machines | done |
| Admin directory | API done (`API.md`) |
| Search (cmd-K) | not checked |
| Brand name, tagline, logo options | not checked |
| Themes | done (seven) |
| Config from environment, `/data` volume | done |
| Docker image, compose | done (Kubernetes example not ported) |
| Base path behind a proxy (new) | done |
| Load and soak test (E-06 equivalent) | not started |

"not checked" means the docs agent could not tell from the report; look in the code before relying on it.
