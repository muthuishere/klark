# Contributing to klark

klark is a small project. Contributions are welcome when they fit its
direction: a self-contained agent runtime plus a lazy, shared web hub.

## Before you start

1. Read [`docs/STATUS.md`](docs/STATUS.md). It is the backlog. Pick an item
   (S-number), or open an issue describing the problem you want to solve.
2. Read the ADRs for the area you are touching
   ([`docs/README.md`](docs/README.md) lists them), and
   [`AGENTS.md`](AGENTS.md), which holds the engineering rules for people and
   coding agents alike.
3. For anything bigger than a bug fix, open an issue first so the approach can
   be agreed on before you write it.

## Using coding agents

Agents are welcome. You are responsible for what yours submits:

- Point it at `AGENTS.md` and `docs/STATUS.md`.
- Make sure it actually ran the checks it claims to have run.
- Review the diff yourself before opening the pull request.

## Reporting a bug

Open an issue with:

- what happened, and what you expected;
- the shortest steps that reproduce it;
- klark version (`klark --version`), OS, terminal, and for hub issues the
  browser and hub config mode (`MockLdap` / `Ldap` / `Local`);
- the smallest useful log excerpt.

## Pull requests

- One topic per pull request, branched from `master`.
- Say what you verified and how: the exact commands from "Testing" in
  `AGENTS.md`. There is no CI, so this is the review's main evidence.
- Update `docs/STATUS.md`: move finished items to Done with how they were
  verified, and add any follow-up you discovered.
- New architecture gets an ADR, and new measurements get an experiment write-up.
- Keep the decisions listed under "Decisions you must not undo" in `AGENTS.md`,
  or propose changing them in an ADR first.

## Building

| part | needs |
|---|---|
| klark (`cargo build`) | Rust (see `rust-toolchain.toml`) and Zig 0.16 (`ZIG=/path/to/zig`) |
| `crates/klark-link` (`cargo test -p klark-link`) | Rust only |
| hub (`cd hub && dotnet build`) | .NET 9 SDK, or Docker (`docker compose up -d --build`) |

## License

By contributing you agree that your contribution is licensed under the
Apache License 2.0, the same as the rest of klark.
