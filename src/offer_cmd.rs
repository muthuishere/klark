//! `klark offer <path>` — an agent offers a file to the people viewing its
//! pane on the hub (ADR 0011 B, agent to person).
//!
//! The bytes go over plain HTTP (`PUT /api/assets`), never over the expose
//! socket, so a big file cannot stall pane streams. The machine token is used
//! because the hub ties the asset to this machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE: &str = "usage: klark offer <path> [--pane <pane-id>] [--force] [--json]
       [--hub <url>] [--token <machine-token>]
  --pane defaults to KLARK_PANE_ID; the hub to KLARK_HUB_URL (or `klark msg login`);
  the token to KLARK_HUB_TOKEN (the machine token `klark expose` uses).
  Credential-looking paths are refused unless --force. Files over 100 MB are refused.";

pub const MAX_OFFER: u64 = 100 * 1024 * 1024;

/// Whether a path looks like a credential (ported from herdr-expose
/// `secretShaped`, plus names containing credentials/secret/token).
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

/// Why this file may not be offered, if anything.
pub fn refusal(path: &Path, size: u64, force: bool) -> Option<String> {
    if !force && credential_like(path) {
        return Some(format!(
            "{} looks like a credential; refusing to offer it (pass --force if you are sure)",
            path.display()
        ));
    }
    if size > MAX_OFFER {
        return Some(format!(
            "{} is {size} bytes; the limit is {MAX_OFFER} (100 MB)",
            path.display()
        ));
    }
    None
}

/// `<base>/api/assets?pane=<pane>&name=<basename>`, both url-encoded.
pub fn put_url(base: &str, pane: &str, name: &str) -> String {
    format!("{base}/api/assets?pane={}&name={}", pct(pane), pct(name))
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

struct Opts {
    path: Option<PathBuf>,
    pane: Option<String>,
    hub: Option<String>,
    token: Option<String>,
    force: bool,
    json: bool,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        path: None,
        pane: std::env::var("KLARK_PANE_ID")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        hub: None,
        token: std::env::var("KLARK_HUB_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        force: false,
        json: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => o.force = true,
            "--json" => o.json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--pane" | "--hub" | "--token" => {
                let v = it.next().ok_or(format!("{a} needs a value"))?.clone();
                match a.as_str() {
                    "--pane" => o.pane = Some(v),
                    "--hub" => o.hub = Some(v),
                    _ => o.token = Some(v),
                }
            }
            s if s.starts_with('-') => return Err(format!("unknown argument: {s}")),
            s if o.path.is_none() => o.path = Some(PathBuf::from(s)),
            s => return Err(format!("unexpected argument: {s}")),
        }
    }
    Ok(o)
}

fn offer(o: Opts) -> Result<serde_json::Value, String> {
    let path = o.path.ok_or("no file given")?;
    let abs = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = std::fs::metadata(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", abs.display()));
    }
    // Check both the name given and where it really points.
    if let Some(r) =
        refusal(&path, meta.len(), o.force).or_else(|| refusal(&abs, meta.len(), o.force))
    {
        return Err(r);
    }
    let pane = o
        .pane
        .ok_or("no pane: run this inside a klark pane or pass --pane <pane-id>")?;
    let base = match o.hub {
        Some(h) => klark_link::http::hub_http_base(&h)?,
        None => crate::msg_cmd::hub_base()
            .ok_or("no hub configured: pass --hub or set KLARK_HUB_URL")??,
    };
    let token = o.token.ok_or(
        "no machine token: set KLARK_HUB_TOKEN (the token `klark expose` uses) or pass --token",
    )?;
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let sha = sha256_file(&abs)?;
    let resp = klark_link::http::put_file(
        &put_url(&base, &pane, &name),
        Some(&token),
        &abs,
        "application/octet-stream",
        Duration::from_secs(60),
    )?;
    if !resp.ok() {
        return Err(format!("hub refused: {}", resp.error_text()));
    }
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
    let sha = body
        .get("sha256")
        .and_then(|v| v.as_str())
        .unwrap_or(&sha)
        .to_string();
    let url = match body.get("url").and_then(|v| v.as_str()) {
        Some(u) if u.starts_with("http") => u.to_string(),
        Some(u) => format!("{base}/{}", u.trim_start_matches('/')),
        None => format!("{base}/assets/{sha}"),
    };
    Ok(serde_json::json!({
        "url": url,
        "sha256": sha,
        "name": name,
        "size": meta.len(),
        "pane_id": pane,
    }))
}

pub fn run(args: &[String]) -> ! {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("klark offer: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let json = o.json;
    match offer(o) {
        Ok(v) => {
            if json {
                println!("{v}");
            } else {
                println!("{}", v["url"].as_str().unwrap_or_default());
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("klark offer: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_paths_are_refused() {
        let refused = [
            ".env",
            "app/.env",
            "prod.env",
            ".env.local",
            "certs/server.pem",
            "tls.key",
            "/home/u/.ssh/id_rsa",
            "id_ed25519",
            "id_ecdsa.pub",
            "/home/u/.ssh/known_hosts",
            "/home/u/.aws/config",
            "/home/u/.gnupg/pubring.kbx",
            "aws_credentials.txt",
            "my-secret-notes.md",
            "github_token",
            "Credentials.json",
            ".netrc",
        ];
        for p in refused {
            assert!(credential_like(Path::new(p)), "{p} should be refused");
            assert!(refusal(Path::new(p), 1, false).is_some());
            assert!(
                refusal(Path::new(p), 1, true).is_none(),
                "--force allows {p}"
            );
        }
        let allowed = [
            "screenshot.png",
            "report.pdf",
            "src/main.rs",
            "environment.md",
            "keyboard.txt",
            "/home/u/project/out.csv",
        ];
        for p in allowed {
            assert!(!credential_like(Path::new(p)), "{p} should be allowed");
        }
    }

    #[test]
    fn size_cap_even_with_force() {
        let r = refusal(Path::new("big.bin"), MAX_OFFER + 1, true).unwrap();
        assert!(r.contains("100 MB"), "{r}");
        assert!(refusal(Path::new("ok.bin"), MAX_OFFER, false).is_none());
    }

    #[test]
    fn put_url_encodes() {
        assert_eq!(
            put_url("http://h:8080", "p 1", "a b&c.png"),
            "http://h:8080/api/assets?pane=p%201&name=a%20b%26c.png"
        );
    }
}
