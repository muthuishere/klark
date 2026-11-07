---
title: Design records
description: The architecture decision records and measured experiments behind klark and the hub.
---

These are internal engineering records. They keep their original wording as
history, so older ones may describe things that a later record replaced (for
example, WebSockets before ADR 0013). This site describes the current state.

All live in the repository under `docs/`.

## Architecture decisions

| ADR | decision |
|---|---|
| [0001](https://github.com/muthuishere/klark/blob/master/docs/adr/0001-klark-hub-architecture.md) | klark, the built-in link, and the hub; no phoning home |
| [0002](https://github.com/muthuishere/klark/blob/master/docs/adr/0002-lazy-pull-protocol.md) | the hub always pulls; output flows only while viewed |
| [0003](https://github.com/muthuishere/klark/blob/master/docs/adr/0003-hub-storage-sqlite.md) | SQLite, one file per machine, 10 MB cap |
| [0004](https://github.com/muthuishere/klark/blob/master/docs/adr/0004-authentication.md) | sign-in, machine and fleet tokens, groups |
| [0005](https://github.com/muthuishere/klark/blob/master/docs/adr/0005-plain-http-and-security.md) | plain HTTP; TLS only through a proxy |
| [0006](https://github.com/muthuishere/klark/blob/master/docs/adr/0006-scale-100-machines.md) | budget for 100+ machines |
| [0007](https://github.com/muthuishere/klark/blob/master/docs/adr/0007-no-ci-and-build.md) | no CI; build and verify by hand |
| [0008](https://github.com/muthuishere/klark/blob/master/docs/adr/0008-hub-and-herdr-expose-are-separate.md) | the hub and herdr-expose are separate products |
| [0009](https://github.com/muthuishere/klark/blob/master/docs/adr/0009-agent-messaging.md) | agents message agents; user JWTs; LDAP default |
| [0010](https://github.com/muthuishere/klark/blob/master/docs/adr/0010-klark-expose.md) | the command is `klark expose` |
| [0011](https://github.com/muthuishere/klark/blob/master/docs/adr/0011-assets-and-faithful-rendering.md) | assets, faithful rendering, chat view, app shell, file viewer |
| [0012](https://github.com/muthuishere/klark/blob/master/docs/adr/0012-klark-hub-connect.md) | `klark hub connect` with a device code |
| [0013](https://github.com/muthuishere/klark/blob/master/docs/adr/0013-http-only-transport.md) | HTTP-only transport, no WebSockets |

## Experiments

Index: [docs/experiments/README.md](https://github.com/muthuishere/klark/blob/master/docs/experiments/README.md).
The ones this site quotes:

- [E-06](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-06-real-hub-resources.md): the real hub under load, and the sizing table.
- [E-11](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-11-http-transport.md): HTTP-only transport against the WebSocket link.
- [E-12](https://github.com/muthuishere/klark/blob/master/docs/experiments/E-12-ui-performance.md): chat shell and file viewer load times.

## Project state

- [docs/STATUS.md](https://github.com/muthuishere/klark/blob/master/docs/STATUS.md): what is done, verified, and left.
- [docs/HANDOVER.md](https://github.com/muthuishere/klark/blob/master/docs/HANDOVER.md): where to start if you pick the project up.
