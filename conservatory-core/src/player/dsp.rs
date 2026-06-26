//! The DSP-module `af`-chain stage builders (Phase 5.5c, docs/libmpv-profiles.md).
//!
//! Pure: each function turns one [`ModuleState`] into the `lavfi` stage string it
//! contributes to the chain ([`crate::player::chain::build_af_chain`]), or `None`
//! when the module is disabled (no stage). Like the EQ, the modules are built
//! once per item and rebuilt on a settings change (a structural `af` rebuild is
//! acceptable for an explicit settings change per the chain discipline; DSP has
//! no slider-drag-while-listening path like the EQ's `af-command`).
//!
//! The dynamics stages sit after the EQ in signal-flow order: compressor
//! (`@comp`) → brick-wall limiter (`@limit`) → leveler (`@boost`). User-facing
//! dB knobs (compressor threshold, limiter ceiling) are converted to the ffmpeg
//! filters' linear forms here, the one place the unit mapping lives.

use crate::db::models::{CompSettings, LevelerSettings, LimiterSettings, ModuleState};
use crate::player::chain::fmt_db;

/// The `@comp` compressor stage (`acompressor`), or `None` when disabled. The
/// threshold is stored in dBFS and converted to the filter's linear `threshold`.
pub fn comp_stage(m: &ModuleState<CompSettings>) -> Option<String> {
    if !m.enabled {
        return None;
    }
    let s = m.settings;
    Some(format!(
        "@comp:lavfi=[acompressor=threshold={}:ratio={}:attack={}:release={}]",
        fmt_lin(db_to_linear(s.threshold_db)),
        fmt_num(s.ratio),
        fmt_num(s.attack_ms),
        fmt_num(s.release_ms),
    ))
}

/// The `@limit` brick-wall limiter stage (`alimiter`), or `None` when disabled.
/// `ceiling_db` (dBFS) becomes the filter's linear `limit`. `level=disabled`
/// keeps it a pure peak catcher (no make-up normalization), so it is transparent
/// until the signal actually exceeds the ceiling — which is what makes it a safe
/// ReplayGain clip net.
pub fn limiter_stage(m: &ModuleState<LimiterSettings>) -> Option<String> {
    if !m.enabled {
        return None;
    }
    Some(format!(
        "@limit:lavfi=[alimiter=limit={}:level=disabled]",
        fmt_lin(db_to_linear(m.settings.ceiling_db)),
    ))
}

/// The `@boost` volume-leveler stage (`dynaudnorm`, single-pass/live), or `None`
/// when disabled. `g` is the Gaussian window size (smoothing); `p` the target
/// peak.
pub fn leveler_stage(m: &ModuleState<LevelerSettings>) -> Option<String> {
    if !m.enabled {
        return None;
    }
    let s = m.settings;
    Some(format!(
        "@boost:lavfi=[dynaudnorm=g={}:p={}]",
        s.gausssize,
        fmt_lin(s.target_peak),
    ))
}

/// Convert a dBFS value to a linear amplitude (the form `acompressor` /
/// `alimiter` expect for their threshold / limit options). Shared with the Phase
/// 6c spoken-word stages ([`crate::player::spoken`]).
pub(crate) fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Format a linear amplitude for the filter string: rounded to 0.0001 and
/// trimmed of float noise, so the chain string is stable and comparable. Shared
/// with the Phase 6c spoken-word stages ([`crate::player::spoken`]).
pub(crate) fn fmt_lin(x: f64) -> String {
    let rounded = (x * 10_000.0).round() / 10_000.0;
    format!("{rounded}")
}

/// Format a plain numeric parameter (ratio, attack/release ms), reusing the EQ's
/// minimal dB formatting (round to 0.01, trim) so `3.0` renders `3`.
fn fmt_num(x: f64) -> String {
    fmt_db(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(enabled: bool) -> ModuleState<CompSettings> {
        ModuleState {
            enabled,
            settings: CompSettings::default(),
        }
    }

    #[test]
    fn disabled_modules_contribute_no_stage() {
        assert_eq!(comp(false), comp(false));
        assert!(comp_stage(&comp(false)).is_none());
        assert!(limiter_stage(&ModuleState::default()).is_none());
        assert!(leveler_stage(&ModuleState::default()).is_none());
    }

    #[test]
    fn compressor_stage_renders_with_linear_threshold() {
        let stage = comp_stage(&comp(true)).expect("enabled comp has a stage");
        assert!(stage.starts_with("@comp:lavfi=[acompressor="));
        // -18 dB → 0.1259 linear (rounded to 4 dp), ratio 3 renders minimally.
        assert!(stage.contains("threshold=0.1259"), "{stage}");
        assert!(stage.contains("ratio=3"), "{stage}");
        assert!(stage.contains("attack=20"));
        assert!(stage.contains("release=250"));
    }

    #[test]
    fn limiter_stage_is_a_disabled_level_brickwall() {
        let m = ModuleState {
            enabled: true,
            settings: LimiterSettings { ceiling_db: -1.0 },
        };
        let stage = limiter_stage(&m).expect("enabled limiter has a stage");
        // -1 dB → ~0.8913 linear; level=disabled keeps it a pure peak catcher.
        assert!(stage.starts_with("@limit:lavfi=[alimiter=limit="));
        assert!(stage.contains("limit=0.8913"), "{stage}");
        assert!(stage.contains("level=disabled"));
    }

    #[test]
    fn leveler_stage_renders_gauss_and_peak() {
        let m = ModuleState {
            enabled: true,
            settings: LevelerSettings::default(),
        };
        let stage = leveler_stage(&m).expect("enabled leveler has a stage");
        assert_eq!(stage, "@boost:lavfi=[dynaudnorm=g=31:p=0.95]");
    }

    #[test]
    fn db_to_linear_is_correct_at_reference_points() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-9);
        assert!((db_to_linear(-6.0) - 0.5011872).abs() < 1e-6);
    }
}
