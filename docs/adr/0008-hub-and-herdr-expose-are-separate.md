# ADR 0008: the klark hub and herdr-expose are separate products

- Status: accepted (owner decision, 2026-11-06). Answers O4 and S-18.

## Context
herdr-expose (`muthuishere/herdr-expose`) is a working Go plugin with a React
web UI. It puts herdr sessions on a URL. klark needed the same kind of thing
for its own machines, built as a C# Blazor Server hub that many klark
machines and many users share.

## Decision
- **herdr-expose stays as it is**: a separate, working product for herdr. It
  is not retired, archived or changed by klark work.
- **klark has its own hub** (`hub/`, C# Blazor Server) and its own link
  (`crates/klark-link`, Rust). They follow herdr-expose's ideas (dial out,
  read a pane without resizing it, pull instead of push) but share no code or
  runtime with it.
- **The hub is a separate web server**, deployed on its own (Docker, any host).
  It is not built into klark. A klark machine connects to any hub with
  `klark link --hub ws://<hub>:8080/agent --token <token>`, and one hub serves
  many klark machines (designed and load-tested for 100) and many users.
- klark itself has no web UI. Without a hub, the TUI is its interface.
- herdr-expose features that klark wants are built klark's way:

| idea in herdr-expose | klark equivalent | status |
|---|---|---|
| socket client, terminal observe | `crates/klark-link` | done (D-04) |
| web UI | the hub | done (not yet compiled) |
| `share` + skill ("share this" → link) | `klark link url` + `skills/klark-share`, hub `/go` | done (D-24) |
| scoped, time-boxed, pairing-gated guest links, QR codes | hub guest links | S-24 |
| agents messaging agents across machines | messaging through the hub | S-25 (needs its own ADR: machine-initiated, an exception to ADR 0002) |
| `panic` | hub admin: disconnect / revoke a machine | S-26 |
| service install, `doctor`, `logs` | `klark link` as an autostart service, `klark link doctor` | S-27 |
| transcript view | hub transcript (`agent.read`) | done (D-16) |

## Consequences
- Two products, maintained independently: herdr + herdr-expose, and klark +
  hub.
- To use a different hub, point `klark link` at a different URL. To appear
  on two hubs at once, run two `klark link` processes.
