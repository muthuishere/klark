#!/usr/bin/env bash
# Release everything under one number: klark (v<V>, binaries + klarkhub image), the hub app, the Go module (hub-go/v<V>) and the UI
# package (hub-ui-v<V>). Usage: scripts/release.sh 0.10.1   (run on a clean, pushed master)
set -euo pipefail
V=${1:?usage: scripts/release.sh <version, e.g. 0.10.1>}
[[ $V =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z" >&2; exit 1; }
ROOT=$(git rev-parse --show-toplevel); cd "$ROOT"
[[ -z $(git status --porcelain) ]] || { echo "working tree not clean" >&2; exit 1; }
[[ $(git branch --show-current) == master ]] || { echo "not on master" >&2; exit 1; }

# 1. one number everywhere
perl -0pi -e 's/^version = ".*?"/version = "'"$V"'"/m' Cargo.toml
cargo update -p klark --offline >/dev/null 2>&1 || cargo metadata --format-version 1 >/dev/null
UI=hub-go/ui/packages/hub-ui
(cd "$UI" && npm pkg set version="$V")
(cd hub-go/ui && npm pkg set version="$V")   # the hub app (klarkhub); its binary gets V via the image build
git add Cargo.toml Cargo.lock; git add -u hub-go/ui
git diff --cached --quiet || git commit -qm "release $V"

# 2. tags on the same commit
git tag -a "v$V" -m "klark $V"
git tag "hub-go/v$V"

# 3. the UI package: built files on the hub-ui-release branch, tagged hub-ui-v<V>
(cd "$UI" && npm run build >/dev/null)
W=$(mktemp -d)
git worktree add -q "$W" hub-ui-release
rsync -a --delete --exclude .git "$UI/release/" "$W/"
(cd "$W" && git add -A && { git diff --cached --quiet || git commit -qm "hub-ui $V"; } && git tag "hub-ui-v$V")
git worktree remove --force "$W"

# 4. push: master, branch, tags (the v tag starts the binary/Docker release)
git push -q origin master hub-ui-release "hub-go/v$V" "hub-ui-v$V" "v$V"
echo "released $V: v$V, hub-go/v$V, hub-ui-v$V"
