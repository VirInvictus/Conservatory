//! Sleep timer (spec §3.6, Phase 6c-iii-d; Belfry §3.6).
//!
//! Pure: no libmpv, no DB. The engine holds one optional [`SleepClock`], ticks it
//! once per loop turn with a wall-clock delta and whether it is playing, and
//! enforces the boundary modes at the EOF / advance points. The clock reports a
//! [`SleepStatus`] onto the player snapshot for the Now-bar / drawer / CLI.
//!
//! The timer pauses playback at a chosen boundary so the listener can fall asleep
//! (Castro / Overcast pause rather than stop, so a resume is possible). A duration
//! timer counts down only while playing (a manual pause holds it, the
//! session-accumulator idle precedent). Castro's **tap-to-extend**: when a duration
//! timer elapses it pauses and opens a 30 s window; pressing play within it re-arms
//! the same interval instead of merely resuming.
//!
//! The timer is transient per-session state, like Castro/Overcast: it is never
//! persisted, so there is no DB column and no migration.

/// How long after a duration timer fires the tap-to-extend window stays open
/// (Castro's "tap play within 30 s to extend", Belfry §3.6).
pub const TAP_TO_EXTEND_WINDOW: f64 = 30.0;

/// The boundary a sleep timer pauses playback at (Belfry §3.6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SleepMode {
    /// Pause after this many seconds of *playback* (the 15 / 30 / 45 / 60 min
    /// presets, or any custom value). Only this mode counts down and tap-extends.
    After(f64),
    /// Pause when the current item finishes (the menu reads "End of episode" for an
    /// episode, "End of track" for a track).
    EndOfItem,
    /// Pause when the whole queue finishes (the queue already stops at its end; this
    /// records the intent and disarms when satisfied).
    EndOfQueue,
}

/// A snapshot of the armed sleep timer for the UI / CLI to read. `None` on the
/// player snapshot means no timer is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SleepStatus {
    pub mode: SleepMode,
    /// Seconds left for an [`SleepMode::After`] timer; `None` for the boundary modes.
    pub remaining: Option<f64>,
    /// The duration timer has elapsed and paused playback; the tap-to-extend window
    /// is open (pressing play re-arms the same interval).
    pub fired: bool,
}

/// The engine-held sleep-timer state. Drive it with [`tick`](Self::tick) each loop
/// turn and [`on_play`](Self::on_play) when playback is (re)started; read
/// [`status`](Self::status) for the snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SleepClock {
    mode: SleepMode,
    /// Seconds left for an `After` timer (meaningless for the boundary modes).
    remaining: f64,
    /// The original `After` interval, kept so tap-to-extend can re-arm it.
    interval: f64,
    /// `true` once an `After` timer has elapsed and paused playback.
    fired: bool,
    /// Wall-clock seconds accrued since firing, for the tap-to-extend window.
    since_fired: f64,
}

impl SleepClock {
    /// Arm a fresh timer in `mode`. An `After` starts counting from its full
    /// interval; the boundary modes carry no countdown.
    pub fn new(mode: SleepMode) -> Self {
        let interval = match mode {
            SleepMode::After(secs) => secs.max(0.0),
            _ => 0.0,
        };
        Self {
            mode,
            remaining: interval,
            interval,
            fired: false,
            since_fired: 0.0,
        }
    }

    /// Advance the clock by `dt` wall-clock seconds. `playing` is the engine's
    /// "actually producing audio" state (`!paused && !ended`).
    ///
    /// Returns `true` exactly on the turn an [`SleepMode::After`] timer crosses to
    /// zero (the engine pauses then). While paused it does not count down (so the
    /// timer cannot expire silently mid-pause). After firing it accrues the
    /// tap-to-extend window regardless of `playing` and self-clears the `fired`
    /// flag once the window lapses. The boundary modes never tick.
    pub fn tick(&mut self, dt: f64, playing: bool) -> bool {
        let dt = dt.max(0.0);
        if self.fired {
            self.since_fired += dt;
            if self.since_fired >= TAP_TO_EXTEND_WINDOW {
                self.fired = false;
            }
            return false;
        }
        if !matches!(self.mode, SleepMode::After(_)) || !playing {
            return false;
        }
        if self.remaining <= 0.0 {
            return false;
        }
        self.remaining -= dt;
        if self.remaining <= 0.0 {
            self.remaining = 0.0;
            self.fired = true;
            self.since_fired = 0.0;
            return true;
        }
        false
    }

    /// Tap-to-extend: called when the user (re)starts playback. If a duration timer
    /// has fired and the tap-to-extend window is still open, re-arm the same
    /// interval and return `true`; otherwise leave the timer untouched and return
    /// `false`. A no-op for the boundary modes.
    pub fn on_play(&mut self) -> bool {
        if self.fired && self.since_fired < TAP_TO_EXTEND_WINDOW {
            self.remaining = self.interval;
            self.fired = false;
            self.since_fired = 0.0;
            true
        } else {
            false
        }
    }

