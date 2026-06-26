//! Smart Speed time-saved accounting (spec §6.3, Phase 6c-ii).
//!
//! Pure: no libmpv, no DB. The engine feeds it a wall-clock delta and the
//! current playhead each tick; it accumulates real vs audio time and reports the
//! wall-clock time Smart Speed saved. At every episode boundary the engine reads
//! the totals off the accumulator and writes one append-only `listening_sessions`
//! row (the CLAUDE.md rule: the math lives in core and is unit-tested headless;
//! the host is thin glue).
//!
//! The math has to survive `silenceremove`'s non-linear timeline. With Smart
//! Speed on, the filter drops dead air, so the playhead jumps forward faster than
//! wall-clock: those jumps **are** audio the listener covered for free, so they
//! count toward `audio_seconds`. A user seek is a jump that is *not* audio played,
//! so it is excluded (the engine flags it via [`SessionAccumulator::seek`]). The
//! saved figure is then how much wall-clock the covered audio would have taken at
//! the playback speed, minus the wall-clock actually spent.

/// Accumulates one episode's listening session: real (wall-clock) time, audio
/// time covered, and the playback speed they were sampled at. Construct it when
/// an episode loads, `tick` it during steady playback, and read
/// [`smart_speed_saved`](Self::smart_speed_saved) when the episode ends.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAccumulator {
    pub episode_id: i64,
    /// Unix seconds when the session began (the engine's `now_secs()`).
    pub started_at: i64,
    speed: f64,
    last_pos: f64,
    /// Set by `seek`: the next `tick`'s position delta is a user jump, not audio.
    skip_next: bool,
    real_seconds: f64,
    audio_seconds: f64,
}

impl SessionAccumulator {
    /// Start a session for `episode_id` at `start_pos` (the resume offset), sampled
    /// against `speed` (the resolved per-show playback rate). A non-positive speed
    /// degrades to 1.0 so the saved division can never divide by zero.
    pub fn new(episode_id: i64, started_at: i64, speed: f64, start_pos: f64) -> Self {
        Self {
            episode_id,
            started_at,
            speed: if speed > 0.0 { speed } else { 1.0 },
            last_pos: start_pos.max(0.0),
            skip_next: false,
            real_seconds: 0.0,
            audio_seconds: 0.0,
        }
    }

    /// One steady-playback sample: `dt_real` wall-clock seconds since the last
    /// tick, `pos` the current playhead. Real time always accrues; audio time
    /// accrues from the forward playhead delta, **including the jumps
    /// `silenceremove` makes** when it drops dead air (that skipped span is the
    /// audio Smart Speed covered for free). A flagged user seek (see [`seek`]) has
    /// its interval excluded, so a manual scrub does not inflate the audio total.
    ///
    /// [`seek`]: Self::seek
    pub fn tick(&mut self, dt_real: f64, pos: f64) {
        if dt_real > 0.0 {
            self.real_seconds += dt_real;
        }
        if self.skip_next {
            self.skip_next = false;
        } else {
            let advance = pos - self.last_pos;
            if advance > 0.0 {
                self.audio_seconds += advance;
            }
        }
        self.last_pos = pos;
    }

    /// A user seek landed at `pos`: resync the playhead and exclude the next
    /// interval's delta (it is a jump the user made, not audio played).
    pub fn seek(&mut self, pos: f64) {
        self.skip_next = true;
        self.last_pos = pos.max(0.0);
    }

    /// Resync the playhead without accumulating (used while paused / ended, so the
    /// gap until a resume neither inflates real time nor counts as audio).
    pub fn resync(&mut self, pos: f64) {
        self.last_pos = pos;
    }

    /// Wall-clock seconds Smart Speed saved: the time the covered audio would have
    /// taken at the playback speed (`audio_seconds / speed`) minus the wall-clock
    /// actually spent. Never negative; a session with no skipped silence is ≈ 0
    /// (at steady speed the playhead advances `speed ×` real time, so the two
    /// terms cancel and only `silenceremove`'s extra jumps produce a surplus).
    pub fn smart_speed_saved(&self) -> f64 {
        (self.audio_seconds / self.speed - self.real_seconds).max(0.0)
    }

