//! When to send, and when a feed stops itself. Times are milliseconds given by
//! the caller, so tests step a fake clock.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// The four send conditions of ADR 0015 (`--message-gap`).
#[derive(Debug, Clone, Copy)]
pub struct SendCheck {
    pub pending: bool,
    pub agent_idle: bool,
    pub now: u64,
    /// Last message of this feed to this agent.
    pub last_sent: Option<u64>,
    pub gap_ms: u64,
    /// Last time the command produced output.
    pub last_output: u64,
    pub quiet_ms: u64,
    /// When the oldest pending line was queued. A stream that never pauses
    /// is never quiet, so once that line has waited a full gap the batch goes
    /// without the quiet period.
    pub oldest_pending: u64,
}

impl SendCheck {
    pub fn due(&self) -> bool {
        let quiet = self.now.saturating_sub(self.last_output) >= self.quiet_ms;
        let waited = self.now.saturating_sub(self.oldest_pending) >= self.gap_ms;
        self.pending
            && self.agent_idle
            && self
                .last_sent
                .is_none_or(|t| self.now.saturating_sub(t) >= self.gap_ms)
            && (quiet || waited)
    }
}

/// Deliveries to one agent, for `max_per_hour`, and the trips of that limit,
/// for "stopped twice in 24 hours: only a user shell may start it again".
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    sent: VecDeque<u64>,
    trips: VecDeque<u64>,
}

#[derive(Debug, PartialEq)]
pub enum Rate {
    Ok,
    /// Over the limit: the subscription stops. `count` is deliveries in the
    /// last hour; `user_only` when this is the second trip within 24 hours.
    Tripped {
        count: usize,
        user_only: bool,
    },
}

impl RateLimit {
    /// Records one delivery (lines or a notice) and says whether the limit
    /// is now exceeded. `max = 0` means no limit.
    pub fn record(&mut self, now: u64, max: u32) -> Rate {
        self.sent.push_back(now);
        while self
            .sent
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= HOUR_MS)
        {
            self.sent.pop_front();
        }
        if max == 0 || self.sent.len() <= max as usize {
            return Rate::Ok;
        }
        while self
            .trips
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= DAY_MS)
        {
            self.trips.pop_front();
        }
        let user_only = !self.trips.is_empty();
        self.trips.push_back(now);
        let count = self.sent.len();
        self.sent.clear();
        Rate::Tripped { count, user_only }
    }
}

/// Counts only time the worker was running since the last delivery, so a
/// laptop that was off for two weeks does not come back to a stopped feed.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IdleClock {
    pub quiet_running_ms: u64,
}

impl IdleClock {
    /// `elapsed_ms` is running time since the previous call; a jump larger
    /// than `max_step_ms` (sleep, worker down) counts as `max_step_ms`.
    pub fn advance(&mut self, elapsed_ms: u64, max_step_ms: u64) {
        self.quiet_running_ms += elapsed_ms.min(max_step_ms);
    }

    pub fn delivered(&mut self) {
        self.quiet_running_ms = 0;
    }

    /// `idle_stop_ms = 0` means never.
    pub fn expired(&self, idle_stop_ms: u64) -> bool {
        idle_stop_ms > 0 && self.quiet_running_ms >= idle_stop_ms
    }
}

/// Poll / filter errors: reported once per exit code, again at most hourly
/// while it lasts; the first good run after reports recovery.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ErrorTracker {
    failing: Option<i32>,
    last_report: u64,
}

impl ErrorTracker {
    pub fn failed(&mut self, code: i32, now: u64) -> bool {
        let report = self.failing != Some(code) || now.saturating_sub(self.last_report) >= HOUR_MS;
        self.failing = Some(code);
        if report {
            self.last_report = now;
        }
        report
    }

