//! The "Sound" preferences surface (Phase 5.5b-ii): the app's first
//! `adw::PreferencesDialog`. Hosts the 10-band graphic equalizer — a row of
//! vertical sliders + a preset picker — that drives the player live (gap-free
//! `af-command` per band) and persists through the single-writer worker. The
//! dialog is built in `window.rs` (so its handlers capture the window); this
//! module carries the reusable widget builder and the pure preset-match logic.

use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{EQ_BAND_COUNT, EqPreset};

/// The "no named preset matches" label shown in the preset picker.
pub const CUSTOM_LABEL: &str = "Custom";

/// The slider gain range, dB (the UI bound; the engine clamps wider). Symmetric
/// so 0 dB sits at the centre detent.
pub const SLIDER_RANGE_DB: f64 = 12.0;

/// Build one EQ band slider: a tall vertical `gtk::Scale` over `[-RANGE, +RANGE]`
/// dB, with a detent mark at 0, inverted so up = boost (the hardware-fader
/// convention). `value` is the band's current gain.
pub fn eq_slider(value: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(
        gtk::Orientation::Vertical,
        -SLIDER_RANGE_DB,
        SLIDER_RANGE_DB,
        0.5,
    );
    scale.set_inverted(true); // top = positive gain
    scale.set_draw_value(false);
    scale.set_height_request(150);
    scale.set_vexpand(true);
    scale.add_mark(0.0, gtk::PositionType::Right, None);
    scale.set_value(value);
    scale
}

/// The name of the preset whose bands match `bands` (within a small epsilon), or
/// `None` (a custom edit) when none does. Drives the preset picker's selection.
/// Pure.
pub fn match_preset(bands: &[f64; EQ_BAND_COUNT], presets: &[EqPreset]) -> Option<String> {
    presets
        .iter()
        .find(|p| {
            p.bands
                .iter()
                .zip(bands.iter())
                .all(|(a, b)| (a - b).abs() < 0.05)
        })
        .map(|p| p.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, bands: [f64; EQ_BAND_COUNT]) -> EqPreset {
        EqPreset {
            name: name.to_string(),
            bands,
        }
    }

    #[test]
    fn match_preset_finds_an_exact_match() {
        let presets = vec![
            preset("Flat", [0.0; EQ_BAND_COUNT]),
            preset("Bass", [6.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        ];
        assert_eq!(
            match_preset(&[0.0; EQ_BAND_COUNT], &presets).as_deref(),
            Some("Flat")
        );
        assert_eq!(
            match_preset(
                &[6.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                &presets
            )
            .as_deref(),
            Some("Bass")
        );
    }

    #[test]
    fn match_preset_returns_none_for_a_custom_edit() {
        let presets = vec![preset("Flat", [0.0; EQ_BAND_COUNT])];
        let mut bands = [0.0; EQ_BAND_COUNT];
        bands[3] = 3.0;
        assert_eq!(match_preset(&bands, &presets), None);
    }
}
