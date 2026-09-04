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

use crate::db::models::{DspState, EQ_CENTRES, EqState, PeqBand};
use crate::player::dsp::{comp_stage, leveler_stage, limiter_stage};
use crate::player::profile::MusicProfile;
use crate::player::spoken::{SmartSpeedLevel, smart_speed_stage, voice_boost_stages};

/// Build the mpv `af` chain string for `profile` + the active `eq` (graphic and
/// parametric bands) + the `dsp` modules + the Smart Speed `level`. Returns `""`
/// when no stages are active (which clears mpv's `af`). 5.5a added the `@rg`
/// head stage; 5.5b added `@eq` (the graphic equalizer, later joined by the
/// parametric bands in the same stage); 5.5c adds the `@comp` / `@limit` /
/// `@boost` dynamics stages. Stage order is signal flow: ReplayGain → EQ →
/// compressor → limiter → leveler (spec §6.2). `level` only matters when
/// `profile.smart_speed` is on.
pub fn build_af_chain(
    profile: &MusicProfile,
    eq: &EqState,
    peq: &[PeqBand],
    dsp: &DspState,
    smart_speed_level: SmartSpeedLevel,
) -> String {
    let mut stages: Vec<String> = Vec::new();

    // @rg: ReplayGain as a head-of-chain volume (dB). A bridged ffmpeg `volume`
    // filter via mpv's `lavfi` so the dB form is accepted directly.
    if let Some(db) = profile.replaygain_db {
        stages.push(format!("@rg:lavfi=[volume={}dB]", fmt_db(db)));
    }

    // @eq: the graphic equalizer plus the parametric bands (a flat graphic EQ
    // and no parametric bands contribute no stage — the no-op chain). Each band
    // is a named `equalizer` peaking filter so the live `af-command` path can
    // address it.
    if let Some(stage) = eq_stage(eq, peq) {
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
    stages.extend(smart_speed_stage(profile.smart_speed, smart_speed_level));
    stages.extend(voice_boost_stages(profile.voice_boost));

    stages.join(",")
}

/// The `@eq` stage for the graphic `eq` plus the parametric `peq` bands, or
/// `None` when the graphic EQ is flat and no parametric bands are defined (the
/// no-op chain). The graphic bands are named `equalizer@b<i>` at the ISO
/// centres, one octave wide; the parametric bands are named `equalizer@p<idx>`
/// at their arbitrary centre with their Q, and sort after the graphic bands.
/// All under a single `@eq` lavfi label.
pub fn eq_stage(eq: &EqState, peq: &[PeqBand]) -> Option<String> {
    let mut filters: Vec<String> = Vec::new();
    if !eq.is_flat() {
        filters.extend(EQ_CENTRES.iter().zip(eq.bands.iter()).enumerate().map(
            |(i, (centre, gain))| format!("equalizer@b{i}=f={centre}:t=o:w=1:g={}", fmt_db(*gain)),
        ));
    }
    for band in peq {
        filters.push(format!(
            "equalizer@p{}=f={}:t=q:q={}:g={}",
            band.idx,
            band.frequency,
            band.q,
            fmt_db(band.gain_db)
        ));
    }
    if filters.is_empty() {
        return None;
    }
    Some(format!("@eq:lavfi=[{}]", filters.join(",")))
}

/// The mpv `af-command` arguments to set EQ band `index` to `gain` dB live
/// (Phase 5.5b-ii): `(label, command, argument, target)` =
/// `("@eq", "gain", "<dB>", "b<index>")`. The target names the `equalizer@b<n>`
/// instance inside the `@eq` lavfi graph (see [`eq_stage`]). Pure.
pub fn eq_band_command(index: usize, gain: f64) -> (&'static str, &'static str, String, String) {
    ("@eq", "gain", fmt_db(gain), format!("b{index}"))
}

/// The mpv `af-command` arguments for a parametric band's live gain edit (the
/// 5.5b follow-on): `("@eq", "gain", "<dB>", "p<idx>")`, the same shipped
/// command path as [`eq_band_command`]. Frequency and Q edits are structural
/// rebuilds, not live commands. Pure.
pub fn peq_band_gain_command(idx: i64, gain: f64) -> (&'static str, &'static str, String, String) {
    ("@eq", "gain", fmt_db(gain), format!("p{idx}"))
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
            rg_album: replaygain_db,
            rg_track: replaygain_db,
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
            build_af_chain(
                &profile(Some(-6.0)),
                &flat(),
                &[],
                &off(),
                SmartSpeedLevel::default()
            ),
            "@rg:lavfi=[volume=-6dB]"
        );
        assert_eq!(
            build_af_chain(
                &profile(Some(-6.5)),
                &flat(),
                &[],
                &off(),
                SmartSpeedLevel::default()
            ),
            "@rg:lavfi=[volume=-6.5dB]"
        );
    }

    #[test]
    fn no_replaygain_and_flat_eq_is_an_empty_chain() {
        assert_eq!(
            build_af_chain(
                &profile(None),
                &flat(),
                &[],
                &off(),
                SmartSpeedLevel::default()
            ),
            ""
        );
    }

    #[test]
    fn different_gains_produce_different_chains() {
        // The per-track recompute that fixes mpv #8267: each item's head volume
        // is its own, so two tracks with different gains never share a chain.
        let a = build_af_chain(
            &profile(Some(-6.0)),
            &flat(),
            &[],
            &off(),
            SmartSpeedLevel::default(),
        );
        let b = build_af_chain(
            &profile(Some(-3.0)),
            &flat(),
            &[],
            &off(),
            SmartSpeedLevel::default(),
        );
        assert_ne!(a, b);
        assert_eq!(b, "@rg:lavfi=[volume=-3dB]");
    }

    #[test]
    fn float_noise_is_rounded_out() {
        // -6.9 + 0.1 style arithmetic should not leak a long decimal.
        assert_eq!(
            build_af_chain(
                &profile(Some(-6.9 + 0.1)),
                &flat(),
                &[],
                &off(),
                SmartSpeedLevel::default()
            ),
            "@rg:lavfi=[volume=-6.8dB]"
        );
    }

    #[test]
    fn flat_eq_contributes_no_stage() {
        assert_eq!(eq_stage(&flat(), &[]), None);
    }

    #[test]
    fn nonflat_eq_emits_named_bands_at_iso_centres() {
        let mut eq = EqState::flat();
        eq.bands[0] = 6.0; // 31 Hz +6 dB
        eq.bands[9] = -4.5; // 16 kHz -4.5 dB
        let stage = eq_stage(&eq, &[]).expect("non-flat EQ has a stage");
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
        let chain = build_af_chain(
            &profile(Some(-6.0)),
            &eq,
            &[],
            &off(),
            SmartSpeedLevel::default(),
        );
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
        let chain = build_af_chain(
            &profile(Some(-6.0)),
            &eq,
            &[],
            &dsp,
            SmartSpeedLevel::default(),
        );
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
        let chain = build_af_chain(
            &profile(Some(-6.0)),
            &flat(),
            &[],
            &off(),
            SmartSpeedLevel::default(),
        );
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
        let chain = build_af_chain(&p, &flat(), &[], &off(), SmartSpeedLevel::default());
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
        let chain = build_af_chain(
            &profile(Some(-6.0)),
            &flat(),
            &[],
            &off(),
            SmartSpeedLevel::default(),
        );
        assert!(!chain.contains("@ss"), "{chain}");
        assert!(!chain.contains("@vb"), "{chain}");
        assert_eq!(chain, "@rg:lavfi=[volume=-6dB]");
    }

    #[test]
    fn peq_bands_render_into_the_eq_stage() {
        // Parametric bands alone (flat graphic): the stage exists, named p<idx>
        // peaking biquads at their arbitrary centre and Q.
        let peq = vec![PeqBand {
            idx: 0,
            frequency: 250.0,
            q: 2.0,
            gain_db: -6.0,
        }];
        let stage = eq_stage(&EqState::flat(), &peq).expect("stage present");
        assert!(stage.contains("equalizer@p0=f=250:t=q:q=2:g=-6"), "{stage}");
        assert!(!stage.contains("equalizer@b"), "{stage}");

        // Together with a non-flat graphic EQ: graphic bands first, then p<idx>.
        let mut eq = EqState::flat();
        eq.bands[0] = 6.0;
        let stage = eq_stage(&eq, &peq).expect("stage present");
        assert!(
            stage.contains("equalizer@b0=f=31:t=o:w=1:g=6")
                && stage.contains("equalizer@p0=f=250:t=q:q=2:g=-6"),
            "{stage}"
        );

        // A defined band at 0 dB still counts as content (the user put it there).
        let zero = vec![PeqBand {
            idx: 0,
            frequency: 1000.0,
            q: 1.0,
            gain_db: 0.0,
        }];
        assert!(eq_stage(&EqState::flat(), &zero).is_some());

        // Neither present: the no-op chain.
        assert!(eq_stage(&EqState::flat(), &[]).is_none());
    }

    #[test]
    fn peq_band_gain_command_targets_the_named_band() {
        let (label, cmd, arg, target) = peq_band_gain_command(2, -4.5);
        assert_eq!(
            (label, cmd, arg.as_str(), target.as_str()),
            ("@eq", "gain", "-4.5", "p2")
        );
    }

    #[test]
    fn build_af_chain_carries_the_parametric_stage() {
        let peq = vec![PeqBand {
            idx: 1,
            frequency: 100.0,
            q: 0.5,
            gain_db: 3.0,
        }];
        let chain = build_af_chain(
            &profile(None),
            &EqState::flat(),
            &peq,
            &DspState::off(),
            SmartSpeedLevel::default(),
        );
        assert!(
            chain.contains("@eq:lavfi=[equalizer@p1=f=100:t=q:q=0.5:g=3]"),
            "{chain}"
        );
        // And with no PEQ the chain is byte-identical to the pre-PEQ shape.
        let plain = build_af_chain(
            &profile(None),
            &EqState::flat(),
            &[],
            &DspState::off(),
            SmartSpeedLevel::default(),
        );
        assert!(!plain.contains("@eq"), "{plain}");
    }
}
