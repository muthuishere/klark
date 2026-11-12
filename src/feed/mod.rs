//! `klark feed`: a background command's new output goes to agents (ADR 0015).
//!
//! M1 holds only the pure core: no process, no socket. Everything that decides
//! (batching, gaps, limits, diffs, filter answers, message text) takes the time
//! as an argument so it can be tested with a fake clock.

pub mod diff;
pub mod filter;
pub mod follow;
pub mod lines;
pub mod message;
pub mod model;
pub mod queue;
pub mod schedule;
pub mod store;

/// Largest single line kept; longer lines are cut with a marker.
pub const MAX_LINE_BYTES: usize = 4096;
/// Pending output per subscriber; the oldest lines are dropped beyond it.
pub const PENDING_CAP_BYTES: usize = 64 * 1024;
/// Output must be quiet this long before a message goes out.
pub const QUIET_MS: u64 = 2_000;
/// No timing option may go below this, except under `KLARK_FEED_TEST=1`.
pub const MIN_GAP_SECS: u64 = 5;
pub const DEFAULT_GAP_SECS: u64 = 30;
pub const DEFAULT_MAX_BYTES: usize = 4096;
pub const DEFAULT_MAX_PER_HOUR: u32 = 30;
pub const DEFAULT_IDLE_STOP_SECS: u64 = 7 * 24 * 3600;
