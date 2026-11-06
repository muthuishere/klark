//! Assets from the hub to a pane (ADR 0011 B, person to agent).
//!
//! The hub sends `fetch_asset`; the machine downloads the bytes with its
//! machine token, stores them under `<data dir>/uploads/<sha256>.<ext>`
//! (directory 0700, file 0600, swept after 24 h), checks the sha256 and size,
//! and prompts the pane with the absolute path.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

/// Files in the uploads dir older than this are deleted on every fetch.
pub const UPLOAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// Hard cap on a single download (the hub caps people's uploads at 20 MB).
pub const MAX_FETCH: u64 = 100 * 1024 * 1024;
/// Whole fetch (download + prompt) must finish within this.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

static PART: AtomicU64 = AtomicU64::new(0);

/// File extension for a mime type; unknown types get `bin`.
pub fn ext_for_mime(mime: Option<&str>) -> &'static str {
    let m = mime
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match m.as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/heif" => "heif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "text/csv" => "csv",
        "application/json" => "json",
        "application/zip" => "zip",
        _ => "bin",
    }
}

pub fn valid_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Create the uploads dir (0700, and tighten it if it already exists).
pub fn prepare_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Delete regular files in `dir` last modified more than `ttl` ago.
pub fn sweep(dir: &Path, ttl: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > ttl);
        if old && std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// A writer that hashes and counts what passes through, and gives up after
