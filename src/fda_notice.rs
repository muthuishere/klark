#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
//! One-time macOS notice: without Full Disk Access, macOS asks for every folder and
//! volume separately ("klark would like to access Documents ..."). Shown once, only
//! when access is missing, only on an interactive terminal.

use std::path::Path;

const MARKER: &str = "fda-notice-shown";

/// Show the notice when it has not been shown yet and access is missing.
pub fn maybe_show() {
    #[cfg(target_os = "macos")]
    {
        use std::io::IsTerminal;
        if !std::io::stderr().is_terminal() {
            return;
        }
        let marker = crate::config::config_dir().join(MARKER);
        if marker.exists() {
            return;
        }
        let granted = has_full_disk_access();
        if should_show(&marker, granted) {
            eprintln!("{}", notice_text());
        }
        // Granted or told: do not ask again.
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, b"1\n");
    }
}

fn should_show(marker: &Path, granted: bool) -> bool {
    !granted && !marker.exists()
}

/// The system TCC database can only be opened with Full Disk Access; a denial never prompts.
#[cfg(target_os = "macos")]
fn has_full_disk_access() -> bool {
    std::fs::File::open("/Library/Application Support/com.apple.TCC/TCC.db").is_ok()
}

fn notice_text() -> &'static str {
    "klark: first run. macOS may ask for access to Documents, Downloads and each volume one by one.\n\
     To stop that, grant Full Disk Access once to your terminal app (and klark):\n\
     \x20 open \"x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles\"\n\
     then add the app, quit and reopen the terminal, and run `klark server stop` so the server restarts with it.\n\
     (shown once)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_only_when_missing_and_not_shown_before() {
        let dir = std::env::temp_dir().join(format!("klark-fda-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join(MARKER);
        assert!(should_show(&marker, false));
        assert!(!should_show(&marker, true));
        std::fs::write(&marker, b"1").unwrap();
        assert!(!should_show(&marker, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notice_names_the_setting_and_the_restart() {
        let text = notice_text();
        assert!(text.contains("Full Disk Access"));
        assert!(text.contains("klark server stop"));
    }
}
