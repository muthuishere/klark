# ADR 0014: the hub is rewritten in Go (API) and React (UI), one binary, one container

- Status: accepted (owner request, 2026-11-12). **In progress**: nothing here is released.
  The released hub is still the C# Blazor Server one in `hub/` (v0.9.5.6).
- Partly supersedes: ADR 0008's wording "C# Blazor Server" for the hub, and only at cut-over
  (see Migration). Until then ADR 0008 describes what ships.
- Keeps unchanged: ADR 0013 (the machine link), ADR 0003 (SQLite, one file per machine, 10 MB cap),
  ADR 0005 (plain HTTP), ADR 0002 (lazy pull).

## Context
The hub is the only part of klark written in C#. The owner's other software is Go and React, so the hub
is the one piece that needs a second toolchain, a second runtime image and a second set of habits. The
Blazor Server UI also holds one server-side circuit per open browser tab (E-06 measured 1.2 to 13 MiB each),
and the browser talks to the server over SignalR, which needs its own transport rules.

## Decision
- **Go API.** One static binary, no .NET runtime. SQLite through a pure-Go driver, so the build needs no C
  compiler and the image can be small.
- **React 18 + Vite + TypeScript UI**, built to static files and embedded in the binary with `go:embed`.
  The Go server serves it; any path that is not a file serves `index.html` so client routes survive a reload.
- **One container, one port, 7470.** Same as today.
- **`HUB_BASE_PATH`** (default `/`) mounts the hub under a prefix for a reverse proxy
  (`https://host/klark/`). The server injects `<base href>` into `index.html`; the UI uses relative URLs only.
- **Server-Sent Events** for the browser (chat rows, terminal frames, tree changes), not SignalR. It is plain
  HTTP, the same family as ADR 0013's machine link.
- **The machine link is unchanged.** ADR 0013 protocol v4/5: `GET /agent`, `POST /agent/session`,
  `GET /agent/events`, `POST /agent/push`, `GET /healthz`. Existing klark machines and their tokens keep
  working against either hub.
- **Sign-in** with two providers, each optional and chosen by environment: static credentials
  (`HUB_USERS`, hashed, plus the built-in `admin`) and OIDC / ADFS. For development and tests the hub can
  serve a **built-in mock ADFS** (`HUB_MOCK_ADFS=1`) and use itself as its client, so the whole OIDC flow can
  be run and tested without an identity provider. LDAP, the Blazor hub's default, is not in this list;
  whether it comes over is open (see Risks).
- The contract is `hub-go/API.md`. The UI follows that file.

## Migration
1. Side by side: the Go hub lives in `hub-go/`; `hub/` is not touched.
2. Parity checklist in `hub-go/README.md` (every Blazor feature, with its state). Work through it.
3. Cut over when the checklist has no open rows that the owner has not waived: the release image is built
   from `hub-go/`, docs and the installers' examples switch, `ghcr.io/muthuishere/klarkhub` points at the
   new build.
4. Then retire `hub/` (delete it, supersede ADR 0008's wording in a follow-up note, update the site).

## State on 2026-11-12
Still in progress and not released. Implemented in `hub-go/` and checked by the coordinator (`go vet`,
`go test -race`, 51 UI tests, a real `klark expose` machine online, login with static and mock-ADFS providers in
a real browser): the machine link, config and base path, pure-Go SQLite, static credentials, the OIDC client and
mock ADFS, machines/tree/input/keys, per-pane SSE, the chat pipeline (Claude, Devin, Codex, Agy, generic),
files, messages, admin users and machines, the React UI, and a single-container image (25 MB, distroless).

Not done: image upload (501), `klark offer`, server-side slash commands, syntax highlighting, quick replies,
CLI token and change-password pages, LDAP, per-machine groups, browser approval for `klark hub connect`
(machines join by `HUB_MACHINES` tokens), any load or soak test. OIDC has not met a real ADFS or Entra.
The per-item table is the parity checklist in `hub-go/README.md`.

## Consequences
- One language family, one binary, no runtime in the image, a smaller image.
- Browser state is a cookie and SSE streams, not a circuit per tab. Memory per viewer should drop; this is
  **not measured yet** and must be measured before it is claimed (new experiment).
- The Blazor hub carries a lot of behaviour that is not in the protocol. Most is ported (see State); image
  upload, offers, slash-command lists, JWTs and LDAP are not. All of it has to be ported or consciously dropped. The 256 tests in
  `hub/hub.Tests` are the behaviour reference.
- Two hubs exist in the repository until cut-over; fixes to the shipped one must not be forgotten in the port.

## Risks
- Parity takes longer than the protocol did; the transformers are the long tail.
- LDAP / Active Directory sign-in, per-group visibility and read-only viewers exist in the Blazor hub and are
  not in the Go contract. Decide before cut-over: port, replace with OIDC groups, or drop.
- OIDC is tested only against the built-in mock. A real ADFS or Entra may differ (claims, group names, keys).
- A data-dir migration (existing `hub.db` and `machines/*.db`) is not designed yet. Fresh start is the
  fallback; machines re-link by themselves.
- Rewrites regress things nobody wrote down. Mitigation: the checklist and a side-by-side run against the
  same machine.
