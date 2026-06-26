//! The labelled `af` filter-chain builder (Phase 5.5a, docs/libmpv-profiles.md).
//!
//! Pure: turns a [`MusicProfile`] into the string set on mpv's `af` property. The
//! chain is built **once per item** with labelled stages so later phases can
//! mutate a single stage's parameters via `af-command` without rebuilding the
//! graph (which would gap the audio). Stage order is signal flow:
//!
//! - `@rg`  — ReplayGain, an explicit `volume` at the chain *head* (Phase 5.5a).
//!   Recomputed per track, which is what fixes mpv #8267 (its built-in
//!   `--replaygain` sits after the chain and inherits track 1's gain across a
//!   gapless boundary).
//! - `@eq`   — the graphic / parametric equalizer (Phase 5.5b).
//! - `@comp` — the compressor (`acompressor`, Phase 5.5c).
//! - `@limit`— the brick-wall limiter (`alimiter`, Phase 5.5c; also the
//!   ReplayGain clip safety net when `replaygain_clip` is off).
//! - `@boost`— the `dynaudnorm` leveler / Voice Boost (Phase 5.5c / 6c).
//!
//! Speed is **not** a stage: mpv auto-inserts `scaletempo2` on `--speed`
//! (`audio-pitch-correction`), so it stays a flat property on the host.

use crate::db::models::{DspState, EQ_CENTRES, EqState};
use crate::player::dsp::{comp_stage, leveler_stage, limiter_stage};
use crate::player::profile::MusicProfile;
use crate::player::spoken::{smart_speed_stage, voice_boost_stages};

/// Build the mpv `af` chain string for `profile` + the active `eq` + the `dsp`
/// modules. Returns `""` when no stages are active (which clears mpv's `af`).
/// 5.5a added the `@rg` head stage; 5.5b added `@eq` (the graphic equalizer);
/// 5.5c adds the `@comp` / `@limit` / `@boost` dynamics stages. Stage order is
/// signal flow: ReplayGain → EQ → compressor → limiter → leveler (spec §6.2).
pub fn build_af_chain(profile: &MusicProfile, eq: &EqState, dsp: &DspState) -> String {
    let mut stages: Vec<String> = Vec::new();

    // @rg: ReplayGain as a head-of-chain volume (dB). A bridged ffmpeg `volume`
    // filter via mpv's `lavfi` so the dB form is accepted directly.
    if let Some(db) = profile.replaygain_db {
        stages.push(format!("@rg:lavfi=[volume={}dB]", fmt_db(db)));
    }

    // @eq: the graphic equalizer (a flat EQ contributes no stage — the no-op
    // chain). Each band is a named `equalizer` peaking filter so 5.5b-ii can
    // address it live via `af-command`.
    if let Some(stage) = eq_stage(eq) {
        stages.push(stage);
    }

    // @comp / @limit / @boost: the dynamics modules (Phase 5.5c), each present
    // only when its module is enabled (an off module contributes no stage).
    stages.extend(comp_stage(&dsp.comp));
    stages.extend(limiter_stage(&dsp.limiter));
    stages.extend(leveler_stage(&dsp.leveler));

    // @ss / @vb*: the spoken-word presets (Phase 6c), appended after the music
    // stages. Only an episode profile sets these flags, so a music chain is
    // unchanged. Smart Speed precedes Voice Boost so the compressor does not
    // raise the noise floor before the silence detector runs.
    stages.extend(smart_speed_stage(profile.smart_speed));
    stages.extend(voice_boost_stages(profile.voice_boost));

    stages.join(",")
}

/// The `@eq` stage for `eq`, or `None` when the EQ is flat (the no-op chain). A
/// stack of named `equalizer` peaking bands at the ISO centres, each one octave
/// wide, under a single `@eq` lavfi label.
pub fn eq_stage(eq: &EqState) -> Option<String> {
    if eq.is_flat() {
        return None;
    }
    let bands: Vec<String> = EQ_CENTRES
        .iter()
        .zip(eq.bands.iter())
        .enumerate()
        .map(|(i, (centre, gain))| format!("equalizer@b{i}=f={centre}:t=o:w=1:g={}", fmt_db(*gain)))
        .collect();
    Some(format!("@eq:lavfi=[{}]", bands.join(",")))
}

/// The mpv `af-command` arguments to set EQ band `index` to `gain` dB live
/// (Phase 5.5b-ii): `(label, command, argument, target)` =
/// `("@eq", "gain", "<dB>", "b<index>")`. The target names the `equalizer@b<n>`
/// instance inside the `@eq` lavfi graph (see [`eq_stage`]). Pure.
pub fn eq_band_command(index: usize, gain: f64) -> (&'static str, &'static str, String, String) {
    ("@eq", "gain", fmt_db(gain), format!("b{index}"))
}

