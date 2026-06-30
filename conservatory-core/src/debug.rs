//! Debug-mode observability (Phase 14): the spine for the `--debug` firehose.
//!
//! Both binaries flip [`set_enabled`] on for `--debug`. The flag gates only the
//! things with a real cost when off: the per-statement SQLite profiler hook and
//! the `/proc` reads. The actual log lines are ordinary `tracing` events on four
//! filterable targets, so a normal (non-debug) run pays nothing and stays quiet:
//!
//! - `conservatory::sql` — every statement and its wall-clock time (this module)
//! - `conservatory::io`  — filesystem mutations (emitted at the IO sites)
//! - `conservatory::net` — HTTP requests (emitted in `conservatory-podcasts`)
//! - `conservatory::mem` — resident memory samples (this module)
//!
//! `--debug` is the one switch for the deep hooks; `RUST_LOG` still narrows the
//! output (e.g. `RUST_LOG=conservatory::sql=debug` for SQL alone).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rusqlite::Connection;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn debug mode on (the binaries call this for `--debug`).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether debug mode is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Install a SQL profiler on `conn` logging each completed statement and its
/// time to `conservatory::sql`, but only in debug mode: the callback fires for
/// every statement, so it stays off otherwise. `rusqlite`'s `profile` takes a
/// bare `fn` pointer (it cannot capture), so the writer/reader role is carried
/// by picking the matching function rather than a closure.
pub fn install_sql_profiler(conn: &mut Connection, role: SqlRole) {
    if !enabled() {
        return;
    }
    match role {
        SqlRole::Writer => conn.profile(Some(profile_writer)),
        SqlRole::Reader => conn.profile(Some(profile_reader)),
    }
}

/// Which connection a profiled statement ran on.
#[derive(Clone, Copy)]
pub enum SqlRole {
    Writer,
    Reader,
}

fn profile_writer(sql: &str, dur: Duration) {
    tracing::debug!(target: "conservatory::sql", role = "writer", us = dur.as_micros() as u64, "{sql}");
}

fn profile_reader(sql: &str, dur: Duration) {
    tracing::debug!(target: "conservatory::sql", role = "reader", us = dur.as_micros() as u64, "{sql}");
}

/// Current resident set size in kB from `/proc/self/status` (`VmRSS`), or `None`
/// off Linux or if the field cannot be read.
pub fn rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line.strip_prefix("VmRSS:")?;
        value.split_whitespace().next()?.parse::<u64>().ok()
    })
}

/// Log the current resident memory to `conservatory::mem` (debug mode only).
/// `label` marks the moment (e.g. `"startup"`, `"library-loaded"`).
pub fn log_memory(label: &str) {
    if !enabled() {
        return;
    }
    if let Some(kb) = rss_kb() {
        tracing::debug!(target: "conservatory::mem", rss_mb = kb as f64 / 1024.0, label, "memory");
    }
}

/// In debug mode, spawn a background task that logs resident memory every five
/// seconds (the long-lived GUI uses this; short CLI commands log start/end
/// instead). A no-op when debug mode is off.
pub fn spawn_memory_sampler(handle: &tokio::runtime::Handle) {
    if !enabled() {
        return;
    }
    handle.spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        // Skip the immediate first tick; `log_memory("startup")` already covers it.
        interval.tick().await;
        loop {
            interval.tick().await;
            log_memory("periodic");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_reads_a_positive_value() {
        // On Linux the process always has a resident set; off Linux this is None
        // and the assert is skipped (the feature degrades, it does not break).
        if let Some(kb) = rss_kb() {
            assert!(kb > 0);
        }
    }

    #[test]
    fn enabled_round_trips() {
        let was = enabled();
        set_enabled(true);
        assert!(enabled());
        set_enabled(false);
        assert!(!enabled());
        set_enabled(was);
    }
}
