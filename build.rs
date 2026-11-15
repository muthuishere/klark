use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=KLARK_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=KLARK_BUILD_ID");
    println!("cargo:rerun-if-env-changed=KLARK_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=KLARK_RELEASE_VERSION");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-changed=.git/HEAD");

    // The klark release version comes from the release tag (v0.9.5.4 -> 0.9.5.4),
    // not from Cargo.toml. KLARK_RELEASE_VERSION overrides it; a build with no tag
    // and no override is 0.0.0-dev.
    let version = std::env::var("KLARK_RELEASE_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(tag_version)
        .map(|v| to_semver_build(&v))
        .unwrap_or_else(|| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=KLARK_RELEASE_VERSION={version}");
    embed_macos_info_plist(&version);
}

/// macOS keeps privacy permissions per app identity. A bare binary has no
/// bundle identifier, so it is named after a hash that changes every build and
/// each update looks like a new app. An Info.plist linked into the binary gives
/// it a stable identifier, a name for the permission prompts and the reasons
/// they show. Signing (scripts/macos_sign.sh) uses the same identifier.
fn embed_macos_info_plist(version: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let short = version.split(['+', '-']).next().unwrap_or(version);
    let why = "Agents and shells running in Klark panes work with files in your";
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleName</key><string>Klark</string>
  <key>CFBundleDisplayName</key><string>Klark</string>
  <key>CFBundleExecutable</key><string>klark</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>{short}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>NSDocumentsFolderUsageDescription</key><string>{why} Documents folder.</string>
  <key>NSDesktopFolderUsageDescription</key><string>{why} Desktop folder.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>{why} Downloads folder.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>{why} network volumes.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>{why} removable volumes.</string>
  <key>NSAppleEventsUsageDescription</key><string>Agents and shells running in Klark panes may control other apps when you ask them to.</string>
</dict>
</plist>
"#,
        id = MACOS_BUNDLE_ID,
        version = xml_escape(version),
        short = xml_escape(short),
    );
    let path = std::path::Path::new(&out_dir).join("Info.plist");
    if std::fs::write(&path, plist).is_err() {
        return;
    }
    println!(
        "cargo:rustc-link-arg-bins=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        path.display()
    );
}

const MACOS_BUNDLE_ID: &str = "dev.klark.klark";

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn tag_version() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--match", "v*", "--abbrev=0"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = String::from_utf8(out.stdout).ok()?;
    let tag = tag.trim().trim_start_matches('v');
    (!tag.is_empty()).then(|| tag.to_string())
}

/// Semver allows only MAJOR.MINOR.PATCH, so the release's fourth number goes in
/// build metadata: `0.9.5.4` -> `0.9.5+4`. Anything else is kept as it is.
fn to_semver_build(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u64>().is_ok()) {
        format!("{}.{}.{}+{}", parts[0], parts[1], parts[2], parts[3])
    } else {
        version.to_string()
    }
}
