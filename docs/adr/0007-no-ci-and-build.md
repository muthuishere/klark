# ADR 0007: no CI; build and verify by hand

- Status: accepted (decided by the owner)

## Decision
No CI. The inherited herdr workflows in `.github/workflows/` are to be deleted
by hand in the GitHub UI. This agent's GitHub access cannot change that folder.

## How to build and verify
```bash
# klark (needs Rust and Zig 0.16 on PATH)
cargo build --release
cargo test -p klark-link             # link unit tests + end-to-end test (no Zig needed)

# hub (needs .NET 9 SDK, or just Docker)
cd hub && dotnet run                  # http://localhost:8080, MockLdap: admin/admin
docker compose up -d --build          # same, in Docker, data in the hub-data volume

# load
cargo run --release -p klark-link --bin klark-loadtest -- --hub ws://localhost:8080/agent --token <fleet> --machines 100
```

## Consequences
Nothing checks pull requests automatically. Before merging, run the commands
above. The first full build of klark and the hub has not happened yet (S-01,
S-02 in STATUS).