/// Format a dB value for the filter string with a minimal representation
/// (`-6.0` → `-6`, `-6.5` → `-6.5`), so the chain string is stable and readable.
/// Shared with [`crate::player::dsp`] (the DSP stage builders, Phase 5.5c).
pub(crate) fn fmt_db(db: f64) -> String {
    // Round to 0.01 dB to avoid float-noise like `-6.0000001` in the string.
    let rounded = (db * 100.0).round() / 100.0;
    format!("{rounded}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        CompSettings, EQ_BAND_COUNT, LevelerSettings, LimiterSettings, ModuleState,
    };

    fn profile(replaygain_db: Option<f64>) -> MusicProfile {
        MusicProfile {
            gapless: true,
            replaygain_db,
            speed: 1.0,
            pitch_correction: false,
            smart_speed: false,
            voice_boost: false,
        }
    }

    fn flat() -> EqState {
        EqState::flat()
    }

    fn off() -> DspState {
        DspState::off()
    }

    #[test]
    fn replaygain_head_stage_is_emitted() {
        assert_eq!(
            build_af_chain(&profile(Some(-6.0)), &flat(), &off()),
            "@rg:lavfi=[volume=-6dB]"
        );
        assert_eq!(
            build_af_chain(&profile(Some(-6.5)), &flat(), &off()),
            "@rg:lavfi=[volume=-6.5dB]"
        );
    }

    #[test]
    fn no_replaygain_and_flat_eq_is_an_empty_chain() {
        assert_eq!(build_af_chain(&profile(None), &flat(), &off()), "");
    }

    #[test]
    fn different_gains_produce_different_chains() {
        // The per-track recompute that fixes mpv #8267: each item's head volume
        // is its own, so two tracks with different gains never share a chain.
        let a = build_af_chain(&profile(Some(-6.0)), &flat(), &off());
        let b = build_af_chain(&profile(Some(-3.0)), &flat(), &off());
        assert_ne!(a, b);
        assert_eq!(b, "@rg:lavfi=[volume=-3dB]");
    }

    #[test]
    fn float_noise_is_rounded_out() {
        // -6.9 + 0.1 style arithmetic should not leak a long decimal.
        assert_eq!(
            build_af_chain(&profile(Some(-6.9 + 0.1)), &flat(), &off()),
            "@rg:lavfi=[volume=-6.8dB]"
        );
    }

    #[test]
    fn flat_eq_contributes_no_stage() {
        assert_eq!(eq_stage(&flat()), None);
    }

    #[test]
    fn nonflat_eq_emits_named_bands_at_iso_centres() {
        let mut eq = EqState::flat();
        eq.bands[0] = 6.0; // 31 Hz +6 dB
        eq.bands[9] = -4.5; // 16 kHz -4.5 dB
        let stage = eq_stage(&eq).expect("non-flat EQ has a stage");
        assert!(stage.starts_with("@eq:lavfi=["));
        assert!(stage.contains("equalizer@b0=f=31:t=o:w=1:g=6"));
        assert!(stage.contains("equalizer@b9=f=16000:t=o:w=1:g=-4.5"));
        // All ten bands are present.
        assert_eq!(stage.matches("equalizer@b").count(), EQ_BAND_COUNT);
    }

    #[test]
    fn eq_band_command_targets_the_named_band() {
        // The roadmap guard: a band change maps to the expected `af-command`.
        let (label, cmd, arg, target) = eq_band_command(3, -4.5);
        assert_eq!(label, "@eq");
        assert_eq!(cmd, "gain");
        assert_eq!(arg, "-4.5");
        assert_eq!(target, "b3");
        // Integer gains render minimally.
        assert_eq!(eq_band_command(0, 6.0).2, "6");
    }

    #[test]
    fn rg_and_eq_compose_in_order() {
        let mut eq = EqState::flat();
        eq.bands[4] = 3.0;
        let chain = build_af_chain(&profile(Some(-6.0)), &eq, &off());
        // @rg precedes @eq (signal-flow order).
        let rg = chain.find("@rg").unwrap();
        let e = chain.find("@eq").unwrap();
        assert!(rg < e, "ReplayGain head stage precedes the EQ");
    }

    #[test]
    fn full_chain_is_in_signal_flow_order() {
        // @rg → @eq → @comp → @limit → @boost (spec §6.2).
        let mut eq = EqState::flat();
        eq.bands[4] = 3.0;
        let dsp = DspState {
            comp: ModuleState {
                enabled: true,
                settings: CompSettings::default(),
            },
            limiter: ModuleState {
                enabled: true,
                settings: LimiterSettings::default(),
            },
            leveler: ModuleState {
                enabled: true,
                settings: LevelerSettings::default(),
            },
        };
        let chain = build_af_chain(&profile(Some(-6.0)), &eq, &dsp);
        let positions: Vec<usize> = ["@rg", "@eq", "@comp", "@limit", "@boost"]
            .iter()
            .map(|label| {
                chain
                    .find(label)
                    .unwrap_or_else(|| panic!("{label} missing from {chain}"))
            })
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "stages out of order: {chain}"
        );
    }

    #[test]
    fn disabled_dsp_adds_nothing_to_the_chain() {
        let chain = build_af_chain(&profile(Some(-6.0)), &flat(), &off());
        assert_eq!(chain, "@rg:lavfi=[volume=-6dB]");
    }

    #[test]
    fn spoken_word_stages_append_after_music() {
        // An episode profile (Phase 6c): @ss then the @vb* group, after the music
        // stages. Smart Speed precedes Voice Boost (the compressor must not raise
        // the noise floor before silence detection).
        let mut p = profile(None);
        p.smart_speed = true;
        p.voice_boost = true;
        let chain = build_af_chain(&p, &flat(), &off());
        assert!(chain.contains("@ss:lavfi=[silenceremove="), "{chain}");
        assert!(chain.contains("@vbcomp:lavfi=[acompressor="), "{chain}");
        assert!(chain.contains("@vbnorm:lavfi=[dynaudnorm="), "{chain}");
        let ss = chain.find("@ss").unwrap();
        let vb = chain.find("@vbcomp").unwrap();
        assert!(ss < vb, "Smart Speed precedes Voice Boost: {chain}");
    }

    #[test]
    fn music_profile_emits_no_spoken_word_stages() {
        // The no-regression guard: a music profile leaves the flags false, so the
        // chain is exactly the 5.5 chain (no @ss / @vb).
        let chain = build_af_chain(&profile(Some(-6.0)), &flat(), &off());
        assert!(!chain.contains("@ss"), "{chain}");
        assert!(!chain.contains("@vb"), "{chain}");
        assert_eq!(chain, "@rg:lavfi=[volume=-6dB]");
    }
}
