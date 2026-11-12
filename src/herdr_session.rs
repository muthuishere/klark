//! Move a session between herdr (classic, `session.json` version 3) and klark.
//!
//! klark was forked from herdr 0.9.5 and kept the file format, so this works on the JSON
//! directly. Reading herdr's files is read-only; export writes only the file you name.
//! The new herdr app/core keeps its structure in the core and has no `session.json`.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SUPPORTED_VERSION: u64 = 3;

fn herdr_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("herdr")
}

fn same_folder(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The workspaces of one herdr `session.json` whose folder is `cwd`.
fn matching_workspaces(content: &str, cwd: &Path) -> Result<Vec<Value>, String> {
    let doc: Value =
        serde_json::from_str(content).map_err(|e| format!("not a session file: {e}"))?;
    if doc.get("version").and_then(Value::as_u64).unwrap_or(0) > SUPPORTED_VERSION {
        return Err("this session file is from a newer herdr than klark understands".into());
    }
    let workspaces = doc
        .get("workspaces")
        .and_then(Value::as_array)
        .ok_or("no workspaces in the session file")?;
    Ok(workspaces
        .iter()
        .filter(|w| {
            w.get("identity_cwd")
                .and_then(Value::as_str)
                .is_some_and(|p| same_folder(Path::new(p), cwd))
        })
        .cloned()
        .collect())
}

fn build_snapshot(workspaces: Vec<Value>) -> Value {
    json!({
        "version": SUPPORTED_VERSION,
        "workspaces": workspaces,
        "active": 0,
        "selected": 0,
        "sidebar_width": null,
        "sidebar_section_split": null,
        "collapsed_space_keys": [],
    })
}

/// herdr's file is newer than klark's. An unknown time on either side counts as newer, so a
/// session on a filesystem without modified times can still be imported.
fn is_newer(herdr: Option<std::time::SystemTime>, klark: Option<std::time::SystemTime>) -> bool {
    match (herdr, klark) {
        (Some(h), Some(k)) => h > k,
        _ => true,
    }
}

fn herdr_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = vec![dir.join("session.json")];
    if let Ok(entries) = std::fs::read_dir(dir.join("sessions")) {
        let mut named: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("session.json"))
            .collect();
        named.sort();
        files.extend(named);
    }
    files.into_iter().filter(|p| p.is_file()).collect()
}

fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

struct ImportOptions {
    from: PathBuf,
    cwd: PathBuf,
    name: Option<String>,
    force: bool,
}

fn parse_import(args: &[String]) -> Result<ImportOptions, String> {
    let mut options = ImportOptions {
        from: herdr_dir(),
        cwd: std::env::current_dir().map_err(|e| format!("cannot read the current folder: {e}"))?,
        name: None,
        force: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--from" => options.from = PathBuf::from(value("--from")?),
            "--cwd" => options.cwd = PathBuf::from(value("--cwd")?),
            "--name" => options.name = Some(value("--name")?),
            "--force" => options.force = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(options)
}

/// `klark session import-herdr [--from <herdr config dir>] [--cwd <folder>] [--name <session>] [--force]`
pub fn run_import(args: &[String]) -> i32 {
    match import(args) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(err) => {
            eprintln!("klark session import-herdr: {err}");
            1
        }
    }
}

