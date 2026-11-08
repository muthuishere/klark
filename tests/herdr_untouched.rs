//! klark must never touch herdr's config folder: herdr may be running from it.

use std::fs;
use std::process::Command;

#[test]
fn klark_leaves_a_sibling_herdr_config_alone() {
    let base = std::env::temp_dir().join(format!("klark-herdr-{}", std::process::id()));
    let herdr = base.join("herdr");
    fs::create_dir_all(herdr.join("sessions/work")).unwrap();
    fs::write(herdr.join("config.toml"), "onboarding = false\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_klark"))
        .env("XDG_CONFIG_HOME", &base)
        .args(["session", "list"])
        .output()
        .unwrap();
    assert!(herdr.join("config.toml").is_file(), "{out:?}");
    assert!(herdr.join("sessions/work").is_dir());
    let _ = fs::remove_dir_all(&base);
}
