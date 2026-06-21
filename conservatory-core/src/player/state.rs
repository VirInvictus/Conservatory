//! Playback-state persistence logic (spec §6.4): when to flush the transport
//! cursor, and which stop reasons count as a completed play.
//!
//! Pure: no libmpv, no DB. The host feeds it events and a monotonic clock; it
//! decides whether to persist. This is the headless-testable half of the state
//! discipline in spec §6.4 ("position written on pause, seek, item end, quit,
//! and every 30 s"). The actual write goes through the single-writer worker.

/// The insurance interval: position is flushed at least this often during
/// steady playback, even with no pause/seek (the Belfry precedent, spec §6.4).
pub const INSURANCE_INTERVAL_MS: u64 = 30_000;

/// What the host observed. Everything but [`StateEvent::Tick`] is a forced
/// flush point; `Tick` flushes only when the insurance interval has elapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateEvent {
    /// Steady-playback position update (the periodic poll).
    Tick,
    /// User paused.
    Pause,
    /// User sought.
    Seek,
    /// The item ended (any reason).
    ItemEnd,
    /// The app is quitting.
    Quit,
}

/// Decides when to persist the transport cursor, debouncing the steady stream
/// of position ticks down to one write per [`INSURANCE_INTERVAL_MS`] while
/// flushing immediately on the meaningful transitions.
#[derive(Debug, Clone)]
pub struct StateDebounce {
    interval_ms: u64,
    last_flush_ms: Option<u64>,
}

impl StateDebounce {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_flush_ms: None,
        }
    }

    /// Whether to flush now, given the current monotonic time and the event.
    /// Updates the internal "last flushed" mark when it returns `true`, so the
    /// caller must persist whenever this says so.
    pub fn should_flush(&mut self, now_ms: u64, event: StateEvent) -> bool {
        let flush = match event {
            // Forced flush points (spec §6.4): always persist.
            StateEvent::Pause | StateEvent::Seek | StateEvent::ItemEnd | StateEvent::Quit => true,
            // Steady playback: only once per interval.
            StateEvent::Tick => match self.last_flush_ms {
                None => true,
                Some(last) => now_ms.saturating_sub(last) >= self.interval_ms,
            },
        };
        if flush {
            self.last_flush_ms = Some(now_ms);
        }
        flush
    }
}

impl Default for StateDebounce {
    fn default() -> Self {
        Self::new(INSURANCE_INTERVAL_MS)
    }
}

/// Why playback of an item stopped. Core's own enum so the pure logic (and the
/// CLI) never depend on libmpv's `EndFileReason`; the host maps one to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Played through to the end.
    Eof,
    /// Stopped by the user / a new load before the end.
    Stopped,
    /// Stopped by an error (decode/IO).
    Errored,
    /// The player is shutting down.
    Quit,
    /// Playlist redirect (a layer of indirection, not a real end).
    Redirect,
}

impl EndReason {
    /// Whether reaching this end should bump `play_count` + `last_played`
    /// (spec §6.4). Only a natural end-of-file counts as a play; a skip, an
    /// error, or a quit does not.
    pub fn counts_as_play(self) -> bool {
        matches!(self, EndReason::Eof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_tick_flushes_then_debounces() {
        let mut d = StateDebounce::new(30_000);
        assert!(d.should_flush(0, StateEvent::Tick)); // nothing flushed yet
        assert!(!d.should_flush(1_000, StateEvent::Tick));
        assert!(!d.should_flush(29_999, StateEvent::Tick));
        assert!(d.should_flush(30_000, StateEvent::Tick)); // interval elapsed
        assert!(!d.should_flush(30_001, StateEvent::Tick));
    }

    #[test]
    fn forced_events_always_flush_and_reset_the_clock() {
        let mut d = StateDebounce::new(30_000);
        assert!(d.should_flush(0, StateEvent::Tick));
        // A pause mid-interval flushes and re-marks, so the next interval is
        // measured from the pause, not the last tick.
        assert!(d.should_flush(5_000, StateEvent::Pause));
        assert!(!d.should_flush(10_000, StateEvent::Tick)); // 5s since pause
        assert!(d.should_flush(35_000, StateEvent::Tick)); // 30s since pause
    }

    #[test]
    fn seek_end_quit_each_force_a_flush() {
        let mut d = StateDebounce::new(30_000);
        assert!(d.should_flush(100, StateEvent::Seek));
        assert!(d.should_flush(200, StateEvent::ItemEnd));
        assert!(d.should_flush(300, StateEvent::Quit));
    }

    #[test]
    fn only_eof_counts_as_a_play() {
        assert!(EndReason::Eof.counts_as_play());
        for r in [
            EndReason::Stopped,
            EndReason::Errored,
            EndReason::Quit,
            EndReason::Redirect,
        ] {
            assert!(!r.counts_as_play());
        }
    }
}
