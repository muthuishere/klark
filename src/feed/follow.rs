//! `--follow <path>`: klark reads appended lines itself and keeps the file's
//! identity and offset, so a restart continues where it stopped. Rotation
//! (a new file at the path) and truncation are followed; a missing file is
//! waited for.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use super::model::FollowPos;

/// Most bytes queued from a file after klark was not running.
pub const CATCH_UP_CAP: u64 = 64 * 1024;
/// Most bytes read in one step, so a huge append cannot stall delivery.
const STEP_MAX: u64 = 256 * 1024;

#[derive(Debug, Default, PartialEq)]
pub struct Step {
    /// Complete lines' bytes (ending in `\n`), plus a released partial line.
    pub bytes: Vec<u8>,
    /// `(… N bytes … were skipped)` after a long stop.
    pub skipped: u64,
    /// The file could not be read (other than not existing yet).
    pub error: Option<String>,
}

pub struct Follower {
    path: PathBuf,
    file: Option<File>,
    pub pos: Option<FollowPos>,
    /// A partial last line: since when it has been waiting, by its size.
    partial: Option<(u64, std::time::Instant)>,
    /// The path was seen missing: whatever appears there next is a new file,
    /// even when the file system gives it the old inode number (Linux does).
    gone: bool,
}

/// The open file was deleted (no name left). A new file at the path may then
/// reuse its inode number, so the identity alone would not show the change.
#[cfg(unix)]
fn unlinked(f: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    f.metadata().is_ok_and(|m| m.nlink() == 0)
}

#[cfg(not(unix))]
fn unlinked(_f: &File) -> bool {
    false
}

/// Which file is at `path` now: device and inode on Unix, volume serial
/// number and file index on Windows. The creation time is not used: Windows
/// "file tunneling" gives a file re-created under the same name within 15 s
/// the old one's creation time, which is exactly what log rotation does.
#[cfg(unix)]
fn identity(_path: &std::path::Path, m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.dev(), m.ino())
}

