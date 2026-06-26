//! The spoken-word `af`-chain stages (Phase 6c, spec §6.3, docs/libmpv-profiles.md).
//!
//! Pure, a sibling to [`crate::player::dsp`]: Smart Speed and Voice Boost are
//! **presets on the shared 5.5 chain** (not a parallel path). They are appended
//! after the music stages by [`crate::player::chain::build_af_chain`] when an
//! episode's profile asks for them; a music profile never sets the flags, so the
//! music chain is byte-for-byte unchanged. The parameters are fixed (only the
//! per-show on/off flags are user-settable), so these take a `bool` rather than a
//! settings struct.

use crate::player::chain::fmt_db;
use crate::player::dsp::{db_to_linear, fmt_lin};

/// Smart Speed (Phase 6c): remove dead air mid-stream via ffmpeg `silenceremove`
/// (`stop_periods=-1` removes every silence run, not just the leading one), or
/// `None` when off. `stop_threshold` / `stop_duration` are tuned to trim dead air
/// without clipping natural pauses, and `stop_silence` leaves a short beat so a
/// cut is not jarring. `silenceremove` changes stream duration on the fly, so the
/// timeline is non-linear: the time-saved accounting (6c-ii) and the seek math
/// account for it.
pub fn smart_speed_stage(enabled: bool) -> Option<String> {
    enabled.then(|| {
        "@ss:lavfi=[silenceremove=stop_periods=-1:stop_duration=1:stop_threshold=-40dB:stop_silence=0.3]"
            .to_string()
    })
}

/// Voice Boost (Phase 6c): a fixed spoken-word preset that makes uneven speech
/// intelligible at low volume, or an empty set when off. Three labelled stages
/// (so a later phase could tune one live): `@vbcomp` a gentle `acompressor` with
/// make-up gain, `@vbeq` a low-cut plus a presence lift, `@vbnorm` a live
/// `dynaudnorm` leveler with a tighter window than the music leveler. The leveler
/// is single-pass/live `dynaudnorm`, **not** offline `loudnorm` (spec §6.3).
pub fn voice_boost_stages(enabled: bool) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    vec![
        format!(
            "@vbcomp:lavfi=[acompressor=threshold={}:ratio=4:attack=5:release=150:makeup=2]",
            fmt_lin(db_to_linear(-24.0)),
        ),
        // Low-cut the rumble, then lift the presence band for intelligibility.
        format!(
            "@vbeq:lavfi=[highpass=f=80,equalizer=f=2500:t=o:w=1:g={}]",
            fmt_db(3.0),
        ),
        "@vbnorm:lavfi=[dynaudnorm=g=15:p=0.9]".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_speed_off_contributes_no_stage() {
        assert!(smart_speed_stage(false).is_none());
    }

    #[test]
    fn smart_speed_on_is_a_silenceremove_stage() {
        let stage = smart_speed_stage(true).expect("enabled Smart Speed has a stage");
        assert!(stage.starts_with("@ss:lavfi=[silenceremove="));
        // Mid-stream removal (negative stop_periods) is what removes dead air
        // throughout the episode, not just at the start.
        assert!(stage.contains("stop_periods=-1"), "{stage}");
        assert!(stage.contains("stop_threshold=-40dB"), "{stage}");
        assert!(stage.contains("stop_silence=0.3"), "{stage}");
    }

    #[test]
    fn voice_boost_off_is_empty() {
        assert!(voice_boost_stages(false).is_empty());
    }

    #[test]
    fn voice_boost_on_is_comp_eq_norm_in_order() {
        let stages = voice_boost_stages(true);
        assert_eq!(stages.len(), 3);
        assert!(stages[0].starts_with("@vbcomp:lavfi=[acompressor="));
        // -24 dB threshold → ~0.0631 linear (the acompressor form, like @comp).
        assert!(stages[0].contains("threshold=0.0631"), "{}", stages[0]);
        assert!(stages[0].contains("makeup=2"), "{}", stages[0]);
        assert!(stages[1].starts_with("@vbeq:lavfi=[highpass=f=80,equalizer="));
        assert!(stages[1].contains("g=3"), "{}", stages[1]);
        assert_eq!(stages[2], "@vbnorm:lavfi=[dynaudnorm=g=15:p=0.9]");
    }
}
