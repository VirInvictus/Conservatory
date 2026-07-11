//! The "Sound" preferences surface (Phase 5.5b-ii; plain GTK since Phase 26i).
//! Hosts the 10-band graphic equalizer — a row of vertical sliders + a preset
//! picker — that drives the player live (gap-free `af-command` per band) and
//! persists through the single-writer worker. The page is built in `window.rs`
//! (so its handlers capture the window); this module carries the reusable
//! widget builder and the pure preset-match logic.

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

/// The ReplayGain-mode picker options (Phase 5.5c-ii): the display label paired
/// with the stored `audio_state.replaygain_mode` value.
pub const RG_MODES: [(&str, &str); 3] = [("Off", "off"), ("Track", "track"), ("Album", "album")];

/// The output-backend picker options: the display label paired with the stored
/// `audio_state.output_backend` / mpv `ao` value.
pub const BACKENDS: [(&str, &str); 5] = [
    ("Automatic", "auto"),
    ("PipeWire", "pipewire"),
    ("PulseAudio", "pulse"),
    ("ALSA", "alsa"),
    ("JACK", "jack"),
];

/// The resampler-quality picker options: the display label paired with the stored
/// `audio_state.resampler_quality` value (mirrors `ResamplerQuality::as_str`).
pub const RESAMPLERS: [(&str, &str); 2] = [("Default", "default"), ("High quality", "high")];

/// The Smart Speed aggressiveness picker: the display label paired with the stored
/// `audio_state.smart_speed_level` token (mirrors `SmartSpeedLevel::as_str`).
pub const SMART_SPEED_LEVELS: [(&str, &str); 3] = [
    ("Gentle", "gentle"),
    ("Balanced", "balanced"),
    ("Aggressive", "aggressive"),
];

/// The display labels of an option table, for a `gtk::StringList` model.
pub fn option_labels<'a>(table: &[(&'a str, &'a str)]) -> Vec<&'a str> {
    table.iter().map(|(label, _)| *label).collect()
}

/// The combo-row index whose stored value is `value`, or 0 (the first option) when
/// none matches (forgiving, the `get_audio_state` stance). Pure.
pub fn option_index(table: &[(&str, &str)], value: &str) -> u32 {
    table.iter().position(|(_, v)| *v == value).unwrap_or(0) as u32
}

/// The stored value at a combo-row `index`, or the first option's when the index
/// is out of range. Pure.
pub fn option_value<'a>(table: &[(&'a str, &'a str)], index: u32) -> &'a str {
    table.get(index as usize).map_or(table[0].1, |(_, v)| *v)
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

    #[test]
    fn option_index_finds_value_and_defaults_to_zero() {
        assert_eq!(option_index(&RG_MODES, "track"), 1);
        assert_eq!(option_index(&BACKENDS, "alsa"), 3);
        assert_eq!(option_index(&RESAMPLERS, "high"), 1);
        // An unknown stored value degrades to the first option.
        assert_eq!(option_index(&BACKENDS, "nonsense"), 0);
    }

    #[test]
    fn option_value_round_trips_and_clamps() {
        for table in [&RG_MODES[..], &BACKENDS[..], &RESAMPLERS[..]] {
            for (i, (_, v)) in table.iter().enumerate() {
                assert_eq!(option_value(table, i as u32), *v);
                assert_eq!(option_index(table, v), i as u32);
            }
            // Out of range → the first option's value.
            assert_eq!(option_value(table, 99), table[0].1);
        }
    }

    #[test]
    fn option_labels_lists_every_label() {
        assert_eq!(option_labels(&RG_MODES), vec!["Off", "Track", "Album"]);
        assert_eq!(option_labels(&RESAMPLERS), vec!["Default", "High quality"]);
    }
}
