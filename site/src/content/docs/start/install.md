---
title: Install
description: Download klark from the GitHub release, or build it from source.
---

klark is one binary. The quickest way to get it is one line.

## One line

macOS and Linux, with curl or wget:

```sh
curl -fsSL https://muthuishere.github.io/klark/install.sh | sh
```

```sh
wget -qO- https://muthuishere.github.io/klark/install.sh | sh
```

Windows, in Command Prompt (`curl.exe` ships with Windows 10 and later):

```bat
curl -fsSL https://muthuishere.github.io/klark/install.cmd -o "%TEMP%\klark-install.cmd" && "%TEMP%\klark-install.cmd"
```

In PowerShell, run the same thing through `cmd`:

```powershell
cmd /c "curl.exe -fsSL https://muthuishere.github.io/klark/install.cmd -o %TEMP%\klark-install.cmd && %TEMP%\klark-install.cmd"
```

Each script picks the build for your machine from the
[latest GitHub release](https://github.com/muthuishere/klark/releases/latest), checks its
sha256, installs it without admin or root and tells you if the folder is not on your PATH. The
Windows script adds it to your user PATH (not the machine PATH) and needs no admin.

Options, set before the command:

| setting | meaning | default |
|---|---|---|
| `KLARK_VERSION=v0.9.5.4` | install that release | the latest |
| `INSTALL_DIR=/usr/local/bin` | where to put klark | `~/.local/bin`; on Windows `%LOCALAPPDATA%\klark\bin` |

macOS and Linux: `curl -fsSL https://muthuishere.github.io/klark/install.sh | KLARK_VERSION=v0.9.5.4 sh`.
Windows: run `set KLARK_VERSION=v0.9.5.4` (and `set INSTALL_DIR=C:\tools` if you want) on their own lines first, then the command above.

Prefer to read a script before you run it? It is plain text:
[install.sh](https://muthuishere.github.io/klark/install.sh) and
[install.cmd](https://muthuishere.github.io/klark/install.cmd). Then see the
[Quick start](/start/quickstart/) for your first session.

## Download from the latest GitHub release

| platform | file |
|---|---|
| macOS, Apple Silicon | `klark-macos-aarch64` |
| macOS, Intel | `klark-macos-x86_64` |
| Linux x86_64 | `klark-linux-x86_64` |
| Linux arm64 | `klark-linux-aarch64` |
| Windows x64 (also runs on Windows on ARM) | `klark-windows-x86_64.exe` |

The Windows build is new: it starts and runs on Windows 11, but klark's Windows
support has had little real use. Please report problems.

```sh
# pick the file for your machine from the table
f=klark-macos-aarch64
curl -fLO https://github.com/muthuishere/klark/releases/latest/download/$f
curl -fLO https://github.com/muthuishere/klark/releases/latest/download/$f.sha256
shasum -a 256 -c $f.sha256
install -m 0755 $f ~/.local/bin/klark
klark --version
```

On Windows, download `klark-windows-x86_64.exe` from the release page and rename it `klark.exe`.

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

## Start klark in a terminal

```sh
klark
```

This starts the background server if it is not running, and attaches to the
default session. Start an agent (`claude`, `devin`, `codex`, ...) in a pane.
**Detach with `ctrl+b q`**: the panes keep running, and `klark` attaches again.

```sh
klark status       # local client and running server status
klark server       # run the server in the foreground, no UI (for a service or a headless box)
klark server stop  # stop the running server
klark --help       # every command
```

`klark -s` gives each project folder its own session: see [Sessions](/start/sessions/).

## Next: the hub, if you want a browser view

klark works fully in a terminal. To watch and answer your agents from a browser
or a phone, [run the hub](/hub/run/) and [connect this machine](/hub/machines/).

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