fn import(args: &[String]) -> Result<String, String> {
    let options = parse_import(args)?;
    let mut found: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut newest_source: Option<std::time::SystemTime> = None;
    for file in herdr_session_files(&options.from) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let matched = matching_workspaces(&content, &options.cwd)?;
        if !matched.is_empty() {
            let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            newest_source = newest_source.max(modified);
        }
        for workspace in matched {
            let id = workspace
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if seen.insert(id) {
                found.push(workspace);
            }
        }
    }
    if found.is_empty() {
        return Err(format!(
            "no herdr workspace for {} under {}",
            options.cwd.display(),
            options.from.display()
        ));
    }
    let name = match options.name {
        Some(name) => name,
        None => crate::session::folder_to_session_name(&options.cwd)?,
    };
    let target = crate::session::data_dir_for(Some(&name)).join("session.json");
    // The latest one wins: an existing klark layout is replaced only when herdr's is newer.
    let backup = if target.exists() {
        let klark_time = std::fs::metadata(&target).and_then(|m| m.modified()).ok();
        if !options.force && !is_newer(newest_source, klark_time) {
            return Err(format!(
                "klark session '{name}' is newer than herdr's ({}); nothing changed. Use --force to replace it anyway",
                target.display()
            ));
        }
        let backup = target.with_extension("json.bak-before-herdr-import");
        std::fs::copy(&target, &backup)
            .map_err(|e| format!("cannot back up the old layout: {e}"))?;
        Some(backup)
    } else {
        None
    };
    let count = found.len();
    let body = serde_json::to_string_pretty(&build_snapshot(found)).map_err(|e| e.to_string())?;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomic(&target, body.as_bytes()).map_err(|e| e.to_string())?;
    let backed_up = backup.map_or(String::new(), |b| {
        format!("\nreplaced an older layout; kept it at {}", b.display())
    });
    Ok(format!(
        "imported {count} herdr workspace(s) into klark session '{name}'. Open it with: klark -s {name}\n\
         (layout and folders are restored; programs are started fresh){backed_up}"
    ))
}

/// `klark session export-herdr [--name <session>] [--out <file>]`
pub fn run_export(args: &[String]) -> i32 {
    match export(args) {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(err) => {
            eprintln!("klark session export-herdr: {err}");
            1
        }
    }
}

fn export(args: &[String]) -> Result<String, String> {
    let mut name: Option<String> = None;
    let mut out = PathBuf::from("herdr-session.json");
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().cloned().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--name" => name = Some(value("--name")?),
            "--out" => out = PathBuf::from(value("--out")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let source = crate::session::data_dir_for(name.as_deref()).join("session.json");
    let content = std::fs::read_to_string(&source)
        .map_err(|e| format!("no saved layout at {}: {e}", source.display()))?;
    let doc: Value =
        serde_json::from_str(&content).map_err(|e| format!("unreadable session file: {e}"))?;
    if doc.get("version").and_then(Value::as_u64) != Some(SUPPORTED_VERSION) {
        return Err("the saved layout is not in the version-3 format herdr reads".into());
    }
    std::fs::write(&out, content).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(format!(
        "wrote {}. To use it in herdr: stop that herdr session, then copy it to\n\
         ~/.config/herdr/session.json (or ~/.config/herdr/sessions/<name>/session.json) and start herdr.\n\
         klark does not write herdr's own files.",
        out.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":3,"workspaces":[
        {"id":"a","identity_cwd":"/work/api","tabs":[]},
        {"id":"b","identity_cwd":"/work/web","tabs":[]}],
        "active":0,"selected":0}"#;

    #[test]
    fn picks_only_the_workspaces_for_the_folder() {
        let found = matching_workspaces(SAMPLE, Path::new("/work/api")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["id"], "a");
        assert!(matching_workspaces(SAMPLE, Path::new("/work/none"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn the_latest_file_wins() {
        use std::time::{Duration, UNIX_EPOCH};
        let old = Some(UNIX_EPOCH + Duration::from_secs(100));
        let new = Some(UNIX_EPOCH + Duration::from_secs(200));
        assert!(is_newer(new, old));
        assert!(!is_newer(old, new));
        assert!(!is_newer(old, old));
        assert!(is_newer(None, old));
    }

    #[test]
    fn refuses_a_newer_format() {
        let err =
            matching_workspaces(r#"{"version":9,"workspaces":[]}"#, Path::new("/x")).unwrap_err();
        assert!(err.contains("newer"));
    }

    #[test]
    fn snapshot_is_version_three_with_the_required_fields() {
        let snap = build_snapshot(vec![json!({"id":"a"})]);
        assert_eq!(snap["version"], 3);
        assert_eq!(snap["workspaces"].as_array().unwrap().len(), 1);
        assert!(snap.get("selected").is_some() && snap.get("collapsed_space_keys").is_some());
    }
}
