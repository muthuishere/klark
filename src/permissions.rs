//! macOS privacy permissions, asked for once.
//!
//! The klark server runs outside the terminal app (see
//! `platform::macos::launchd_spawn`), so macOS holds klark, not the terminal,
//! responsible for what every pane does: each agent or shell that opens a file
//! in Documents, Desktop or Downloads makes macOS ask again. One grant of Full
//! Disk Access covers all of those. With a signed klark (scripts/macos_sign.sh)
//! the grant survives updates.
//!
//! Only the server can tell whether klark has it (a CLI run in a terminal is
//! the terminal's responsibility), so the server checks at start and every 30
//! seconds until it is granted, and writes `<state>/permissions.json`.
//! `klark setup permissions` reads that file, opens the right System Settings
//! page and waits for the switch.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionsStatus {
    /// Whether the server has Full Disk Access. None when it cannot tell.
    #[serde(default)]
    pub full_disk_access: Option<bool>,
    #[serde(default)]
    pub checked_at: u64,
    /// The klark binary the server runs: the one to switch on in System Settings.
    #[serde(default)]
    pub binary: String,
    /// `klark setup permissions --never`: no more reminders at start.
    #[serde(default)]
    pub reminders_off: bool,
}

pub fn status_path() -> PathBuf {
    crate::config::state_dir().join("permissions.json")
}

pub fn read_status() -> PermissionsStatus {
    std::fs::read(status_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_status(st: &PermissionsStatus) {
    let path = status_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Ok(bytes) = serde_json::to_vec_pretty(st) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Whether this process has Full Disk Access: the user's privacy database can
/// only be opened with it. None off macOS or when the answer is unclear.
pub fn has_full_disk_access() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    let db = PathBuf::from(home).join("Library/Application Support/com.apple.TCC/TCC.db");
    match std::fs::File::open(&db) {
        Ok(_) => Some(true),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Some(false),
        // EPERM (operation not permitted) is how macOS refuses it.
        Err(e) if e.raw_os_error() == Some(1) => Some(false),
        Err(_) => None,
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Server start: check now, write the status, and keep checking every 30
/// seconds until it is granted. Returns true when a reminder should be shown
/// (Full Disk Access is missing and reminders are on). Does nothing off macOS.
pub(crate) fn watch_from_server() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let check = || {
        let mut st = read_status();
        st.full_disk_access = has_full_disk_access();
        st.checked_at = unix_secs();
        st.binary = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        write_status(&st);
        st
    };
    let st = check();
    if st.full_disk_access != Some(false) {
        return false;
    }
    let _ = std::thread::Builder::new()
        .name("klark-permissions".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            if check().full_disk_access != Some(false) {
                break;
            }
        });
    !st.reminders_off
}

const USAGE: &str = "\
klark setup permissions [--never | --remind]

macOS asks for permission whenever an agent or shell in a Klark pane opens files in
Documents, Desktop, Downloads and similar folders. Switching on Full Disk Access for
klark once stops those prompts; a signed klark keeps it across updates.

  klark setup permissions          show the state; if missing, open System Settings and wait
  klark setup permissions --never  stop the reminder at start
  klark setup permissions --remind turn the reminder back on";

/// `klark setup <what>`.
pub fn run_setup(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("permissions") => run_permissions(&args[1..]),
        None | Some("-h" | "--help" | "help") => {
            println!(
                "klark setup permissions   allow klark once instead of prompt after prompt (macOS)"
            );
            Ok(0)
        }
        Some(other) => {
            eprintln!("klark setup: unknown {other:?}; try klark setup permissions");
            Ok(2)
        }
    }
}

fn run_permissions(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return Ok(0);
        }
        Some("--never") => {
            let mut st = read_status();
            st.reminders_off = true;
            write_status(&st);
            println!("No more reminders at start. `klark setup permissions --remind` turns them back on.");
            return Ok(0);
        }
        Some("--remind") => {
            let mut st = read_status();
            st.reminders_off = false;
            write_status(&st);
            println!("Reminders at start are on.");
            return Ok(0);
        }
        Some(other) => {
            eprintln!("klark setup permissions: unknown {other:?}\n\n{USAGE}");
            return Ok(2);
        }
        None => {}
    }
    if !cfg!(target_os = "macos") {
        println!("Nothing to set up: only macOS asks for these permissions.");
        return Ok(0);
    }
    let st = read_status();
    let binary = if st.binary.is_empty() {
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "klark".into())
    } else {
        st.binary.clone()
    };
    match st.full_disk_access {
        Some(true) => {
            println!("klark has Full Disk Access ({binary}). Nothing to do.");
            return Ok(0);
        }
        None if st.checked_at == 0 => {
            println!(
                "No klark server has checked yet. Start klark (or `klark server`), then run this again."
            );
            return Ok(1);
        }
        _ => {}
    }
    println!("klark does not have Full Disk Access yet, so macOS keeps asking for folders.");
    println!();
    println!("  1. System Settings opens at Privacy & Security > Full Disk Access.");
    println!("  2. Finder shows the klark binary: drag it into the list (or press + and pick it):");
    println!("       {binary}");
    println!("  3. Make sure its switch is on.");
    println!();
    open_settings(&binary);
    println!("Waiting for the switch (Ctrl-C to stop; the klark server re-checks every 30 s)...");
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    let started = unix_secs();
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(2));
        let now = read_status();
        if now.full_disk_access == Some(true) {
            println!("Done: klark has Full Disk Access. The folder prompts stop.");
            return Ok(0);
        }
        // A server that has stopped writes no news.
        if now.checked_at + 90 < unix_secs() && started + 90 < unix_secs() {
            println!("No klark server is checking any more; start klark and run this again.");
            return Ok(1);
        }
    }
    println!("Not granted yet. Run `klark setup permissions` again after switching it on.");
    Ok(1)
}

fn open_settings(binary: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .status();
    let _ = std::process::Command::new("open")
        .args(["-R", binary])
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips_and_tolerates_old_files() {
        let st = PermissionsStatus {
            full_disk_access: Some(false),
            checked_at: 5,
            binary: "/x/klark".into(),
            reminders_off: true,
        };
        let back: PermissionsStatus =
            serde_json::from_slice(&serde_json::to_vec(&st).unwrap()).unwrap();
        assert_eq!(back, st);
        let old: PermissionsStatus = serde_json::from_str("{}").unwrap();
        assert_eq!(old.full_disk_access, None);
        assert!(!old.reminders_off);
    }

    #[test]
    fn probe_answers_on_macos_only() {
        assert_eq!(has_full_disk_access().is_some(), cfg!(target_os = "macos"));
    }
}
