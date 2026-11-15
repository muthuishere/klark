//! Removes what klark keeps but nobody reads after a while: hub image uploads,
//! old log files and message-log entries older than the retention period.
//! The server runs it at most once a day. `KLARK_RETENTION_DAYS` sets the
//! period (default 14); 0 turns it off.

use std::path::Path;
use std::time::{Duration, SystemTime};

const DEFAULT_DAYS: u64 = 14;
const MARKER: &str = ".housekeeping";

pub(crate) fn retention_days() -> u64 {
    std::env::var("KLARK_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_DAYS)
}

/// Runs the cleanup unless it already ran in the last day. Never fails.
pub(crate) fn run_daily() {
    let days = retention_days();
    if days == 0 {
        return;
    }
    let dir = crate::session::data_dir_for(None);
    let marker = dir.join(MARKER);
    let ran_recently = std::fs::metadata(&marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if ran_recently {
        return;
    }
    let _ = std::fs::write(&marker, b"");
    let removed = clean(&dir, days);
    if removed > 0 {
        tracing::info!(removed, days, "housekeeping: removed old klark files");
    }
}

/// Returns how many files were removed (message-log trimming not counted).
pub(crate) fn clean(dir: &Path, days: u64) -> usize {
    let cutoff = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
    let mut removed = remove_older(&dir.join("uploads"), cutoff, |_| true);
    // Log files directly in the data dir and in each session's folder; the
    // live log is written all the time, so only abandoned ones are this old.
    removed += remove_older(dir, cutoff, is_log);
    if let Ok(sessions) = std::fs::read_dir(dir.join("sessions")) {
        for s in sessions.flatten() {
            removed += remove_older(&s.path(), cutoff, is_log);
        }
    }
    trim_messages(dir, cutoff);
    removed
}

fn is_log(name: &str) -> bool {
    name.ends_with(".log") || name.contains(".log.")
}

fn remove_older(dir: &Path, cutoff: SystemTime, keep_if: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() || !keep_if(&name) {
            continue;
        }
        if meta.modified().is_ok_and(|t| t < cutoff) && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    n
}

/// Drops message-log lines older than the cutoff; lines it cannot date stay.
fn trim_messages(dir: &Path, cutoff: SystemTime) {
    let path = dir.join("messages.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    // klark writes UTC RFC 3339 ("2026-11-13T11:23:51.89Z"); to the second
    // that sorts as text.
    let Ok(cutoff) =
        time::OffsetDateTime::from(cutoff).format(&time::format_description::well_known::Rfc3339)
    else {
        return;
    };
    let cutoff = cutoff[..19].to_string();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v["ts"].as_str().map(str::to_string))
                .filter(|ts| ts.len() >= 19)
                .is_none_or(|ts| ts[..19] >= *cutoff)
        })
        .collect();
    if kept.len() == text.lines().count() {
        return;
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn removes_only_old_uploads_logs_and_messages() {
        let dir = std::env::temp_dir().join(format!("klark-housekeeping-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = dir.join("sessions").join("work");
        std::fs::create_dir_all(dir.join("uploads")).unwrap();
        std::fs::create_dir_all(&s).unwrap();
        let files = [
            (dir.join("uploads/old.png"), 30, false),
            (dir.join("uploads/new.png"), 1, true),
            (s.join("herdr-server.log"), 30, false),
            (s.join("klark-server.log.1"), 30, false),
            (s.join("klark-server.log"), 1, true),
            (s.join("session.json"), 30, true),
            (dir.join("config.toml"), 30, true),
        ];
        for (p, d, _) in &files {
            std::fs::write(p, "x").unwrap();
            age(p, *d);
        }
        let old = (time::OffsetDateTime::now_utc() - time::Duration::days(30))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let new = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        std::fs::write(
            dir.join("messages.jsonl"),
            format!("{{\"ts\":\"{old}\",\"text\":\"old\"}}\n{{\"ts\":\"{new}\",\"text\":\"new\"}}\nnot json\n"),
        )
        .unwrap();

        assert_eq!(clean(&dir, 14), 3);
        for (p, _, kept) in &files {
            assert_eq!(p.exists(), *kept, "{}", p.display());
        }
        let msgs = std::fs::read_to_string(dir.join("messages.jsonl")).unwrap();
        assert!(!msgs.contains("\"old\"") && msgs.contains("\"new\"") && msgs.contains("not json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
