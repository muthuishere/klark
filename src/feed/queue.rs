//! Pending output for one subscriber.

use std::collections::VecDeque;

/// Lines waiting for one agent, capped in bytes (oldest dropped), plus
/// notices (stopped, error, recovered, stale) that sit outside the cap and are
/// never dropped.
#[derive(Debug)]
pub struct Queue {
    lines: VecDeque<String>,
    bytes: usize,
    cap: usize,
    dropped: u64,
    notices: Vec<String>,
}

/// What one message carries.
#[derive(Debug, PartialEq)]
pub struct Batch {
    pub notices: Vec<String>,
    /// The newest lines that fit in the message.
    pub shown: Vec<String>,
    /// Every line of the batch; kept in a batch file when `cut`.
    pub all: Vec<String>,
    /// Lines dropped by the pending cap before this batch was taken.
    pub dropped: u64,
    pub cut: bool,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.notices.is_empty() && self.all.is_empty() && self.dropped == 0
    }
}

fn cost(line: &str) -> usize {
    line.len() + 1
}

impl Queue {
    pub fn new(cap: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            cap,
            dropped: 0,
            notices: Vec::new(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        self.bytes += cost(&line);
        self.lines.push_back(line);
        while self.bytes > self.cap {
            match self.lines.pop_front() {
                Some(old) => {
                    self.bytes -= cost(&old);
                    self.dropped += 1;
                }
                None => break,
            }
        }
    }

    pub fn push_notice(&mut self, notice: String) {
        self.notices.push(notice);
    }

    pub fn has_lines(&self) -> bool {
        !self.lines.is_empty() || self.dropped > 0
    }

    pub fn has_notices(&self) -> bool {
        !self.notices.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        !self.has_lines() && !self.has_notices()
    }

    /// What is pending, oldest first: (notices, lines). For rewriting the
    /// pending file after the cap dropped lines.
    pub fn snapshot(&self) -> (Vec<String>, Vec<String>) {
        (self.notices.clone(), self.lines.iter().cloned().collect())
    }

    /// A batch that could not be delivered goes back in front of anything
    /// queued since, so nothing is lost or reordered. The cap still applies.
    pub fn put_back(&mut self, batch: Batch) {
        let newer: Vec<String> = self.lines.drain(..).collect();
        self.bytes = 0;
        let newer_notices = std::mem::take(&mut self.notices);
        self.notices = batch.notices;
        self.notices.extend(newer_notices);
        self.dropped += batch.dropped;
        for l in batch.all.into_iter().chain(newer) {
            self.push_line(l);
        }
    }

    /// Takes everything pending. `max_bytes` bounds the lines shown in the
    /// message; the newest are kept.
    pub fn take(&mut self, max_bytes: usize) -> Batch {
        let all: Vec<String> = self.lines.drain(..).collect();
        self.bytes = 0;
        let mut shown = Vec::new();
        let mut used = 0;
        for line in all.iter().rev() {
            if used + cost(line) > max_bytes {
                break;
            }
            used += cost(line);
            shown.push(line.clone());
        }
        shown.reverse();
        let cut = shown.len() < all.len();
        Batch {
            notices: std::mem::take(&mut self.notices),
            shown,
            all,
            dropped: std::mem::take(&mut self.dropped),
            cut,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_cap_drops_oldest_with_count() {
        let mut q = Queue::new(10);
        for l in ["aaaa", "bbbb", "cccc"] {
            q.push_line(l.into()); // 5 bytes each
        }
        let b = q.take(1000);
        assert_eq!(b.all, vec!["bbbb", "cccc"]);
        assert_eq!(b.dropped, 1);
        assert!(q.is_empty());
    }

    #[test]
    fn notice_survives_a_flood() {
        let mut q = Queue::new(100);
        q.push_notice("stopped".into());
        for i in 0..10_000 {
            q.push_line(format!("line {i}"));
        }
        let b = q.take(1000);
        assert_eq!(b.notices, vec!["stopped"]);
        assert!(b.dropped > 0);
    }

    #[test]
    fn batch_over_max_bytes_keeps_newest() {
        let mut q = Queue::new(1 << 20);
        for i in 0..100 {
            q.push_line(format!("{i:04}")); // 5 bytes each
        }
        let b = q.take(20);
        assert_eq!(b.shown, vec!["0096", "0097", "0098", "0099"]);
        assert_eq!(b.all.len(), 100);
        assert!(b.cut);
    }

    #[test]
    fn put_back_keeps_order_and_notices() {
        let mut q = Queue::new(1 << 20);
        q.push_line("a".into());
        q.push_notice("n1".into());
        let b = q.take(4096);
        q.push_line("b".into());
        q.push_notice("n2".into());
        q.put_back(b);
        let b = q.take(4096);
        assert_eq!(b.all, vec!["a", "b"]);
        assert_eq!(b.notices, vec!["n1", "n2"]);
    }

    #[test]
    fn small_batch_is_not_cut() {
        let mut q = Queue::new(1 << 20);
        q.push_line("one".into());
        let b = q.take(4096);
        assert!(!b.cut);
        assert_eq!(b.shown, b.all);
        assert!(!b.is_empty());
        assert!(q.take(4096).is_empty());
    }
}
