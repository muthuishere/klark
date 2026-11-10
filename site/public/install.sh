#!/bin/sh
# klark installer for macOS and Linux.
#
#   curl -fsSL https://muthuishere.github.io/klark/install.sh | sh
#   wget -qO- https://muthuishere.github.io/klark/install.sh | sh
#
# Downloads the klark binary for this machine from the GitHub release, checks its
# sha256, and installs it. No root needed.
#
#   KLARK_VERSION=v0.9.5.5   install that release (default: the latest)
#   INSTALL_DIR=/usr/local/bin   where to put it (default: ~/.local/bin)
#
# Windows: use install.ps1 (https://muthuishere.github.io/klark/start/install/).
set -eu

REPO="muthuishere/klark"

say() { printf '%s\n' "$*"; }
die() { printf 'klark install: %s\n' "$*" >&2; exit 1; }

os=$(uname -s)
case "$os" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) die "unsupported system '$os'. On Windows run install.ps1 (see https://muthuishere.github.io/klark/start/install/)." ;;
esac

arch=$(uname -m)
case "$arch" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) die "unsupported CPU '$arch'. Builds exist for arm64 and x86_64; otherwise build from source." ;;
esac

file="klark-$os-$arch"
if [ -n "${KLARK_VERSION:-}" ]; then
  base="https://github.com/$REPO/releases/download/$KLARK_VERSION"
else
  base="https://github.com/$REPO/releases/latest/download"
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    die "need curl or wget"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "need sha256sum or shasum to check the download"
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $file ..."
fetch "$base/$file" "$tmp/$file" || die "download failed: $base/$file"
fetch "$base/$file.sha256" "$tmp/$file.sha256" || die "download failed: $base/$file.sha256"

want=$(cut -d' ' -f1 "$tmp/$file.sha256")
got=$(sha256_of "$tmp/$file")
[ "$want" = "$got" ] || die "checksum mismatch (expected $want, got $got); nothing installed"

dest="${INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$dest" || die "cannot create $dest (set INSTALL_DIR to a folder you can write)"
install -m 0755 "$tmp/$file" "$dest/klark" || die "cannot write $dest/klark"

# The binary is not signed: clear the download flag so macOS does not block the first run.
if [ "$os" = macos ]; then
  xattr -d com.apple.quarantine "$dest/klark" 2>/dev/null || true
fi

say "Installed: $dest/klark"
"$dest/klark" --version || true

case ":$PATH:" in
  *":$dest:"*) ;;
  *)
    say ""
    say "$dest is not on your PATH. Add it, for example:"
    say "  echo 'export PATH=\"$dest:\$PATH\"' >> ~/.zshrc   # or ~/.bashrc"
    ;;
esac
say ""
say "Run: klark"
