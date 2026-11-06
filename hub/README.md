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

Or with Docker: `docker build -t klark-hub hub && docker run -p 8080:8080 -v $PWD/hub/appsettings.Production.json:/app/appsettings.Production.json klark-hub`.
Put it behind TLS (any reverse proxy) so machines can use `wss://`.

## Link a machine

```bash
klark link --hub wss://hub.example.com/agent --token <machine token> [--name laptop]
# or KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME
```

The machine shows up on the hub under the name configured for its token. The
link reconnects with backoff when the connection drops.

## Wire protocol

JSON text frames, `{"type": ...}`. It is defined in `crates/klark-link/src/protocol.rs`.

| machine → hub | hub → machine |
|---|---|
| `hello`, `tree`, `frame` (base64 ANSI), `closed`, `result`, `pong` | `subscribe`, `unsubscribe`, `input`, `keys`, `call`, `refresh`, `ping` |
