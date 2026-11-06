# klark hub

A Blazor Server app that many people sign in to in order to watch and answer
the coding agents on many klark machines from one place.

```
klark machine ──(outbound wss, machine token)──▶ klark hub ◀──(browser, sign-in)── people
```

- **Machines dial out.** `klark link` opens one WebSocket to `/agent`. Nothing
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

Or with Docker: put your users and machine hashes in `hub/appsettings.Production.json`, then run `cd hub && docker compose up -d --build`.
The hub is plain HTTP on port 8080; machines connect with `ws://`. Run it on a
trusted network or VPN. Tokens and passwords cross the wire unencrypted, so for
public exposure put a TLS proxy in front and use `wss://` (the link supports both).

## Sign-in

`Hub:Auth:Mode`:

- `MockLdap` (default): a built-in pretend LDAP directory. Demo users are
  `admin/admin`, `alice/alice` and `bob/bob`; set your own in
  `Hub:Auth:MockUsers` (`Uid`, `Password`, `DisplayName`, `Groups`).
- `Ldap`: a real server. Set `Hub:Auth:Ldap` (`Host`, `Port`, `UserDnTemplate`,
  `GroupSearchBase`). To try it:
  `HUB_AUTH_MODE=Ldap docker compose --profile ldap up -d --build`, which signs
  in alice/alice or bob/bob against a test OpenLDAP.
- `Local`: `Hub:Users` with `dotnet run -- hash-password` hashes.

`Hub:Auth:AllowedGroups`, for example `["klark-users"]`, limits who can sign in.

## Load test

```bash
# hub config: "Machines": [{ "Name": "load", "Fleet": true, "TokenSha256": "<hash-token fleet-secret>" }]
cargo run --release -p klark-link --bin klark-loadtest -- \
  --hub ws://localhost:8080/agent --token fleet-secret --machines 100 --seconds 60
```

Then sign in and open a few panes: only those panes start streaming.

## Storage

SQLite, in `Hub:DataDir` (`/data` in Docker):

- `hub.db` lists every machine that has connected, with its host, version and when it was first and last seen.
- `machines/<name>.db` holds one machine's data: its latest pane tree, recent pane output and connect/disconnect events. Each file is capped at `Hub:MaxBytesPerMachine` (10 MB by default). The oldest output is dropped first.

Writes are batched by one background writer, so disk never slows the machine sockets.

## Link a machine

```bash
klark link --hub ws://hub-host:8080/agent --token <machine token> [--name laptop]
# or KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME
```

The machine shows up on the hub under the name configured for its token. The
link reconnects with backoff when the connection drops.

## Wire protocol

JSON messages `{"type": ...}`. Machines send them gzip-compressed in binary frames, with terminal output merged per pane every ~25 ms. The hub sends plain JSON text. The architecture and status are tracked in `docs/adr/0001-klark-hub-architecture.md`. It is defined in `crates/klark-link/src/protocol.rs`.

| machine → hub | hub → machine |
|---|---|
| `hello`, `tree` / `tree_unchanged` (only when asked), binary output frames, `closed`, `result`, `pong` | `get_tree`, `subscribe`, `unsubscribe`, `input`, `keys`, `call`, `ping` |
