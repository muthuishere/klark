# macOS permissions: asked once, kept across updates

## Why macOS kept asking

- **Each update looked like a new app.** macOS remembers privacy permissions per app
  identity. A klark binary had only the linker's ad-hoc signature, named after a hash
  that changes every build, so every update started over.
- **klark is asked, not the terminal.** The klark server runs outside the terminal app
  (so quitting the terminal does not kill your panes). macOS therefore holds klark
  responsible for everything its panes do: each agent or shell that opens a file in
  Documents, Desktop or Downloads triggered a prompt.

## What klark does now

1. **A built-in identity.** The macOS binary carries an Info.plist (`build.rs`): identifier
   `dev.klark.klark`, the name "Klark" in prompts, and the reason for each folder.
2. **A stable signature.** Releases are signed with a Developer ID certificate under that
   identifier, with the hardened runtime, and notarized (`scripts/macos_sign.sh`). macOS
   identifies klark by "identifier + team", which does not change between releases, so a
   permission given once stays.
3. **One install path.** The installer always writes `~/.local/bin/klark` (or `INSTALL_DIR`).
4. **Asked once, at first start.** The server checks for Full Disk Access when it starts.
   Without it, klark shows "allow klark once" and `klark setup permissions` opens
   System Settings at Full Disk Access, shows the binary in Finder to drag into the
   list, and waits until it is switched on. One switch covers every folder prompt.
   `klark setup permissions --never` stops the reminder.

Linux has no such prompts. Windows has no folder prompts.

## Check a binary

```sh
codesign -dv ~/.local/bin/klark      # Identifier=dev.klark.klark, TeamIdentifier set
codesign -d -r- ~/.local/bin/klark   # designated requirement: identifier + team
launchctl plist __TEXT,__info_plist ~/.local/bin/klark   # the built-in Info.plist
```

A binary built on a machine without the signing identity is ad-hoc signed: fine for development, but
macOS treats every build as new.

## Local builds

Every macOS build is signed at link time as `dev.klark.klark` with the Developer ID
in the keychain (`scripts/macos_link.sh`, set as the linker in `.cargo/config.toml`).
Debug, release and test builds therefore share the release's identity, and one Full
Disk Access switch covers them all, with no new "klark" entry per build.

Without a signing identity (CI, other machines) the build keeps the linker's ad-hoc
signature, as before. `KLARK_DEV_SIGN=0` skips signing; `KLARK_SIGN_IDENTITY` picks
another certificate.

Old "klark" entries left over from ad-hoc builds stay in System Settings until removed
there (select, then **–**): `tccutil` only resets app bundles, not bare binaries.
