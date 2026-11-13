//! Feed files under `<state dir>/feeds/`: written by temp file + rename, mode
//! 0600, and the locks that decide whether a feed runs.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

pub struct Store {
    dir: PathBuf,
}

/// Held for the worker's whole life; dropping it releases the lock. The file
/// is never read, only kept open, hence the `dead_code` allowance.
pub struct Held(#[allow(dead_code)] File);

fn open_private(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

impl Store {
    pub fn open(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { dir })
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Writes `bytes` to `name` atomically: a reader sees the old file or the
    /// new one, never half of either.
    pub fn write(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let target = self.path(name);
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!(".{name}.{}.{seq}.tmp", std::process::id()));
        {
            let mut f = OpenOptions::new();
            f.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                f.mode(0o600);
            }
            let mut f = f.open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &target)
    }

    pub fn read(&self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        match fs::read(self.path(name)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        self.write(name, &bytes).map_err(|e| format!("{name}: {e}"))
    }

    pub fn read_json<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        match self.read(name).map_err(|e| format!("{name}: {e}"))? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| format!("{name}: {e}")),
        }
    }

    pub fn remove(&self, name: &str) -> std::io::Result<()> {
        match fs::remove_file(self.path(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Appends to a file (pending output). Created 0600.
    pub fn append(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut o = OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(self.path(name))?.write_all(bytes)
    }

    /// The store lock, held briefly by `feed add` / `stop` so two of them
    /// never interleave. Blocks until free.
    /// Every file name in the store.
    pub fn names(&self) -> Vec<String> {
        fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Ids of every saved feed (`<id>.json`).
    pub fn feed_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .names()
            .into_iter()
            .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
            .filter(|n| !n.contains('.') && !n.starts_with('.'))
            .collect();
        ids.sort();
        ids
    }

    /// Removes every file of feed `id` except its lock, which a worker may
    /// still hold.
    pub fn remove_feed(&self, id: &str) {
        let prefix = format!("{id}.");
        for n in self.names() {
            if n.starts_with(&prefix) && n != format!("{id}.lock") {
                let _ = fs::remove_file(self.path(&n));
            }
        }
    }

    pub fn lock_store(&self) -> std::io::Result<Held> {
        let f = open_private(&self.path("store.lock"))?;
        f.lock()?;
        Ok(Held(f))
    }

    /// `try_lock_feed`, tried again for up to `wait`: a CLI that only looks
    /// whether a worker runs holds the lock for a moment, and a worker
    /// starting in that moment must not take it for a running one.
    pub fn try_lock_feed_for(
        &self,
        id: &str,
        wait: std::time::Duration,
    ) -> std::io::Result<Option<Held>> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            if let Some(h) = self.try_lock_feed(id)? {
                return Ok(Some(h));
            }
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// The per-feed run lock. `Ok(None)` when another worker holds it: the
    /// feed already runs, so the caller does nothing.
    pub fn try_lock_feed(&self, id: &str) -> std::io::Result<Option<Held>> {
        let f = open_private(&self.path(&format!("{id}.lock")))?;
        match f.try_lock() {
            Ok(()) => Ok(Some(Held(f))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "klark-feed-store-{}-{}-{}",
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
        (Store::open(dir.clone()).unwrap(), dir)
    }

    #[test]
    fn write_is_whole_and_json_round_trips() {
        let (s, dir) = tmp();
        s.write_json("a.json", &vec![1, 2, 3]).unwrap();
        let back: Vec<i32> = s.read_json("a.json").unwrap().unwrap();
        assert_eq!(back, vec![1, 2, 3]);
        assert!(s.read_json::<Vec<i32>>("missing.json").unwrap().is_none());
        // no temp file left behind
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["a.json"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn concurrent_writers_never_leave_half_a_file() {
        let (s, dir) = tmp();
        let s = std::sync::Arc::new(s);
        let big_a = vec![b'a'; 200_000];
        let big_b = vec![b'b'; 300_000];
        let hs: Vec<_> = [big_a.clone(), big_b.clone()]
            .into_iter()
            .map(|data| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        s.write("x", &data).unwrap();
                    }
                })
            })
            .collect();
        for _ in 0..50 {
            if let Some(b) = s.read("x").unwrap() {
                assert!(b == big_a || b == big_b, "torn read of {} bytes", b.len());
            }
        }
        for h in hs {
            h.join().unwrap();
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn feed_lock_refuses_a_second_holder() {
        let (s, dir) = tmp();
        let held = s.try_lock_feed("f1").unwrap();
        assert!(held.is_some());
        assert!(s.try_lock_feed("f1").unwrap().is_none());
        assert!(s.try_lock_feed("f2").unwrap().is_some());
        drop(held);
        assert!(s.try_lock_feed("f1").unwrap().is_some());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_momentary_holder_does_not_turn_a_worker_away() {
        let (s, dir) = tmp();
        let s = std::sync::Arc::new(s);
        let peek = s.try_lock_feed("f").unwrap().unwrap();
        let h = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(peek);
        });
        let got = s
            .try_lock_feed_for("f", std::time::Duration::from_secs(2))
            .unwrap();
        assert!(got.is_some(), "a peek is not a running worker");
        h.join().unwrap();
        // A real holder still refuses.
        let t = std::time::Instant::now();
        assert!(s
            .try_lock_feed_for("f", std::time::Duration::from_millis(200))
            .unwrap()
            .is_none());
        assert!(t.elapsed() >= std::time::Duration::from_millis(200));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn store_lock_serialises() {
        let (s, dir) = tmp();
        let s = std::sync::Arc::new(s);
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let g = s.lock_store().unwrap();
        let (s2, o2) = (s.clone(), order.clone());
        let h = std::thread::spawn(move || {
            let _g = s2.lock_store().unwrap();
            o2.lock().unwrap().push(2);
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        order.lock().unwrap().push(1);
        drop(g);
        h.join().unwrap();
        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
        fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (s, dir) = tmp();
        s.write("w", b"x").unwrap();
        s.append("p", b"x").unwrap();
        let _l = s.try_lock_feed("f").unwrap();
        for n in ["w", "p", "f.lock"] {
            let mode = fs::metadata(s.path(n)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{n}");
        }
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        fs::remove_dir_all(dir).ok();
    }
}
