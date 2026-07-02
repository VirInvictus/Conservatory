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

/// How aggressively Smart Speed trims dead air. The per-show / per-book on/off is
/// separate; this is the global gate applied wherever Smart Speed is on. The three
/// tiers are the measured presets (ffmpeg over real episodes): `Gentle` removes
/// ~0.3% on a tightly-produced show, `Aggressive` ~3.5%, at the cost of cutting
/// closer to natural speech rhythm. `Gentle` is the default (the v0.1.1 tuning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SmartSpeedLevel {
    #[default]
    Gentle,
    Balanced,
    Aggressive,
}

impl SmartSpeedLevel {
    /// The DB / config token (round-trips through `AudioState`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gentle => "gentle",
            Self::Balanced => "balanced",
            Self::Aggressive => "aggressive",
        }
    }

    /// Parse a stored token, degrading an unknown value to the default (`Gentle`)
    /// rather than failing playback on a hand-edited row.
    pub fn from_db(s: &str) -> Self {
        match s {
            "balanced" => Self::Balanced,
            "aggressive" => Self::Aggressive,
            _ => Self::Gentle,
        }
    }

    /// `(stop_duration, stop_threshold, stop_silence)` for the level, tuned against
    /// real podcasts (see the module note). A lower threshold + shorter duration +
    /// smaller retained beat trims more but risks sounding choppy.
    fn gate(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Gentle => ("0.5", "-30dB", "0.3"),
            Self::Balanced => ("0.35", "-30dB", "0.2"),
            Self::Aggressive => ("0.3", "-28dB", "0.15"),
        }
    }
}

/// Smart Speed (Phase 6c): remove dead air mid-stream via ffmpeg `silenceremove`
/// (`stop_periods=-1` removes every silence run, not just the leading one), or
/// `None` when off. `stop_silence` leaves a short beat so a cut is not jarring.
/// `silenceremove` changes stream duration on the fly, so the timeline is
/// non-linear: the time-saved accounting (6c-ii) and the seek math account for it.
///
/// The gate comes from `level`. The original `-40dB / 1s` was measured (with
/// silenceremove plus silencedetect over real podcasts) to remove *nothing*:
/// mastered speech "silence" is room tone / breaths / music beds around -25 to
/// -30 dB, not below -40 dB, and pauses are shorter than 1 s. The tiers here
/// actually trigger on real speech.
pub fn smart_speed_stage(enabled: bool, level: SmartSpeedLevel) -> Option<String> {
    enabled.then(|| {
        let (dur, threshold, silence) = level.gate();
        format!(
            "@ss:lavfi=[silenceremove=stop_periods=-1:stop_duration={dur}:stop_threshold={threshold}:stop_silence={silence}]"
        )
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
        assert!(smart_speed_stage(false, SmartSpeedLevel::Gentle).is_none());
        assert!(smart_speed_stage(false, SmartSpeedLevel::Aggressive).is_none());
    }

    #[test]
    fn smart_speed_gentle_is_the_default_gate() {
        let stage = smart_speed_stage(true, SmartSpeedLevel::default())
            .expect("enabled Smart Speed has a stage");
        assert!(stage.starts_with("@ss:lavfi=[silenceremove="));
        // Mid-stream removal (negative stop_periods) is what removes dead air
        // throughout the episode, not just at the start.
        assert!(stage.contains("stop_periods=-1"), "{stage}");
        // The Gentle default = the v0.1.1 gate that actually triggers on real
        // speech (the old -40dB gate measured to remove nothing).
        assert!(stage.contains("stop_threshold=-30dB"), "{stage}");
        assert!(stage.contains("stop_duration=0.5"), "{stage}");
        assert!(stage.contains("stop_silence=0.3"), "{stage}");
    }

    #[test]
    fn smart_speed_levels_have_distinct_gates() {
        let g = smart_speed_stage(true, SmartSpeedLevel::Gentle).unwrap();
        let b = smart_speed_stage(true, SmartSpeedLevel::Balanced).unwrap();
        let a = smart_speed_stage(true, SmartSpeedLevel::Aggressive).unwrap();
        assert!(
            b.contains("stop_duration=0.35") && b.contains("stop_threshold=-30dB"),
            "{b}"
        );
        assert!(
            a.contains("stop_duration=0.3") && a.contains("stop_threshold=-28dB"),
            "{a}"
        );
        // Punchier tiers trim more, so the gates must differ.
        assert_ne!(g, b);
        assert_ne!(b, a);
    }

    #[test]
    fn smart_speed_level_round_trips_through_db_token() {
        for lvl in [
            SmartSpeedLevel::Gentle,
            SmartSpeedLevel::Balanced,
            SmartSpeedLevel::Aggressive,
        ] {
            assert_eq!(SmartSpeedLevel::from_db(lvl.as_str()), lvl);
        }
        // An unknown / hand-edited token degrades to the default.
        assert_eq!(
            SmartSpeedLevel::from_db("nonsense"),
            SmartSpeedLevel::Gentle
        );
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
