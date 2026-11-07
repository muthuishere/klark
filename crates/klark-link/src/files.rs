//! `get_file` (protocol v4): the hub asks for a file a chat linked to, and the
//! machine decides on its own whether to hand it over. The hub runs the same
//! checks, but this side is the one that holds the file, so it never trusts
//! the hub's judgement: only files under the pane's working directory or the
//! user's home, never anything credential-shaped, never more than the limit.

use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{json, Value};

/// Default (and maximum) size of a file the hub may fetch: 10 MB.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// The limit in force: `KLARK_FILES_MAX_BYTES` may lower it, never raise it.
pub fn max_bytes(requested: Option<u64>) -> u64 {
    let env = std::env::var("KLARK_FILES_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());
    [Some(MAX_FILE_BYTES), env, requested]
        .into_iter()
        .flatten()
        .filter(|n| *n > 0)
        .min()
        .unwrap_or(MAX_FILE_BYTES)
}

/// Whether a path looks like a credential (ported from herdr-expose
/// `secretShaped`, plus names containing credentials/secret/token). Shared by
/// `klark offer` and `get_file`.
pub fn credential_like(path: &Path) -> bool {
    let lp = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let lp = format!("/{}", lp.trim_start_matches('/'));
    for dir in [
        "/.ssh/",
        "/.aws/",
        "/.gnupg/",
        "/.kube/",
        "/.docker/",
        "/.config/gcloud/",
        "/vault/",
        "/.password-store/",
    ] {
        if lp.contains(dir) {
            return true;
        }
    }
    let base = lp.rsplit('/').next().unwrap_or("");
    base == ".env"
        || base.ends_with(".env")
        || base.starts_with(".env.")
        || base.starts_with("id_")
        || [".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|s| base.ends_with(s))
        || [".netrc", ".npmrc", ".pypirc"].contains(&base)
        || ["credential", "secret", "token"]
            .iter()
            .any(|w| base.contains(w))
}

/// "12.3 MB" for a refusal message.
pub fn mb(n: u64) -> String {
    let v = n as f64 / 1_048_576.0;
    if v >= 10.0 || v.fract() == 0.0 {
        format!("{v:.0} MB")
    } else {
        format!("{v:.1} MB")
    }
}

/// Resolve `asked` (absolute, `~/…` or relative to `cwd`; a trailing
/// `:line[:col]` is ignored) and check it may be handed over. Returns the
/// canonical path and its size.
pub fn check(
    asked: &str,
    cwd: &Path,
    home: Option<&Path>,
    max: u64,
) -> Result<(PathBuf, u64), String> {
    let asked = strip_line_suffix(asked.trim());
    if asked.is_empty() {
        return Err("no path given".into());
    }
    let given = if let Some(rest) = asked.strip_prefix("~/") {
        home.ok_or("no home directory")?.join(rest)
    } else if Path::new(asked).is_absolute() {
        PathBuf::from(asked)
    } else {
        cwd.join(asked)
    };
    if credential_like(&given) {
        return Err(format!("{asked} looks like a credential; refusing"));
    }
    let abs = std::fs::canonicalize(&given).map_err(|e| format!("{asked}: {e}"))?;
    if credential_like(&abs) {
        return Err(format!("{asked} looks like a credential; refusing"));
    }
    let roots: Vec<PathBuf> = [Some(cwd), home]
        .into_iter()
        .flatten()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .collect();
    if !roots.iter().any(|r| abs.starts_with(r)) {
        return Err(format!(
            "{asked} is outside the pane's folder and your home; refusing"
        ));
    }
    let meta = std::fs::metadata(&abs).map_err(|e| format!("{asked}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{asked} is not a regular file"));
    }
    // Refused before a single byte is read.
    if meta.len() > max {
        return Err(format!("file is {}; limit {}", mb(meta.len()), mb(max)));
    }
    Ok((abs, meta.len()))
}

/// `src/a.rs:42` / `src/a.rs:42:7` → `src/a.rs`.
pub fn strip_line_suffix(s: &str) -> &str {
    let mut out = s;
    for _ in 0..2 {
        if let Some(i) = out.rfind(':') {
            if i > 0 && out[i + 1..].chars().all(|c| c.is_ascii_digit()) && i + 1 < out.len() {
                out = &out[..i];
                continue;
            }
        }
        break;
    }
    out
}

/// Check, then read: `{name, path, size, data}` with the bytes base64-encoded.
pub fn read(asked: &str, cwd: &Path, home: Option<&Path>, max: u64) -> Result<Value, String> {
    let (abs, size) = check(asked, cwd, home, max)?;
    let bytes = std::fs::read(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
    if bytes.len() as u64 > max {
        return Err(format!(
            "file is {}; limit {}",
            mb(bytes.len() as u64),
            mb(max)
        ));
    }
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    Ok(json!({
        "name": name,
        "path": abs.display().to_string(),
        "size": size,
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("klf-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn credentials_are_refused() {
        for p in [
            ".env",
            "app/.env.local",
            "certs/server.pem",
            "x.key",
            "/home/u/.ssh/config",
            "id_ed25519",
            "aws-credentials.json",
        ] {
            assert!(credential_like(Path::new(p)), "{p}");
        }
        for p in ["src/main.rs", "README.md", "keyboard.rs", "docs/env.md"] {
            assert!(!credential_like(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn line_suffix_is_ignored() {
        assert_eq!(strip_line_suffix("src/a.rs:42"), "src/a.rs");
        assert_eq!(strip_line_suffix("src/a.rs:42:7"), "src/a.rs");
        assert_eq!(strip_line_suffix("C:x"), "C:x");
        assert_eq!(strip_line_suffix("a.rs"), "a.rs");
    }

    #[test]
    fn only_inside_cwd_or_home_and_not_secrets() {
        let cwd = tmp("cwd");
        let home = tmp("home");
        let other = tmp("other");
        std::fs::write(cwd.join("a.txt"), "hi").unwrap();
        std::fs::write(home.join("h.txt"), "home").unwrap();
        std::fs::write(other.join("o.txt"), "no").unwrap();
        std::fs::write(cwd.join(".env"), "SECRET=1").unwrap();
        let ok = read("a.txt:3", &cwd, Some(&home), MAX_FILE_BYTES).unwrap();
        assert_eq!(ok["name"], "a.txt");
        assert_eq!(ok["data"], "aGk=");
        assert!(check("~/h.txt", &cwd, Some(&home), MAX_FILE_BYTES).is_ok());
        let out = other.join("o.txt");
        assert!(
            check(out.to_str().unwrap(), &cwd, Some(&home), MAX_FILE_BYTES)
                .unwrap_err()
                .contains("outside")
        );
        assert!(check(
            "../../../../../../etc/passwd",
            &cwd,
            Some(&home),
            MAX_FILE_BYTES
        )
        .is_err());
        assert!(check(".env", &cwd, Some(&home), MAX_FILE_BYTES)
            .unwrap_err()
            .contains("credential"));
        // A symlink to a secret is judged by where it points.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(cwd.join(".env"), cwd.join("notes.txt")).unwrap();
            assert!(check("notes.txt", &cwd, Some(&home), MAX_FILE_BYTES).is_err());
        }
    }

    #[test]
    fn ten_mb_boundary() {
        let cwd = tmp("big");
        let f = std::fs::File::create(cwd.join("at.bin")).unwrap();
        f.set_len(MAX_FILE_BYTES).unwrap();
        let f = std::fs::File::create(cwd.join("over.bin")).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(check("at.bin", &cwd, None, MAX_FILE_BYTES).is_ok());
        let e = check("over.bin", &cwd, None, MAX_FILE_BYTES).unwrap_err();
        assert_eq!(e, "file is 10 MB; limit 10 MB");
        assert_eq!(max_bytes(Some(50 * 1024 * 1024)), MAX_FILE_BYTES);
        assert_eq!(max_bytes(Some(1024)), 1024);
    }
}
