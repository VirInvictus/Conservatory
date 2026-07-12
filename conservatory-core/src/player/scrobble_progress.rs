//! Scrobble submission-rule accounting (Phase 9d, spec §14 carve-out).
//!
//! Pure: no libmpv, no DB. The engine builds one when a scrobbleable item (a
//! music track or a podcast episode) starts playing, feeds it wall-clock
//! playtime each tick, and asks [`eligible`](ScrobbleProgress::eligible) when the
//! item leaves the current slot (natural end, skip, stop). It is the piece that
//! replaces Phase 9b's end-of-file-only scrobble with the real rule.
//!
//! The rule is the AudioScrobbler 2.0 convention every reference scrobbler uses
//! (verified against DeaDBeeF's `lastfm.c` and matched by foobar2000's
//! `foo_scrobble`): a track longer than 30 seconds is scrobbled once it has been
//! played for at least **half its length or four minutes, whichever comes
//! first**. Shorter tracks never scrobble. This is deliberately decoupled from
//! the local `play_count` bump, which stays end-of-file only (a "play" for the
//! library is a finished track; a "listen" for a history service is this rule).
//!
//! Playtime is *time spent playing*, not playhead position: a seek does not
//! change it, and the engine only ticks it while actually playing, so a pause
//! cannot accrue playtime. That mirrors DeaDBeeF's `playtime` field exactly.

use crate::db::MediaKind;

/// A track shorter than this (with a known duration) is never scrobbled.
const MIN_TRACK_SECS: f64 = 30.0;
/// Playtime at which any track becomes eligible regardless of its length.
const SCROBBLE_AFTER_SECS: f64 = 240.0;

/// Tracks one scrobbleable play's progress toward the submission threshold.
/// Constructed at load with the play's start timestamp (the timestamp the listen
/// is stamped with, per the protocol) and the track duration if known.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrobbleProgress {
    /// The item kind (`Track` or `Episode`; audiobooks are never scrobbled).
    pub kind: MediaKind,
    /// The per-kind id (track id, or episode id for an episode).
    pub id: i64,
    /// Unix seconds when the play began: the listen's `listened_at` (Last.fm and
    /// ListenBrainz both key a scrobble by its *start* time, not its finish).
    pub started_at: i64,
    /// The track duration in seconds, or 0 until the host reports it.
    duration: f64,
    /// Accumulated wall-clock seconds actually played (pause- and seek-immune).
    playtime: f64,
    /// Set when the track reached its natural end (a full listen): the engine
    /// marks this on EOF, so a fully-played track qualifies on the strength of
    /// completion rather than accumulated wall-clock (the reference scrobbler's
    /// "played to the end" case). Still subject to the 30-second floor.
    completed: bool,
}

impl ScrobbleProgress {
    /// Start accounting a play. `duration` may be 0 at load (the host has not
    /// decoded it yet); [`observe_duration`](Self::observe_duration) fills it in.
    pub fn new(kind: MediaKind, id: i64, started_at: i64, duration: f64) -> Self {
        Self {
            kind,
            id,
            started_at,
            duration: duration.max(0.0),
            playtime: 0.0,
            completed: false,
        }
    }

    /// Accrue `dt` wall-clock seconds of actual playback. The engine calls this
    /// only while playing (never paused), so idle time is never counted; a
    /// non-positive delta is ignored.
    pub fn tick(&mut self, dt: f64) {
        if dt > 0.0 {
            self.playtime += dt;
        }
    }

    /// Update the known duration once the host reports it (0 right after load).
    /// Only a positive value replaces the current one, so a transient 0 from the
    /// host mid-stream cannot erase a duration already learned.
    pub fn observe_duration(&mut self, duration: f64) {
        if duration > 0.0 {
            self.duration = duration;
        }
    }

    /// Mark the track as having reached its natural end. The engine calls this on
    /// a natural EOF, so a fully-played track scrobbles on the strength of
    /// completion (subject to the 30-second floor) without depending on the exact
    /// wall-clock the tick loop happened to sample.
    pub fn mark_complete(&mut self) {
        self.completed = true;
    }

    /// Whether this play has earned a scrobble. A known duration under 30 seconds
    /// never qualifies (the reference scrobbler's floor). Otherwise it qualifies
    /// three ways: the track reached its natural end (a full listen), playtime
    /// reached four minutes, or playtime reached half a known duration. An unknown
    /// duration (a stream) falls back to completion or the four-minute rule, since
    /// "half" cannot be computed.
    pub fn eligible(&self) -> bool {
        if self.duration > 0.0 && self.duration < MIN_TRACK_SECS {
            return false;
        }
        if self.completed || self.playtime >= SCROBBLE_AFTER_SECS {
            return true;
        }
        self.duration > 0.0 && self.playtime >= self.duration / 2.0
    }

    #[cfg(test)]
    pub fn playtime(&self) -> f64 {
        self.playtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(duration: f64) -> ScrobbleProgress {
        ScrobbleProgress::new(MediaKind::Track, 1, 1_700_000_000, duration)
    }

    #[test]
    fn short_track_never_scrobbles() {
        let mut p = prog(20.0);
        p.tick(20.0); // played the whole thing
        assert!(!p.eligible());
    }

    #[test]
    fn half_rule_for_a_short_ish_track() {
        // A 100s track: half is 50s. Eligible at 50, not at 49.
        let mut p = prog(100.0);
        p.tick(49.0);
        assert!(!p.eligible());
        p.tick(1.0); // now 50
        assert!(p.eligible());
    }

    #[test]
    fn four_minute_rule_for_a_long_track() {
        // A 20-minute track: half is 600s, but 240s of playtime is enough.
        let mut p = prog(1200.0);
        p.tick(239.0);
        assert!(!p.eligible());
        p.tick(1.0); // 240
        assert!(p.eligible());
    }

    #[test]
    fn unknown_duration_uses_the_four_minute_rule() {
        let mut p = prog(0.0); // a stream: duration never learned
        p.tick(239.0);
        assert!(!p.eligible());
        p.tick(1.0);
        assert!(p.eligible());
    }

    #[test]
    fn exactly_thirty_seconds_is_allowed() {
        // The floor is "under 30s"; a 30s track is fine, half is 15s.
        let mut p = prog(30.0);
        p.tick(15.0);
        assert!(p.eligible());
    }

    #[test]
    fn duration_learned_after_load_gates_correctly() {
        let mut p = prog(0.0); // 0 at load
        p.tick(20.0);
        p.observe_duration(40.0); // host now reports 40s; half is 20s
        assert!(p.eligible());
        // A transient 0 does not erase the learned duration.
        p.observe_duration(0.0);
        assert!(p.eligible());
    }

    #[test]
    fn completion_scrobbles_a_full_length_track_without_wall_clock() {
        // A natural EOF on a 200s track: eligible on completion alone, even with
        // no accumulated playtime (the fast-playback / missed-tick case).
        let mut p = prog(200.0);
        assert!(!p.eligible());
        p.mark_complete();
        assert!(p.eligible());
    }

    #[test]
    fn completion_still_respects_the_thirty_second_floor() {
        // A fully-played sub-30s track (an interlude / skit) is not a scrobble,
        // matching the reference scrobbler's default.
        let mut p = prog(10.0);
        p.tick(10.0);
        p.mark_complete();
        assert!(!p.eligible());
    }

    #[test]
    fn pause_and_seek_do_not_accrue_playtime() {
        let mut p = prog(100.0);
        p.tick(10.0);
        p.tick(0.0); // a zero-delta idle tick
        p.tick(-5.0); // defensive: never subtract
        assert_eq!(p.playtime(), 10.0);
    }
}
