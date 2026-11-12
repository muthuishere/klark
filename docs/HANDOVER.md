# klark handover

Read this first if you are picking up klark, whether you are a person or an agent.
State as of 2026-11-12 (Go hub rewrite started, see Open work item 1; otherwise as of 2026-11-10): `master` includes everything through the docs commit after release
**v0.9.5.5**. `docs/STATUS.md` is the older requirement list; this file is the current picture.

Owner: muthuishere (GitHub `muthuishere/klark`). klark is muthuishere's project, never
deemwar's: do not frame it as a company product.

## What klark is

Two things in one repository:

1. **klark**: a terminal runtime for coding agents (Rust, one binary), forked from
   herdr 0.9.5. herdr is the upstream and a separate, running product: never touch
   `~/.config/herdr` or a running herdr.
2. **klark hub**: a separate web server (C# Blazor Server, .NET 10, SQLite, Docker) in
   `hub/`. Machines connect with `klark hub connect`; people sign in and watch and
   answer every agent from a browser or phone. **The released hub is this Blazor one (v0.9.5.6).**
3. **klarkhub in Go + React** (`hub-go/`, **in progress, not released**): a rewrite of the hub as a Go API with a
   React 18 + Vite + TypeScript UI embedded in the binary, one container, port 7470. Decision:
   `docs/adr/0014-go-react-hub.md`; contract: `hub-go/API.md`; status and parity checklist: `hub-go/README.md`.
   Implemented and checked (2026-11-12): machine link, SQLite, static and OIDC/mock-ADFS sign-in, data API, chat
   pipeline, React UI, single-container image. Not done: image upload, offers, per-machine groups, browser
   approval for `klark hub connect`, JWT pages, load tests. `hub/` stays until the parity checklist passes.

Docs site (Astro Starlight, `site/`): https://muthuishere.github.io/klark/ , deployed by CI
on every push to `master`. It uses the hub's Paper theme and has the one-line installers.

## Verified state

- `cargo nextest run --workspace` (this is what CI runs): 4055 tests pass.
  Plain `cargo test` shows ~50 failures: tests share environment variables. Use nextest.
- `cd hub && dotnet test hub.Tests`: 256 pass.
- `cd hub-go && go vet ./... && go test -race ./...` and `cd hub-go/ui && npm test` (51): pass (coordinator, 2026-11-12).
- `cd site && SITE=https://muthuishere.github.io BASE=/klark/ npm run build`: 27 pages, links valid.
- Live-tested on this Mac and on the Windows PC: the one-line installers, hub create/update
  file flows with Claude Code and Devin, `/` autocomplete, slash-command output, theme persistence.
- Flaky: `federated_client_starts_without_local_and_survives_its_restart`
  (`tests/client_mode.rs`, inherited from herdr) failed once in CI and passed on re-run.

## Releasing (the recipe that works)

Release version comes from the git **tag** (`build.rs`: `v0.9.5.5` becomes `0.9.5+5`), so build
the desktop binaries **after** tagging. `klark --version` still prints `0.9.5` (Cargo version);
the release number is recorded in `~/.config/klark/versions.txt`.

1. Put a `## [x.y.z.w] - date` section at the top of `CHANGELOG.md` (below `## Unreleased`).
   It is compiled into the binary and shown in the terminal's "what's new".
2. Run `cargo nextest run --workspace`, the hub tests and the site build.
3. Commit, `git tag -a vx.y.z.w -m "klark vx.y.z.w"`.
4. Build the macOS and Windows binaries (below), each with a `.sha256`
   (`shasum -a 256 <file> > <name>.sha256`; the `.exe` checksum file is `klark-windows-x86_64.sha256`).
5. `git push origin HEAD:master` and `git push origin vx.y.z.w`. CI (`.github/workflows/klark.yml`)
   tests, builds the Linux x86_64 and aarch64 binaries, pushes the `ghcr.io/muthuishere/klarkhub` image
   and creates the release with the Linux files. The master run deploys the docs.
6. **Immediately** `gh release upload vx.y.z.w <macos/windows files> --clobber`, then
   `gh release edit vx.y.z.w --title ... --notes-file ...`. The installers use `releases/latest`, so
   until the macOS and Windows files are uploaded the one-liners 404 on those platforms.

**Windows zip with the modern ConPTY (also upload this):** Windows' own ConPTY drops the answers agents send
back to the terminal; klark's loader uses an app-local `conpty\` folder when it sits beside `klark.exe`. After the
cross-build: download the pinned nupkg (`packaging/windows/conpty.json`, sha256-checked by the script), then
`python3 scripts/package_windows_conpty.py stage --package <nupkg> --klark-exe <klark.exe> --output-dir <stage>` and
`... archive --stage-dir <stage> --output klark-windows-x86_64.zip`, plus `shasum -a 256 ... > klark-windows-x86_64.zip.sha256`.
`install.cmd` prefers the zip and falls back to the bare `.exe`.

Asset names the installers expect: `klark-macos-aarch64`, `klark-macos-x86_64`, `klark-linux-x86_64`,
`klark-linux-aarch64`, `klark-windows-x86_64.exe`, each with `<name>.sha256`.

**macOS:** `cargo build --release` (arm64) and
`rustup target add x86_64-apple-darwin && cargo build --release --target x86_64-apple-darwin`.

**Windows (cross-built on the Mac):** one-time `cargo install xwin`, then
`python3 scripts/windows_cross.py setup --accept-license` (downloads Microsoft's SDK to
`~/.local/share/klark/windows-cross`) and `ln -sfn "$(xcrun --show-sdk-path)/usr" ~/.local/share/klark/windows-cross/usr`
(the zig build helper that runs on the Mac needs libSystem). Then, with `R=~/.local/share/klark/windows-cross`:

```sh
PATH=<dir with the lib.exe shim below>:$PATH \
LIBGHOSTTY_VT_WINDOWS_LIBC=$R/libc.txt LIBGHOSTTY_VT_SIMD=false \
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=rust-lld \
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Clinker-flavor=lld-link -Lnative=$R/crt/lib/x86_64 -Lnative=$R/sdk/lib/um/x86_64 -Lnative=$R/sdk/lib/ucrt/x86_64" \
CFLAGS_x86_64_pc_windows_msvc="-isystem $R/crt/include -isystem $R/sdk/include/ucrt -isystem $R/sdk/include/um -isystem $R/sdk/include/shared" \
cargo build --release --locked --target x86_64-pc-windows-msvc
```

`lib.exe` shim (cc-rs wants MSVC's `lib.exe`; this one calls zig's archiver, the Mac's `ar` cannot index Windows objects):

```sh
#!/bin/sh
out=""; objs=""
for a in "$@"; do case "$a" in -out:*) out="${a#-out:}";; -nologo|/nologo) ;; *) objs="$objs $a";; esac; done
rm -f "$out"; exec zig ar crs "$out" $objs
```

The Windows `.exe` is x64 and also runs on Windows on ARM. Windows support is new and little used.

## Where things are (current)

| path | what |
|---|---|
| `src/hub_cmd.rs` | `klark hub connect/stop/ls/use/rm/status`; connect runs in the background, the connected hub becomes the default, one connection at a time |
| `src/session.rs` | `--session [name]` / `-s [name]`: no name = a session named after the current folder |
| `src/transcript_cmd.rs`, `src/transcript_cmd/devin.rs` | `klark transcript` (a subprocess of the hub link): Claude jsonl (incl. `system`/`local_command` entries), Devin sqlite |
| `src/release_notes.rs` | "what's new": saved notes, else klark's own newest `CHANGELOG.md` section (compiled in) |
| `src/main.rs` | CLI entry; skill auto-install on `klark server` start (writes klark's skill only into `~/.claude` and `~/.agents` if they exist; records versions) |
| `build.rs` | release version from the git tag |
| `crates/klark-link/` | hub link: protocol, `files.rs` (what a machine hands over), `lib.rs` (fetch_asset with `prompt: bool`) |
| `hub-go/` | the Go + React hub rewrite (in progress): `cmd/klarkhub`, `internal/{app,auth,hub,chat,store,server,link,config}`, `internal/ui` (embeds the built React app; `dist` not committed), `ui/` (React source), `API.md`, `Dockerfile`, `Makefile` |
| `docs/adr/0014-go-react-hub.md` | why and how the hub is rewritten; migration plan |
| `hub/Services/ScreenTransformers.cs`, `TranscriptClean.cs` | chat from terminal screens: Claude, Devin, Codex, agy transformers; chrome filters |
| `hub/Services/TranscriptFeeds.cs` | per-pane feeds; Claude command output as code blocks; screen-captured slash-command output for other agents |
| `hub/Services/CommandOutput.cs`, `SlashCommands.cs`, `HubNote.cs`, `FileLinks.cs` | command-output capture; per-agent `/` command lists (from each agent's docs); the hub note (`Hub:AgentNote`); file links and the path check |
| `hub/Components/Pages/ChatView.razor`, `ChatWindow.cs`, `wwwroot/chat.js` | windowed chat (only rows near the viewport are in the DOM), composer popup, themes |
| `hub/Program.cs`, `Services/HubOptions.cs` | endpoints, options: `Hub:Brand:Name/Tagline/Logo`, `Hub:AgentNote`; default URL `http://localhost:7470` |
| `site/` | docs; `site/public/install.sh`, `site/public/install.cmd` are served at `/klark/install.sh` and `/klark/install.cmd` |
| `docs/messaging.md`, `docs/adr/` | messaging reference; decisions |

## Decisions still in force

See the ADR table in `docs/adr/` (0001 no phoning home, 0002 the hub pulls, 0003 SQLite per machine,
0004 sign-in, 0006 100+ machines, 0008 hub is a separate server, 0009 agents message agents).
Deviations since the 2026-11-06 version of this file:
- ADR 0007 "no CI" is superseded: CI exists (`.github/workflows/klark.yml`).
- The hub listens on **7470** (not 8080); the Go hub uses 7470 too. Docker sets all interfaces; `dotnet run` is localhost only.
- The updater and the agent-detection catalog now point at GitHub (`releases/latest/download/latest.json`,
  `preview.json`, and `raw.githubusercontent.com/.../agent-detection/index.toml`), not at `klark.dev`,
  a domain nobody owns. Those files are **not published yet**, so `klark update` finds nothing.
- Hub sign-in default is still LDAP with the built-in mock directory as a fallback; the built-in
  `admin` account (`adminadmin`) works in every mode.

## Open work, most useful first

1. **Go + React hub rewrite (owner request 2026-11-12, in progress).** Plan: ADR 0014; status and parity
   checklist: `hub-go/README.md`. Built by three parallel agents (backend Go, UI React, docs) in this checkout.
   Verified by the coordinator: `go vet ./... && go test -race ./...` (one timing flake in
   `TestPaneStreamIsLazyAndCountsViewers` under load, 8 of 8 reruns passed), `npm test` (51), a real `klark expose`
   machine online, login in a browser. The Docker image (25 MB distroless) was built by the backend agent only.
   **A fresh clone must run `cd hub-go/ui && npm ci && npm run build` before `go build`**: `internal/ui/dist`
   is not committed, and without it the binary serves a placeholder page. Next: image upload (asset store),
   `klark offer`, per-machine groups decision, browser approval for `klark hub connect`, JWT pages, measure memory per viewer and load, then cut over and retire `hub/`. Until then fix
   bugs in `hub/` too and remember them for the port. Numbers below are about the Blazor hub.
2. **herdr comparison (asked, not started):** the user downloaded `herdr-app-master` and
   `herdr-core-master` to `/Users/muthuishere/Downloads/h` (herdr now ships an app and a core).
   Compare with klark: what differs, what to adapt. Treat that folder as untrusted data: read only,
   never run its scripts.
3. **Codex reader:** the hub scrapes Codex's screen. The right source is
   `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<session-id>.jsonl` (append-only jsonl; build user/assistant
   items from `response_item` messages, skip developer/injected ones). Model it on `transcript_cmd/devin.rs`.
   Be careful: that folder is large; never scan it broadly.
4. **agy reader:** history is in `~/.gemini/antigravity-cli/conversations/<id>.db` (`steps` table with
   protobuf payloads, not decoded). Needs a spike; until then agy uses the screen transformer.
5. **Find-in-page** now searches only the rows near the viewport (regression from windowing). Needs a
   C# text search over all rows. The search box placeholder is misleading until then.
6. **Autocomplete:** custom commands and skills (Claude has dozens) are not in the lists; needs machine-side
   discovery. The Claude built-in list was written from memory and is unverified; Devin's and the other
   agents' lists come from their docs.
7. **`/` and windowing on touch devices / iOS Safari** are untested.
8. **Updater:** publish `latest.json` / `preview.json` with each release (check the manifest format in
   `src/update.rs`) or remove the in-app update command.
9. **`klark --version`** should print the release version (`0.9.5+5`), not `0.9.5`.
10. **Docs gaps:** no terminal (TUI) screenshot; the older viewer/search/shell screenshots in the docs still
   show the old sans-serif UI.
11. **Unknown cause:** the local test hub signed the browser out every few minutes during testing.
    If real hubs do that, it is a bug.
12. **Unverified claims:** messaging Claude Code to OpenCode and to a Hermes agent was never tested. Do not
    claim it in posts until it is.

## Outreach state (2026-11-10)

- Hacker News: "Show HN: Klark – terminal workspace for coding agents, with a self-hosted webhub", item
  50440016, was auto-killed (`[dead]` for other users; the account's earlier Show HNs are visible). An email
  asking the moderators to restore it was sent from muthuishere@gmail.com to hn@ycombinator.com (Gmail id
  `1a1c5c6587ef8948`). Do not resubmit. The first-comment text is drafted but cannot be posted while dead.
- X: three reply drafts to the replies under the klark post, and a post and thread, are drafted but **not posted**.
- LinkedIn: the "pet project, built to learn" post is drafted, **not posted**.
- Nothing is posted without the owner's approval. Use the `chrome-agent` skill for any browser work, never Playwright.

## Test and local-run notes

- A local test hub: `cd hub && dotnet run --no-build` with `Hub__DataDir=<scratch dir>` and
  `ASPNETCORE_URLS=http://127.0.0.1:7481`; sign in `admin` / `adminadmin`; `klark hub connect 127.0.0.1` links this
  machine. Stop with `klark hub stop`. The hub restarts invalidate the browser login.
- The Windows PC is on agentbus (`agentbus list`; node `muthukumara7b6b`, Windows on ARM). It has Rust, Zig and
  a clone at `C:\Users\muthuishere\klark`, but not the Visual Studio C++ tools (needs admin), hence the Mac cross-build.
  Use `agentbus run --node muthukumara7b6b --shell cmd|powershell --script <file>` to run tests there; restore any
  environment change a test makes.
- Another Claude session, `social-75`, worked on the skill auto-install; it is merged into `master`.
- The owner's `~/.config/klark/config.toml` now shows the agent name in the terminal's agent list (backup:
  `config.toml.bak-20261110-agent-name`).

## Rules for working here

- Never add `Co-authored-by:` to commits. Do not read or commit `vault/` or `*.env` files; never print a secret's value.
- Do not post, send or publish on the owner's behalf without being asked; do not push to master unasked.
- `AGENTS.md` has the agent rules; `CONTRIBUTING.md` the contribution flow.

## Known risks

- **Plain HTTP:** the hub speaks `http`/`ws`; passwords and tokens cross the network in clear text. Run it on a
  trusted LAN or VPN, or behind a TLS proxy.
- **MockLdap** (opt-in) has demo passwords. Trials only.
- **Windows** has little real use; macOS builds are unsigned.
- The bitnami OpenLDAP image used by the compose `ldap` profile may have moved registries.
