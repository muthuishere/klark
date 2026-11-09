---
title: Install
description: Download klark from the GitHub release, or build it from source.
---

klark is one binary. Download it from the
[latest release](https://github.com/muthuishere/klark/releases/latest), or build it from source.

## Download

| platform | file |
|---|---|
| macOS, Apple Silicon | `klark-macos-aarch64` |
| Linux x86_64 | `klark-linux-x86_64` |
| Linux arm64 | `klark-linux-aarch64` |

Windows and Intel Macs have no download yet; build from source.

```sh
# pick the file for your machine from the table
f=klark-macos-aarch64
curl -fLO https://github.com/muthuishere/klark/releases/latest/download/$f
curl -fLO https://github.com/muthuishere/klark/releases/latest/download/$f.sha256
shasum -a 256 -c $f.sha256
install -m 0755 $f ~/.local/bin/klark
klark --version
```

On macOS the binary is not signed. If macOS refuses to open it, clear the download flag:
`xattr -d com.apple.quarantine ~/.local/bin/klark`.

## Build from source

Requirements:

- Rust (stable) and Cargo.
- **Zig 0.16**, for the vendored `libghostty-vt` terminal engine.
- macOS or Linux. Windows code exists (ConPTY) but has not been run as klark.

```sh
git clone https://github.com/muthuishere/klark
cd klark
cargo build --release
install -m 0755 target/release/klark ~/.local/bin/klark
klark --version
```

A debug build (`cargo build`) works too. It keeps its config and sockets in
`~/.config/klark-dev` instead of `~/.config/klark`, so it never touches the
sessions of a release build.

## Run it

```sh
klark
```

This starts the background server if it is not running, and attaches to the
default session. Detach with `ctrl+b q`. The panes keep running.

```sh
klark status       # client and server status
klark --help       # every command
```

## Where things live

| what | path |
|---|---|
| config | `~/.config/klark/config.toml` (`KLARK_CONFIG_PATH` overrides) |
| logs | `~/.config/klark/klark.log`, `klark-client.log`, `klark-server.log` |
| saved hubs | `~/.config/klark/hubs.json` (mode 0600) |
| default config | `klark --default-config` prints it |

Coming from herdr: klark never touches `~/.config/herdr`, so herdr keeps running
alongside it. klark's own settings live in `~/.config/klark`, and its environment
variables are `KLARK_*`, for example `KLARK_SOCKET_PATH`.

## The hub is separate

The web hub (klarkhub) is a separate server in `hub/`. You do not need it to use
klark in a terminal. See [Run the hub](/hub/run/).
