# klark docs

| doc | what |
|---|---|
| [adr/0010](adr/0010-klark-expose.md) | the command is `klark expose`; what it sends, and when |
| [HANDOVER.md](HANDOVER.md) | **start here**: what exists, what is verified, first steps |
| [STATUS.md](STATUS.md) | what is done, what is not, and what comes next (one checklist) |
| [adr/0001](adr/0001-klark-hub-architecture.md) | the overall architecture: rename, link, hub, wire, storage (D1–D13) |
| [adr/0002](adr/0002-lazy-pull-protocol.md) | link protocol v1: every message, lazy pull rules |
| [adr/0003](adr/0003-hub-storage-sqlite.md) | SQLite layout, the 10 MB per-machine cap, retention |
| [adr/0004](adr/0004-authentication.md) | sign-in (mock LDAP, LDAP, local), machine tokens, fleet tokens |
| [adr/0005](adr/0005-plain-http-and-security.md) | running on plain HTTP: what it means and its limits |
| [adr/0006](adr/0006-scale-100-machines.md) | how the hub handles 100+ machines, and the budget per machine |
| [adr/0007](adr/0007-no-ci-and-build.md) | no CI; how to build and verify by hand |
| [adr/0008](adr/0008-hub-and-herdr-expose-are-separate.md) | the klark hub and herdr-expose are separate products |
| [adr/0009](adr/0009-agent-messaging.md) | agents message agents across sessions and machines; user JWTs; LDAP as the default sign-in |
| [HANDOVER.md](HANDOVER.md) | read first: what klark is, what is verified, first steps |
| [experiments/](experiments/README.md) | measured experiments: what was tried, the numbers, what is still to try |

ADR format: status, context, decision, consequences. When a decision changes,
write a new ADR that supersedes the old one; don't rewrite history.