/// a deadline so a stalled hub cannot hold the fetch forever.
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    deadline: Instant,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if Instant::now() > self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "fetch timed out",
            ));
        }
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Store bytes produced by `fill` as `<dir>/<sha256>.<ext>`, verifying the
/// sha256 and size. Writes to a 0600 `create_new` temp file first and renames
/// it into place, so a half-written or wrong file never carries the final name.
pub fn store<F>(
    dir: &Path,
    sha256: &str,
    size: u64,
    mime: Option<&str>,
    fill: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(&mut dyn Write) -> Result<u64, String>,
{
    if !valid_sha256(sha256) {
        return Err(format!("bad sha256: {sha256}"));
    }
    if size > MAX_FETCH {
        return Err(format!("asset of {size} bytes exceeds {MAX_FETCH}"));
    }
    let sha = sha256.to_ascii_lowercase();
    prepare_dir(dir)?;
    sweep(dir, UPLOAD_TTL);
    let dest = dir.join(format!("{sha}.{}", ext_for_mime(mime)));
    if dest.exists() && file_sha256(&dest).as_deref() == Ok(sha.as_str()) {
        return Ok(dest);
    }
    let part = dir.join(format!(
        ".{sha}.part-{}-{}",
        std::process::id(),
        PART.fetch_add(1, Ordering::Relaxed)
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts
        .open(&part)
        .map_err(|e| format!("{}: {e}", part.display()))?;
    let mut w = HashingWriter {
        inner: std::io::BufWriter::new(file),
        hasher: Sha256::new(),
        deadline: Instant::now() + FETCH_TIMEOUT,
    };
    let outcome = fill(&mut w).and_then(|n| {
        w.flush().map_err(|e| e.to_string())?;
        let got = hex(&w.hasher.clone().finalize());
        if n != size {
            Err(format!("size mismatch: expected {size}, got {n}"))
        } else if got != sha {
            Err(format!("sha256 mismatch: expected {sha}, got {got}"))
        } else {
            Ok(())
        }
    });
    drop(w);
    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("{}: {e}", dest.display())
    })?;
    Ok(dest)
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).map_err(|e| e.to_string())?;
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Download `<hub>/api/assets/<sha256>` with the machine token into the
/// uploads dir. Returns the stored file's absolute path.
pub fn fetch(
    hub_base: &str,
    token: &str,
    dir: &Path,
    sha256: &str,
    size: u64,
    mime: Option<&str>,
) -> Result<PathBuf, String> {
    let url = format!("{hub_base}/api/assets/{}", sha256.to_ascii_lowercase());
    store(dir, sha256, size, mime, |w| {
        crate::http::get_to_writer(&url, Some(token), w, size, Duration::from_secs(30))
    })
}

/// The text the pane's agent is prompted with.
pub fn prompt_text(name: Option<&str>, path: &Path, caption: Option<&str>) -> String {
    let from = name.filter(|n| !n.trim().is_empty()).unwrap_or("hub");
    let mut text = format!("[image from {from}] {}", path.display());
    if let Some(c) = caption.map(str::trim).filter(|c| !c.is_empty()) {
        text.push('\n');
        text.push_str(c);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "klark-assets-{tag}-{}-{}",
            std::process::id(),
            PART.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn sha(data: &[u8]) -> String {
        hex(&Sha256::digest(data))
    }

    #[cfg(unix)]
    #[test]
    fn stores_with_private_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp("perm").join("uploads");
        let data = b"\x89PNG fake";
        let p = store(
            &dir,
            &sha(data),
            data.len() as u64,
            Some("image/png"),
            |w| {
                w.write_all(data).unwrap();
                Ok(data.len() as u64)
            },
        )
        .unwrap();
        assert_eq!(p, dir.join(format!("{}.png", sha(data))));
        assert_eq!(std::fs::read(&p).unwrap(), data);
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(&dir), 0o700);
        // Same asset again is reused, not rewritten.
        let again = store(
            &dir,
            &sha(data),
            data.len() as u64,
            Some("image/png"),
            |_| panic!("should not download twice"),
        )
        .unwrap();
        assert_eq!(again, p);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn sha_or_size_mismatch_is_rejected_and_leaves_nothing() {
        let dir = tmp("bad");
        let good = sha(b"expected");
        let e = store(&dir, &good, 8, None, |w| {
            w.write_all(b"tampered").unwrap();
            Ok(8)
        })
        .unwrap_err();
        assert!(e.contains("sha256 mismatch"), "{e}");
        let e = store(&dir, &good, 9, None, |w| {
            w.write_all(b"expected").unwrap();
            Ok(8)
        })
        .unwrap_err();
        assert!(e.contains("size mismatch"), "{e}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert!(store(&dir, "../etc/passwd", 1, None, |_| Ok(1)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sweep_removes_only_old_files() {
        let dir = tmp("sweep");
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old.png");
        let new = dir.join("new.png");
        std::fs::write(&old, b"o").unwrap();
        std::fs::write(&new, b"n").unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(25 * 3600))
            .unwrap();
        drop(f);
        assert_eq!(sweep(&dir, UPLOAD_TTL), 1);
        assert!(!old.exists() && new.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mime_and_prompt() {
        assert_eq!(ext_for_mime(Some("image/jpeg")), "jpg");
        assert_eq!(ext_for_mime(Some("text/html")), "bin");
        assert_eq!(ext_for_mime(None), "bin");
        let p = Path::new("/d/uploads/ab.png");
        assert_eq!(
            prompt_text(None, p, None),
            "[image from hub] /d/uploads/ab.png"
        );
        assert_eq!(
            prompt_text(Some("anna"), p, Some("what is this?")),
            "[image from anna] /d/uploads/ab.png\nwhat is this?"
        );
    }

    #[test]
    fn fetch_streams_from_a_local_hub() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;
        let data = vec![7u8; 300_000];
        let digest = sha(&data);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = data.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            s.write_all(&body).unwrap();
            req
        });
        let dir = tmp("fetch");
        let p = fetch(
            &format!("http://127.0.0.1:{port}"),
            "mtok",
            &dir,
            &digest,
            data.len() as u64,
            Some("image/png"),
        )
        .unwrap();
        let req = server.join().unwrap();
        assert!(
            req.starts_with(&format!("GET /api/assets/{digest} HTTP/1.1\r\n")),
            "{req}"
        );
        assert!(req.contains("Authorization: Bearer mtok\r\n"));
        assert_eq!(std::fs::read(p).unwrap(), data);
        let _ = std::fs::remove_dir_all(dir);
    }
}