    pub fn mode(&self) -> SleepMode {
        self.mode
    }

    /// `true` once an `After` timer has elapsed (the tap-to-extend window is open).
    pub fn fired(&self) -> bool {
        self.fired
    }

    /// `true` for a duration timer that has done its job: it elapsed, and the
    /// tap-to-extend window then lapsed without a resume (so `fired` self-cleared).
    /// The engine disarms a spent clock so the UI returns to "no timer" rather than
    /// lingering at `0:00`. Never true for the boundary modes (they disarm via the
    /// EOF / queue-end paths).
    pub fn spent(&self) -> bool {
        matches!(self.mode, SleepMode::After(_)) && !self.fired && self.remaining <= 0.0
    }

    /// Project the snapshot the UI / CLI read.
    pub fn status(&self) -> SleepStatus {
        let remaining = match self.mode {
            SleepMode::After(_) => Some(self.remaining.max(0.0)),
            _ => None,
        };
        SleepStatus {
            mode: self.mode,
            remaining,
            fired: self.fired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A duration timer counts down only while playing and fires at zero.
    #[test]
    fn after_counts_down_and_fires() {
        let mut c = SleepClock::new(SleepMode::After(10.0));
        assert_eq!(c.status().remaining, Some(10.0));
        for _ in 0..9 {
            assert!(!c.tick(1.0, true));
        }
        assert_eq!(c.status().remaining, Some(1.0));
        // The tenth second crosses zero: it fires exactly once.
        assert!(c.tick(1.0, true));
        assert!(c.fired());
        assert_eq!(c.status().remaining, Some(0.0));
        // It does not fire again on subsequent ticks.
        assert!(!c.tick(1.0, true));
    }

    /// A pause holds the countdown: it neither expires nor advances while not
    /// playing (the session-accumulator idle precedent).
    #[test]
    fn paused_does_not_count_down() {
        let mut c = SleepClock::new(SleepMode::After(5.0));
        for _ in 0..100 {
            assert!(!c.tick(1.0, false)); // paused the whole time
        }
        assert_eq!(c.status().remaining, Some(5.0));
        assert!(!c.fired());
    }

    /// Tap-to-extend re-arms the same interval inside the window and refuses
    /// outside it.
    #[test]
    fn tap_to_extend_window() {
        let mut c = SleepClock::new(SleepMode::After(10.0));
        while !c.tick(1.0, true) {} // run it to fire
        assert!(c.fired());
        // 20 s later (still inside the 30 s window): a play re-arms the full 10 s.
        c.tick(20.0, false);
        assert!(c.on_play());
        assert_eq!(c.status().remaining, Some(10.0));
        assert!(!c.fired());

        // Fire again, then let the window lapse: a play no longer extends, and the
        // clock is now spent (the engine disarms it).
        while !c.tick(1.0, true) {}
        assert!(!c.spent(), "still within the window, not spent");
        c.tick(TAP_TO_EXTEND_WINDOW, false); // window lapses, fired self-clears
        assert!(!c.fired());
        assert!(c.spent());
        assert!(!c.on_play());
    }

    /// `on_play` is a no-op before firing and for the boundary modes.
    #[test]
    fn on_play_noop_when_not_fired() {
        let mut after = SleepClock::new(SleepMode::After(10.0));
        assert!(!after.on_play());
        assert_eq!(after.status().remaining, Some(10.0));

        let mut eoi = SleepClock::new(SleepMode::EndOfItem);
        assert!(!eoi.tick(100.0, true)); // boundary modes never tick down
        assert!(!eoi.on_play());
    }

    /// The boundary modes report no countdown and are never "spent" (they disarm
    /// via the engine's EOF / queue-end paths, not the clock).
    #[test]
    fn boundary_modes_have_no_remaining() {
        let eoi = SleepClock::new(SleepMode::EndOfItem);
        assert_eq!(eoi.status().remaining, None);
        assert!(!eoi.spent());
        let eoq = SleepClock::new(SleepMode::EndOfQueue);
        assert_eq!(eoq.status().remaining, None);
        assert!(!eoq.spent());
    }

    /// A freshly armed duration timer is not spent.
    #[test]
    fn fresh_after_is_not_spent() {
        assert!(!SleepClock::new(SleepMode::After(10.0)).spent());
    }

    /// The status projection carries the mode through unchanged.
    #[test]
    fn status_projection() {
        let c = SleepClock::new(SleepMode::After(42.0));
        let s = c.status();
        assert_eq!(s.mode, SleepMode::After(42.0));
        assert_eq!(s.remaining, Some(42.0));
        assert!(!s.fired);
    }
}
