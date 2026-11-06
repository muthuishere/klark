# klark hub

A Blazor Server app that many people sign in to in order to watch and answer
the coding agents on many klark machines from one place.

```
klark machine ──(outbound wss, machine token)──▶ klark hub ◀──(browser, sign-in)── people
```

- **Machines dial out.** `klark link` opens one WebSocket to `/agent`. Nothing
  listens on the machine and no tunnel is needed.
- **Lazy.** On connect a machine sends only its pane tree, and sends it again
  only when it changes. A pane's terminal bytes flow only while at least one
  person has that pane open. The first viewer starts the stream and the last
  one to leave stops it.
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

Or with Docker: put your users and machine hashes in `hub/appsettings.Production.json`, then run `cd hub && docker compose up -d --build`.
Put it behind TLS (any reverse proxy) so machines can use `wss://`.

## Storage

SQLite, in `Hub:DataDir` (`/data` in Docker):

- `hub.db` lists every machine that has connected, with its host, version and when it was first and last seen.
- `machines/<name>.db` holds one machine's data: its latest pane tree, recent pane output and connect/disconnect events. Each file is capped at `Hub:MaxBytesPerMachine` (10 MB by default). The oldest output is dropped first.

Writes are batched by one background writer, so disk never slows the machine sockets.

## Link a machine

```bash
klark link --hub wss://hub.example.com/agent --token <machine token> [--name laptop]
# or KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME
```

The machine shows up on the hub under the name configured for its token. The
link reconnects with backoff when the connection drops.

## Wire protocol

JSON messages `{"type": ...}`. Machines send them gzip-compressed in binary frames, with terminal output merged per pane every ~25 ms. The hub sends plain JSON text. The architecture and status are tracked in `docs/adr/0001-klark-hub-architecture.md`. It is defined in `crates/klark-link/src/protocol.rs`.

| machine → hub | hub → machine |
|---|---|
| `hello`, `tree`, `frame` (base64 ANSI), `closed`, `result`, `pong` | `subscribe`, `unsubscribe`, `input`, `keys`, `call`, `refresh`, `ping` |