    pub fn real_seconds(&self) -> f64 {
        self.real_seconds
    }

    pub fn audio_seconds(&self) -> f64 {
        self.audio_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady playback at 1× with no silence: audio tracks real, saved ≈ 0.
    #[test]
    fn no_silence_saves_nothing() {
        let mut acc = SessionAccumulator::new(1, 0, 1.0, 0.0);
        for i in 1..=10 {
            acc.tick(1.0, i as f64); // one wall-clock second, one audio second
        }
        assert_eq!(acc.real_seconds(), 10.0);
        assert_eq!(acc.audio_seconds(), 10.0);
        assert!(acc.smart_speed_saved().abs() < 1e-9);
    }

    /// Variable speed alone is not "saved": at 2× the playhead advances twice as
    /// fast, so `audio/speed` cancels the real time (Smart Speed accounts only for
    /// the silence `silenceremove` drops, not the speed-up).
    #[test]
    fn variable_speed_alone_saves_nothing() {
        let mut acc = SessionAccumulator::new(1, 0, 2.0, 0.0);
        for i in 1..=10 {
            acc.tick(1.0, (i * 2) as f64); // 2 audio seconds per wall-clock second
        }
        assert_eq!(acc.real_seconds(), 10.0);
        assert_eq!(acc.audio_seconds(), 20.0);
        assert!(acc.smart_speed_saved().abs() < 1e-9);
    }

    /// A `silenceremove` jump (the playhead leaps over dead air in one tick) is
    /// counted as covered audio, producing a positive saved figure equal to the
    /// dropped silence.
    #[test]
    fn silence_jump_is_counted_as_saved() {
        let mut acc = SessionAccumulator::new(1, 0, 1.0, 0.0);
        acc.tick(1.0, 1.0); // normal: audio 1, real 1
        acc.tick(1.0, 2.0); // normal: audio 2, real 2
        acc.tick(1.0, 7.0); // silence drop: playhead leaps +5, real +1
        assert_eq!(acc.real_seconds(), 3.0);
        assert_eq!(acc.audio_seconds(), 7.0);
        // 7s audio would take 7s at 1×; only 3s of wall-clock elapsed → 4s saved
        // (exactly the 4s of silence the jump skipped beyond a 1s step).
        assert!((acc.smart_speed_saved() - 4.0).abs() < 1e-9);
    }

    /// A user seek's jump is excluded: the interval right after `seek` does not
    /// add to the audio total, so scrubbing forward never inflates saved.
    #[test]
    fn user_seek_is_excluded() {
        let mut acc = SessionAccumulator::new(1, 0, 1.0, 0.0);
        acc.tick(1.0, 1.0); // audio 1, real 1
        acc.seek(10.0); // user jumped to 10s
        acc.tick(1.0, 11.0); // excluded delta: audio stays 1, real 2
        acc.tick(1.0, 12.0); // normal again: audio 2, real 3
        assert_eq!(acc.audio_seconds(), 2.0);
        assert_eq!(acc.real_seconds(), 3.0);
        assert!(acc.smart_speed_saved().abs() < 1e-9);
    }

    /// A pause (engine calls `resync` each idle tick) neither advances real time
    /// nor counts as audio, so a long pause cannot inflate the session.
    #[test]
    fn pause_resync_accumulates_nothing() {
        let mut acc = SessionAccumulator::new(1, 0, 1.0, 0.0);
        acc.tick(1.0, 1.0); // audio 1, real 1
        for _ in 0..100 {
            acc.resync(1.0); // paused at 1.0
        }
        acc.tick(1.0, 2.0); // resume: audio 2, real 2 (no jump from the pause)
        assert_eq!(acc.real_seconds(), 2.0);
        assert_eq!(acc.audio_seconds(), 2.0);
    }

    /// A zero / negative speed cannot divide-by-zero; it degrades to 1×.
    #[test]
    fn nonpositive_speed_degrades_to_one() {
        let mut acc = SessionAccumulator::new(1, 0, 0.0, 0.0);
        acc.tick(1.0, 5.0); // 5s audio jump
        assert!((acc.smart_speed_saved() - 4.0).abs() < 1e-9);
    }
}
