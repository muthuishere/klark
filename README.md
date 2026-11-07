# klark

klark is a terminal workspace for AI coding agents, with a built-in link to a
shared web hub.

- **Persistent panes.** Agents keep running in a background server when you
  detach. Run `klark` to reattach, and `ctrl+b q` to detach.
- **Agent-aware.** Every pane is marked working, blocked or idle, so you can
  see which agent is waiting for an answer.
- **Hub link.** `klark expose` connects this machine to a [klark hub](hub/README.md),
  a Blazor Server app where a team signs in and watches and answers the agents
  on every linked machine (built for 100+ machines). Panes stream only while
  someone is looking at them.
- **No phoning home.** No self-update, no hosted announcements, no remote
  catalogs. klark uses what is built into the binary.

## Build

```bash
cargo build --release          # needs Zig 0.16 for the vendored libghostty-vt
./target/release/klark
```

## Hub in two steps

1. Deploy the hub (klarkhub: `hub/`, Docker image; Kubernetes or `docker compose up -d`).
2. On each machine:

```bash
klark hub connect --url http://hub-host:8080   # approve this machine in the browser (device code)
klark hub connect                              # afterwards: reconnect to the default hub
klark hub ls / use <name> / rm <name> / status
```

Hubs are saved in `<config dir>/hubs.json` (0600). Scripts can pass `--token`/`--token-stdin`
or `KLARK_HUB_TOKEN` instead of the browser step. `https://` hubs work (rustls); for a
private CA or self-signed ingress set `KLARK_HUB_CA_FILE=/path/ca.pem`. See ADR 0012.

## Link a machine to a hub

```bash
klark expose --hub http://hub-host:8080 --token <machine token> [--name laptop]
klark expose url            # inside a pane: the hub link to this pane ("share this")
```

The hub is a **separate web server**: deploy it anywhere with Docker
(`hub/`, `docker compose up -d`), then link any number of klark machines to it.
Each machine connects out to whichever hub URL it is given; one hub is built
for 100+ machines and many users. The `klark` skill (skills/klark) lets an agent hand
back its own hub link. (herdr-expose is a separate product for herdr; see ADR 0008.)

## Layout

| path | what |
|---|---|
| `src/` | klark itself (Rust) |
| `crates/klark-link` | the machine side of the hub link |
| `hub/` | the hub: C# Blazor Server, SQLite, Docker |
| `api/klark-api.schema.json` | socket API schema (`klark api schema`) |
| `docs/` | [handover](docs/HANDOVER.md), [status](docs/STATUS.md), [ADRs](docs/README.md), [experiments](docs/experiments/README.md) |
| `experiments/` | scripts behind the measured experiments |

Existing users: on first run, klark moves an old `~/.config/herdr` directory to
`~/.config/klark`. Environment variables are now `KLARK_*` (for example
`KLARK_SOCKET_PATH`).

klark is based on [herdr](https://github.com/herdrdev/herdr) (Apache-2.0).
