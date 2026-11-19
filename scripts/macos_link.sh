#!/bin/sh
# Linker for macOS targets (.cargo/config.toml). Links with cc as usual, then
# signs the klark binary with the Developer ID under the fixed identifier
# dev.klark.klark, the identity releases carry.
#
# Why: macOS keys privacy permissions (Full Disk Access, folder prompts) to a
# binary's identity. The linker's ad-hoc signature is named after a hash that
# changes every build, so every debug or local build showed up as another
# "klark" in System Settings and asked again. Signed here, every build (debug,
# release, tests) is the same app to macOS: one switch covers them all.
#
# Skipped quietly when no signing identity is in the keychain (CI, other
# machines) or with KLARK_DEV_SIGN=0. Only binaries carrying klark's embedded
# Info.plist (build.rs) are signed.
set -e
cc "$@"

[ "${KLARK_DEV_SIGN:-1}" = 0 ] && exit 0
out=""
prev=""
for arg in "$@"; do
  [ "$prev" = "-o" ] && out="$arg"
  prev="$arg"
done
[ -n "$out" ] && [ -f "$out" ] || exit 0
launchctl plist __TEXT,__info_plist "$out" 2>/dev/null | grep -q '"dev.klark.klark"' || exit 0

identity="${KLARK_SIGN_IDENTITY:-Developer ID Application}"
security find-identity -v -p codesigning 2>/dev/null | grep -qF "$identity" || exit 0
if ! codesign --force --timestamp=none --identifier dev.klark.klark --sign "$identity" "$out" 2>/dev/null; then
  echo "warning: macos_link.sh: could not sign $out with \"$identity\"; macOS will treat it as a new app" >&2
fi
exit 0
