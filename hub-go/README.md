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
| OIDC / ADFS client, mock ADFS (HTTP-level end-to-end test) | done; OIDC **not tested against a real ADFS or Entra** (RSA keys only) |
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

- Image upload (`501`; the asset store is not ported).
- `offers` and the `offer` event (`klark offer`).
- Server-side slash-command lists (the UI carries its own list).
- Syntax highlighting; quick-reply buttons for a blocked pane.
- CLI token page and change-password page.
- Per-machine access groups.
- Browser approval for `klark hub connect`: not ported, so machines join with `HUB_MACHINES` tokens (or from
  `/admin/machines`) for now.
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
| `HUB_MOCK_ADFS`, `HUB_MOCK_ADFS_USERS` | off | `1` serves a mock IdP at `/mock-adfs/`; users `alice=pw:admin,bob=pw`. Dev and tests only |

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
`HUB_USERS`. Machine approval in the browser (`klark hub connect`) is not ported yet.

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

The flow has an HTTP-level end-to-end test. A real ADFS or Entra ID has not been tried.

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
| OIDC / ADFS, mock ADFS (new, not in Blazor) | done; real ADFS / Entra untested |
| LDAP (direct bind / search-then-bind, groups, allowed groups, test), mock LDAP, extra OIDC providers | done; tested against a fake LDAP connection only, no real LDAP server yet |
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
| Image upload from the phone, assets store (`Assets.cs`) | not started (returns 501) |
| `klark offer`: `offers`, `offer` event | not started |
| Agent messages page and API (`Messaging.cs`, `Messages.razor`) | done |
| User JWTs, `/token` page, `POST /api/token` (`ApiAuth.cs`, `Token.razor`) | not started |
| Machine approval in the browser (`/connect`, `MachineConnect.cs`) | not started; machines join by `HUB_MACHINES` |
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
