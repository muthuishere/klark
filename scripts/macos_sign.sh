#!/usr/bin/env bash
# Signs macOS klark binaries with a Developer ID certificate under the fixed
# identifier dev.klark.klark, with the hardened runtime and a secure timestamp,
# then has Apple notarize them.
#
# Why: macOS remembers privacy permissions (Documents, Desktop, Downloads, Full
# Disk Access, ...) per app identity. An unsigned or ad-hoc signed binary gets a
# new identity on every build, so every update asks again. A Developer ID
# signature with a fixed identifier keeps the same identity across releases.
#
# Usage: scripts/macos_sign.sh <binary>...
#   KLARK_SIGN_IDENTITY   certificate to use (default: the keychain's "Developer ID Application")
#   KLARK_NOTARIZE=0      sign only
#   KLARK_NOTARY_PROFILE  a `xcrun notarytool store-credentials` keychain profile; else the
#                         App Store Connect API key from ~/.config/apple/applecert.conf
#                         (ASC_ISSUER_ID, ASC_KEY_ID, ASC_PRIVATE_KEY = path to the .p8)
set -euo pipefail

BUNDLE_ID="dev.klark.klark"
IDENTITY="${KLARK_SIGN_IDENTITY:-Developer ID Application}"

[[ $# -ge 1 ]] || { echo "usage: $0 <binary>..." >&2; exit 2; }
[[ "$(uname -s)" == Darwin ]] || { echo "macOS only" >&2; exit 2; }

for bin in "$@"; do
  [[ -f "$bin" ]] || { echo "no such file: $bin" >&2; exit 1; }
  # The Info.plist built into the binary (build.rs) must carry the same identifier.
  if ! launchctl plist __TEXT,__info_plist "$bin" 2>/dev/null | grep -q "\"$BUNDLE_ID\""; then
    echo "$bin: no embedded Info.plist with $BUNDLE_ID (built by an older build.rs?)" >&2
    exit 1
  fi
  codesign --force --timestamp --options runtime \
    --identifier "$BUNDLE_ID" --sign "$IDENTITY" "$bin"
  codesign --verify --strict --verbose=2 "$bin"
  echo "$bin: $(codesign -d -r- "$bin" 2>&1 | sed -n 's/^designated => /designated requirement: /p')"
done

[[ "${KLARK_NOTARIZE:-1}" == 0 ]] && exit 0

# Apple notarizes a zip; the ticket is looked up online when the binary is
# first checked (a bare binary cannot be stapled).
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
zip_path="$work/klark-notarize.zip"
ditto -c -k --keepParent "$@" "$zip_path" 2>/dev/null || {
  # several files: zip them side by side
  mkdir "$work/files"; cp "$@" "$work/files/"; ditto -c -k "$work/files" "$zip_path"
}

if [[ -n "${KLARK_NOTARY_PROFILE:-}" ]]; then
  xcrun notarytool submit "$zip_path" --keychain-profile "$KLARK_NOTARY_PROFILE" --wait
else
  conf="$HOME/.config/apple/applecert.conf"
  [[ -f "$conf" ]] || { echo "no KLARK_NOTARY_PROFILE and no $conf" >&2; exit 1; }
  # Read in a subshell so nothing from the file is printed or left in this shell.
  (
    # shellcheck disable=SC1090
    . "$conf"
    [[ -n "${ASC_ISSUER_ID:-}" && -n "${ASC_KEY_ID:-}" ]] || { echo "$conf: ASC_ISSUER_ID / ASC_KEY_ID missing" >&2; exit 1; }
    key="${ASC_PRIVATE_KEY:-}"
    key="${key/#\~/$HOME}"
    [[ -f "$key" ]] || { echo "$conf: ASC_PRIVATE_KEY must be the path of the .p8 file" >&2; exit 1; }
    xcrun notarytool submit "$zip_path" --issuer "$ASC_ISSUER_ID" --key-id "$ASC_KEY_ID" --key "$key" --wait
  )
fi
echo "signed and notarized: $*"
