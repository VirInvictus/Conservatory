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
//! - `@comp` — compressor / brick-wall limiter (Phase 5.5c; also the ReplayGain
//!   clip safety net when `replaygain_clip` is off).
//! - `@boost`— the `dynaudnorm` leveler / Voice Boost (Phase 5.5c / 6c).
//!
//! Speed is **not** a stage: mpv auto-inserts `scaletempo2` on `--speed`
//! (`audio-pitch-correction`), so it stays a flat property on the host.

use crate::player::profile::MusicProfile;

/// Build the mpv `af` chain string for `profile`. Returns `""` when no stages are
/// active (which clears mpv's `af`). Phase 5.5a emits only the `@rg` head stage;
/// `@eq` / `@comp` / `@boost` join here in 5.5b/c.
pub fn build_af_chain(profile: &MusicProfile) -> String {
    let mut stages: Vec<String> = Vec::new();

    // @rg: ReplayGain as a head-of-chain volume (dB). A bridged ffmpeg `volume`
    // filter via mpv's `lavfi` so the dB form is accepted directly.
    if let Some(db) = profile.replaygain_db {
        stages.push(format!("@rg:lavfi=[volume={}dB]", fmt_db(db)));
    }

    stages.join(",")
}

/// Format a dB value for the filter string with a minimal representation
/// (`-6.0` → `-6`, `-6.5` → `-6.5`), so the chain string is stable and readable.
fn fmt_db(db: f64) -> String {
    // Round to 0.01 dB to avoid float-noise like `-6.0000001` in the string.
    let rounded = (db * 100.0).round() / 100.0;
    format!("{rounded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(replaygain_db: Option<f64>) -> MusicProfile {
        MusicProfile {
            gapless: true,
            replaygain_db,
            speed: 1.0,
            pitch_correction: false,
        }
    }

    #[test]
    fn replaygain_head_stage_is_emitted() {
        assert_eq!(
            build_af_chain(&profile(Some(-6.0))),
            "@rg:lavfi=[volume=-6dB]"
        );
        assert_eq!(
            build_af_chain(&profile(Some(-6.5))),
            "@rg:lavfi=[volume=-6.5dB]"
        );
    }

    #[test]
    fn no_replaygain_is_an_empty_chain() {
        assert_eq!(build_af_chain(&profile(None)), "");
    }

    #[test]
    fn different_gains_produce_different_chains() {
        // The per-track recompute that fixes mpv #8267: each item's head volume
        // is its own, so two tracks with different gains never share a chain.
        let a = build_af_chain(&profile(Some(-6.0)));
        let b = build_af_chain(&profile(Some(-3.0)));
        assert_ne!(a, b);
        assert_eq!(b, "@rg:lavfi=[volume=-3dB]");
    }

    #[test]
    fn float_noise_is_rounded_out() {
        // -6.9 + 0.1 style arithmetic should not leak a long decimal.
        assert_eq!(
            build_af_chain(&profile(Some(-6.9 + 0.1))),
            "@rg:lavfi=[volume=-6.8dB]"
        );
    }
}