    /// A good run. True when it ends a failure (send `recovered`).
    pub fn succeeded(&mut self) -> bool {
        self.failing.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(now: u64, last_sent: Option<u64>, last_output: u64) -> SendCheck {
        SendCheck {
            pending: true,
            agent_idle: true,
            now,
            last_sent,
            gap_ms: 30_000,
            last_output,
            quiet_ms: 2_000,
            oldest_pending: last_output,
        }
    }

    #[test]
    fn lone_line_is_due_after_quiet_period() {
        assert!(!check(1_000, None, 0).due());
        assert!(check(2_000, None, 0).due());
    }

    #[test]
    fn nothing_before_the_gap() {
        assert!(!check(100_000, Some(80_000), 90_000).due());
        assert!(check(110_000, Some(80_000), 90_000).due());
    }

    #[test]
    fn continuous_stream_is_due_once_per_gap() {
        // a burst every 3 s: quiet comes often, the gap holds it to one per 30 s
        let mut sent = vec![];
        let mut last = None;
        for now in (0..120_000).step_by(500) {
            let last_output = now - now % 3_000;
            let c = check(now, last, last_output);
            if c.due() {
                sent.push(now);
                last = Some(now);
            }
        }
        for w in sent.windows(2) {
            assert!(w[1] - w[0] >= 30_000, "{sent:?}");
        }
        assert!(sent.len() >= 3, "{sent:?}");
    }

    #[test]
    fn never_pausing_stream_is_not_starved() {
        // a line every 500 ms, never quiet for 2 s
        let mut sent = vec![];
        let mut last = None;
        let mut oldest = None;
        for now in (0..120_000u64).step_by(500) {
            oldest.get_or_insert(now);
            let mut c = check(now, last, now);
            c.oldest_pending = oldest.unwrap();
            if c.due() {
                sent.push(now);
                last = Some(now);
                oldest = None;
            }
        }
        assert!(sent.len() >= 3, "{sent:?}");
        for w in sent.windows(2) {
            assert!(w[1] - w[0] >= 30_000, "{sent:?}");
        }
    }

    #[test]
    fn not_due_while_busy_or_empty() {
        let mut c = check(100_000, None, 0);
        c.agent_idle = false;
        assert!(!c.due());
        let mut c = check(100_000, None, 0);
        c.pending = false;
        assert!(!c.due());
    }

    #[test]
    fn delivery_31_in_an_hour_trips() {
        let mut r = RateLimit::default();
        for i in 0..30 {
            assert_eq!(r.record(i * 60_000, 30), Rate::Ok);
        }
        assert_eq!(
            r.record(30 * 60_000, 30),
            Rate::Tripped {
                count: 31,
                user_only: false
            }
        );
    }

    #[test]
    fn old_deliveries_fall_out_of_the_hour() {
        let mut r = RateLimit::default();
        for i in 0..100 {
            assert_eq!(r.record(i * 130_000, 30), Rate::Ok);
        }
    }

    #[test]
    fn second_trip_within_24h_is_user_only() {
        let mut r = RateLimit::default();
        for _ in 0..30 {
            r.record(0, 30);
        }
        assert!(matches!(
            r.record(0, 30),
            Rate::Tripped {
                user_only: false,
                ..
            }
        ));
        for _ in 0..30 {
            r.record(HOUR_MS, 30);
        }
        assert!(matches!(
            r.record(HOUR_MS, 30),
            Rate::Tripped {
                user_only: true,
                ..
            }
        ));
        let later = 3 * DAY_MS;
        for _ in 0..30 {
            r.record(later, 30);
        }
        assert!(matches!(
            r.record(later, 30),
            Rate::Tripped {
                user_only: false,
                ..
            }
        ));
    }

    #[test]
    fn zero_means_no_limit() {
        let mut r = RateLimit::default();
        for _ in 0..1000 {
            assert_eq!(r.record(0, 0), Rate::Ok);
        }
    }

    #[test]
    fn idle_counts_running_time_only() {
        let mut c = IdleClock::default();
        c.advance(14 * DAY_MS, 60_000); // laptop off for two weeks
        assert!(!c.expired(7 * DAY_MS));
        for _ in 0..(7 * 24 * 60) {
            c.advance(60_000, 60_000);
        }
        assert!(c.expired(7 * DAY_MS));
        assert!(!c.expired(0));
        c.delivered();
        assert!(!c.expired(7 * DAY_MS));
    }

    #[test]
    fn same_error_again_only_after_an_hour() {
        let mut e = ErrorTracker::default();
        assert!(e.failed(1, 0));
        assert!(!e.failed(1, 60_000));
        assert!(e.failed(2, 120_000), "a different code is reported");
        assert!(!e.failed(2, 120_000 + HOUR_MS - 1));
        assert!(e.failed(2, 120_000 + HOUR_MS));
        assert!(e.succeeded());
        assert!(!e.succeeded());
    }
}
