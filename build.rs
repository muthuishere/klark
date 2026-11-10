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