#[cfg(windows)]
fn identity(path: &std::path::Path, _m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    // Share everything, so following never stops a writer or a rename.
    let Ok(f) = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
    else {
        return (0, 0);
    };
    // SAFETY: the handle is valid while `f` lives; the struct is plain data
    // that the call fills in.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
        return (0, 0);
    }
    (
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

impl Follower {
    /// `pos`: where a previous worker stopped, or `None` for a new feed,
    /// which starts at the end of the file (or at 0 once it appears).
    pub fn new(path: PathBuf, pos: Option<FollowPos>) -> Self {
        Self {
            path,
            file: None,
            pos,
            partial: None,
            gone: false,
        }
    }

    /// The first call of a worker: a new feed starts at the end, a resumed
    /// one where it stopped, cut to the newest `CATCH_UP_CAP` bytes.
    pub fn start(&mut self) -> Step {
        let mut step = Step::default();
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return step,
            Err(e) => {
                step.error = Some(e.to_string());
                return step;
            }
        };
        let (dev, ino) = identity(&self.path, &meta);
        let len = meta.len();
        match &mut self.pos {
            None => {
                self.pos = Some(FollowPos {
                    dev,
                    ino,
                    offset: len,
                })
            }
            Some(p) => {
                if (p.dev, p.ino) != (dev, ino) || p.offset > len {
                    // Rotated or truncated while klark was down: the new file
                    // is all new.
                    *p = FollowPos {
                        dev,
                        ino,
                        offset: 0,
                    };
                }
                if len - p.offset > CATCH_UP_CAP {
                    step.skipped = len - CATCH_UP_CAP - p.offset;
                    p.offset = len - CATCH_UP_CAP;
                    // Start at a line boundary.
                    if let Ok(mut f) = File::open(&self.path) {
                        let mut buf = vec![0u8; CATCH_UP_CAP as usize];
                        if f.seek(SeekFrom::Start(p.offset)).is_ok() {
                            let n = f.read(&mut buf).unwrap_or(0);
                            if let Some(i) = buf[..n].iter().position(|b| *b == b'\n') {
                                step.skipped += i as u64 + 1;
                                p.offset += i as u64 + 1;
                            }
                        }
                    }
                }
            }
        }
        step
    }

    /// Reads what was appended since the last step. A last line without a
    /// newline is held until `partial_ms` passed without it growing.
    pub fn step(&mut self, partial_ms: u64) -> Step {
        let mut step = Step::default();
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => Some(m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                step.error = Some(e.to_string());
                return step;
            }
        };
        let Some(meta) = meta else {
            // Gone (being rotated): finish the old file, then wait.
            if self.file.is_some() {
                self.finish_old(&mut step, partial_ms);
                self.file = None;
            }
            self.gone = true;
            return step;
        };
        let (dev, ino) = identity(&self.path, &meta);
        let same = !std::mem::take(&mut self.gone)
            && !self.file.as_ref().is_some_and(unlinked)
            && self
                .pos
                .as_ref()
                .is_some_and(|p| (p.dev, p.ino) == (dev, ino));
        if !same {
            if self.pos.is_some() && self.file.is_some() {
                // Rotated: what was appended to the old file first.
                self.finish_old(&mut step, partial_ms);
            }
            self.file = None;
            self.partial = None;
            self.pos = Some(FollowPos {
                dev,
                ino,
                offset: 0,
            });
        } else if let Some(p) = &mut self.pos {
            if meta.len() < p.offset {
                // Truncated: start again from the top.
                p.offset = 0;
                self.partial = None;
            }
        }
        if self.file.is_none() {
            match File::open(&self.path) {
                Ok(f) => self.file = Some(f),
                Err(e) => {
                    step.error = Some(e.to_string());
                    return step;
                }
            }
        }
        self.read_open(&mut step, false, partial_ms);
        step
    }

    /// The rest of a file that is no longer at the path. One that cannot be
    /// read (it was a directory) has nothing more to give: its error must
    /// not hide what the new file holds, which is read in the same step.
    fn finish_old(&mut self, step: &mut Step, partial_ms: u64) {
        let mut old = Step::default();
        self.read_open(&mut old, true, partial_ms);
        step.bytes = old.bytes;
    }

    fn read_open(&mut self, step: &mut Step, to_end: bool, partial_ms: u64) {
        let (Some(f), Some(p)) = (self.file.as_mut(), self.pos.as_mut()) else {
            return;
        };
        if f.seek(SeekFrom::Start(p.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if let Err(e) = f.by_ref().take(STEP_MAX).read_to_end(&mut buf) {
            step.error = Some(e.to_string());
            return;
        }
        let complete = buf
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut take = complete;
        if complete < buf.len() {
            let size = (buf.len() - complete) as u64;
            let released = to_end
                || match self.partial {
                    Some((s, since)) if s == size => {
                        since.elapsed().as_millis() as u64 >= partial_ms
                    }
                    _ => {
                        self.partial = Some((size, std::time::Instant::now()));
                        false
                    }
                };
            if released {
                take = buf.len();
                self.partial = None;
            }
        } else {
            self.partial = None;
        }
        step.bytes.extend_from_slice(&buf[..take]);
        if take > 0 && step.bytes.last() != Some(&b'\n') {
            step.bytes.push(b'\n');
        }
        p.offset += take as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "klark-follow-{}-{}-{}",
            std::process::id(),
            {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn append(p: &PathBuf, s: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap()
            .write_all(s.as_bytes())
            .unwrap();
    }

    #[test]
    fn new_feed_starts_at_the_end() {
        let p = tmp("a.log");
        append(&p, "old\n");
        let mut f = Follower::new(p.clone(), None);
        assert_eq!(f.start(), Step::default());
        assert!(f.step(5000).bytes.is_empty());
        append(&p, "new\n");
        assert_eq!(f.step(5000).bytes, b"new\n");
    }

    #[test]
    fn missing_file_is_waited_for_then_read_from_the_top() {
        let p = tmp("later.log");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        assert!(f.step(5000).bytes.is_empty());
        append(&p, "first\n");
        assert_eq!(f.step(5000).bytes, b"first\n");
    }

    #[test]
    fn rotation_and_truncation() {
        let p = tmp("r.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        append(&p, "a\n");
        assert_eq!(f.step(5000).bytes, b"a\n");
        append(&p, "b\n");
        std::fs::rename(&p, p.with_extension("1")).unwrap();
        append(&p, "c\n");
        assert_eq!(f.step(5000).bytes, b"b\nc\n");
        // Like `tail -F`, a truncation is seen when the file is shorter
        // than what was read.
        std::fs::write(&p, "").unwrap();
        assert!(f.step(5000).bytes.is_empty());
        append(&p, "d\n");
        assert_eq!(f.step(5000).bytes, b"d\n");
    }

    #[test]
    fn partial_line_is_held() {
        let p = tmp("p.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        append(&p, "half");
        assert!(f.step(50).bytes.is_empty());
        std::thread::sleep(std::time::Duration::from_millis(80));
        assert_eq!(f.step(50).bytes, b"half\n");
        append(&p, "next\n");
        assert_eq!(f.step(50).bytes, b"next\n");
    }

    #[test]
    fn deleted_and_never_recreated_waits_quietly() {
        let p = tmp("gone.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        assert!(f.step(5000).bytes.is_empty()); // the file is open now
        append(&p, "last\n");
        std::fs::remove_file(&p).unwrap();
        // What was written before the delete is still read once.
        assert_eq!(f.step(5000).bytes, b"last\n");
        for _ in 0..3 {
            assert_eq!(f.step(5000), Step::default());
        }
        append(&p, "back\n");
        assert_eq!(f.step(5000).bytes, b"back\n");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_by_a_directory_is_an_error_then_recovers() {
        let p = tmp("d.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir(&p).unwrap();
        let s = f.step(5000);
        assert!(s.error.is_some(), "{s:?}");
        assert!(s.bytes.is_empty());
        std::fs::remove_dir(&p).unwrap();
        append(&p, "file again\n");
        assert_eq!(
            f.step(5000),
            Step {
                bytes: b"file again\n".to_vec(),
                ..Step::default()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return; // root reads anything
        }
        let p = tmp("ro.log");
        append(&p, "x\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut f = Follower::new(p.clone(), None);
        assert!(f.start().error.is_none(), "metadata needs no read access");
        // A new file at the path: it is opened, and that fails.
        std::fs::remove_file(&p).unwrap();
        assert_eq!(f.step(5000), Step::default(), "missing: waited for");
        append(&p, "y\n");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(f.step(5000).error.is_some());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(f.step(5000).bytes, b"y\n");
    }

    #[test]
    fn two_rotations_between_steps_lose_only_the_middle_file() {
        // Like `tail -F`: a file that came and went between two looks is
        // never seen. The old file's tail and the newest file are read.
        let p = tmp("rr.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        append(&p, "a\n");
        assert_eq!(f.step(5000).bytes, b"a\n");
        append(&p, "b\n");
        std::fs::rename(&p, p.with_extension("1")).unwrap();
        append(&p, "middle\n");
        std::fs::rename(&p, p.with_extension("2")).unwrap();
        append(&p, "c\n");
        assert_eq!(f.step(5000).bytes, b"b\nc\n");
    }

    #[test]
    fn copytruncate_is_followed() {
        let p = tmp("ct.log");
        append(&p, "");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        append(&p, "one\ntwo\n");
        assert_eq!(f.step(5000).bytes, b"one\ntwo\n");
        // logrotate copytruncate: copy, then truncate in place (same inode).
        std::fs::copy(&p, p.with_extension("1")).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(0)
            .unwrap();
        append(&p, "3\n");
        assert_eq!(f.step(5000).bytes, b"3\n");
        // A truncation followed by more than was read before the next look
        // is not seen (the file is not shorter): as with `tail -F`, the
        // first bytes are skipped.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(0)
            .unwrap();
        append(&p, "long line after\n");
        assert_eq!(f.step(5000).bytes, b"ng line after\n");
    }

    #[test]
    fn resume_continues_and_caps_the_catch_up() {
        let p = tmp("c.log");
        append(&p, "x\n");
        let mut f = Follower::new(p.clone(), None);
        f.start();
        let pos = f.pos.clone();
        append(&p, "while down\n");
        let mut g = Follower::new(p.clone(), pos.clone());
        assert_eq!(g.start().skipped, 0);
        assert_eq!(g.step(5000).bytes, b"while down\n");

        let big = "y".repeat(99) + "\n";
        for _ in 0..2000 {
            append(&p, &big);
        }
        let mut h = Follower::new(p.clone(), g.pos.clone());
        let s = h.start();
        assert!(s.skipped > 100_000, "{}", s.skipped);
        let got = h.step(5000).bytes;
        assert!(got.len() as u64 <= CATCH_UP_CAP);
        assert!(got.starts_with(b"yyy"));
    }
}
