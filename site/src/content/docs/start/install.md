---
title: Install
description: Build klark from source and put it on your PATH.
---

klark is one Rust binary. There are no published release binaries for klark yet,
so you build it from source.

## Requirements

- Rust (stable) and Cargo.
- **Zig 0.16**, for the vendored `libghostty-vt` terminal engine.
- macOS or Linux. Windows code exists (ConPTY) but has not been run as klark.

## Build

```sh
git clone https://github.com/muthuishere/klark
cd klark
cargo build --release
```

The binary is `target/release/klark`. Copy it somewhere on your `PATH`:

```sh
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

Moving from herdr: on first run klark moves `~/.config/herdr` to
`~/.config/klark`. Environment variables are now `KLARK_*`, for example
`KLARK_SOCKET_PATH`.

## The hub is separate

The web hub (klarkhub) is a separate server in `hub/`. You do not need it to use
klark in a terminal. See [Run the hub](/hub/run/).
