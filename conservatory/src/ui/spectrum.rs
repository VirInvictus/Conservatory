//! The spectrum visualizer widget (Phase 12d): a `gtk::DrawingArea` of
//! accent-coloured frequency bars in the Now Playing drawer. It is driven by a
//! **frame-clock tick** (display rate), decoupled from the engine's ~10fps
//! snapshot, reading raw band levels from the PipeWire [`SpectrumTap`] and
//! smoothing them (fast attack / slow decay) per frame.
//!
//! The capture is started when the widget maps (the drawer opens + the area is
//! shown) and stopped when it unmaps, so it costs nothing while the drawer is
//! closed.
//!
//! Rendering downsamples the 192 capture bands into a smaller set of wide
//! gradient bars (deep accent at the base brightening to a hot top), each with a
//! slow-falling peak-hold cap and a faint mirrored reflection: the "sexier"
//! analyzer of the full-bleed Now Playing rebuild.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::player::spectrum::SpectrumSmoother;

use crate::viz::{SpectrumTap, N_BANDS};

/// The visualizer's natural height; it expands past this via `vexpand` to fill
/// the full-bleed drawer, but keeps a sane floor.
const HEIGHT: i32 = 140;
/// Per-frame fall of the peak-hold caps (fraction of full height per tick).
const PEAK_FALL: f32 = 0.006;
/// Kanagawa dragonRed (`#c4746e`), the default bar colour when no album accent.
const DEFAULT_ACCENT: (f64, f64, f64) = (
    0xc4 as f64 / 255.0,
    0x74 as f64 / 255.0,
    0x6e as f64 / 255.0,
);

struct SpectrumState {
    tap: Option<SpectrumTap>,
    /// The drawer is open (the area is mapped): one half of the capture gate.
    mapped: bool,
    /// Conservatory is actively playing: the other half. The tap targets our mpv
    /// node, which only exists while playing; running it otherwise would fall back
    /// to the microphone (see `viz.rs`), so both must hold for the tap to live.
    playing: bool,
    smoother: SpectrumSmoother,
    /// The smoothed bar levels (0..=1), one per band.
    display: Vec<f32>,
    /// The slow-falling peak-hold value per band (0..=1).
    peaks: Vec<f32>,
    accent: Option<u32>,
}

impl SpectrumState {
    /// Start the tap when (and only when) the drawer is open *and* playback is on;
    /// stop it otherwise. Idempotent, so map/unmap and `set_playing` can all call
    /// it freely.
    fn refresh_tap(&mut self) {
        let want = self.mapped && self.playing;
        if want && self.tap.is_none() {
            self.tap = Some(SpectrumTap::start());
        } else if !want && let Some(tap) = self.tap.take() {
            tap.stop();
        }
    }
}

/// The visualizer: the `DrawingArea` to mount, plus the shared state the window
/// pokes the accent + play-state into.
pub struct Spectrum {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<SpectrumState>>,
}

impl Spectrum {
    /// Tint the bars with the playing item's accent (`None` falls back to the
    /// Dragon red). The bars repaint on the next frame tick.
    pub fn set_accent(&self, accent: Option<u32>) {
        self.state.borrow_mut().accent = accent;
    }

    /// Tell the visualizer whether Conservatory is actively playing; the tap runs
    /// only while it is (and the drawer is open). When it stops, the bars decay to
    /// rest rather than freezing.
    pub fn set_playing(&self, playing: bool) {
        let mut st = self.state.borrow_mut();
        if st.playing != playing {
            st.playing = playing;
            st.refresh_tap();
        }
    }
}

pub fn build_spectrum() -> Spectrum {
    let area = gtk::DrawingArea::builder()
        .content_height(HEIGHT)
        .hexpand(true)
        .vexpand(true)
        .css_classes(["spectrum"])
        .build();

    let state = Rc::new(RefCell::new(SpectrumState {
        tap: None,
        mapped: false,
        playing: false,
        // Per-frame blend: a brisk attack so peaks pop, a slow decay so bars fall.
        smoother: SpectrumSmoother::new(N_BANDS, 0.5, 0.12),
        display: vec![0.0; N_BANDS],
        peaks: vec![0.0; N_BANDS],
        accent: None,
    }));

    area.set_draw_func({
        let state = state.clone();
        move |_, cr, width, height| {
            let st = state.borrow();
            draw_bars(cr, width, height, &st.display, &st.peaks, st.accent);
        }
    });

    // The drawer opening / closing maps / unmaps the area; combined with the
    // play-state (`set_playing`) this gates the capture.
    area.connect_map({
        let state = state.clone();
        move |_| {
            let mut st = state.borrow_mut();
            st.mapped = true;
            st.refresh_tap();
        }
    });
    area.connect_unmap({
        let state = state.clone();
        move |_| {
            let mut st = state.borrow_mut();
            st.mapped = false;
            st.refresh_tap();
        }
    });

    // Frame-clock tick: smooth the latest bands (or zeros when not capturing, so
    // the bars decay to rest), age the peak caps, and redraw at display rate.
    area.add_tick_callback({
        let state = state.clone();
        let zeros = vec![0.0_f32; N_BANDS];
        move |area, _clock| {
            let mut st = state.borrow_mut();
            // One bar per captured band (the dense foobar-style field). With no tap
            // the target is zeros, so a pause / close lets the bars fall to rest.
            let raw = st.tap.as_ref().map(|t| t.bands());
            let target = raw.as_deref().unwrap_or(&zeros);
            let smoothed = st.smoother.update(target).to_vec();
            for (i, &level) in smoothed.iter().enumerate() {
                let fallen = (st.peaks[i] - PEAK_FALL).max(0.0);
                st.peaks[i] = fallen.max(level);
            }
            st.display = smoothed;
            drop(st);
            area.queue_draw();
            glib::ControlFlow::Continue
        }
    });

    Spectrum { area, state }
}

/// Draw the spectrum as a dense field of thin gradient bars, each topped with a
/// slow-falling peak-hold cap and trailed by a faint mirrored reflection.
fn draw_bars(
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    levels: &[f32],
    peaks: &[f32],
    accent: Option<u32>,
) {
    let n = levels.len();
    if n == 0 || width <= 0 || height <= 0 {
        return;
    }
    let (r, g, b) = accent.map(unpack_rgb).unwrap_or(DEFAULT_ACCENT);
    // The hot top: the accent lifted toward a warm white so peaks read incandescent.
    let (hr, hg, hb) = (
        r + (1.0 - r) * 0.78,
        g + (0.96 - g) * 0.78,
        b + (0.82 - b) * 0.78,
    );
    let w = width as f64;
    let h = height as f64;

    // The bars occupy the top ~78% of the area; the bottom band is left for the
    // mirrored reflection so the analyzer reads as sitting on a surface.
    let baseline = h * 0.80;
    let bar_zone = baseline;
    let slot = w / n as f64;
    let bar_w = (slot * 0.62).max(1.0);
    let radius = (bar_w * 0.5).min(4.0);

    for (i, &level) in levels.iter().enumerate() {
        let level = (level as f64).clamp(0.0, 1.0);
        let x = i as f64 * slot + (slot - bar_w) / 2.0;
        let bar_h = (level * bar_zone).max(1.0);
        let top = baseline - bar_h;

        // The bar: a vertical gradient from a deep accent base to the hot top, the
        // top corners rounded so it reads as a soft column rather than a hard block.
        let grad = gtk::cairo::LinearGradient::new(0.0, baseline, 0.0, top);
        grad.add_color_stop_rgba(0.0, r * 0.45, g * 0.45, b * 0.45, 0.55);
        grad.add_color_stop_rgba(0.65, r, g, b, 0.92);
        grad.add_color_stop_rgba(1.0, hr, hg, hb, 1.0);
        rounded_top_rect(cr, x, top, bar_w, bar_h, radius.min(bar_h));
        let _ = cr.set_source(&grad);
        let _ = cr.fill();

        // The reflection: the same column mirrored under the baseline, fading out.
        let refl_h = (bar_h * 0.45).min(h - baseline);
        if refl_h > 1.0 {
            let refl = gtk::cairo::LinearGradient::new(0.0, baseline, 0.0, baseline + refl_h);
            refl.add_color_stop_rgba(0.0, r, g, b, 0.20);
            refl.add_color_stop_rgba(1.0, r, g, b, 0.0);
            cr.rectangle(x, baseline, bar_w, refl_h);
            let _ = cr.set_source(&refl);
            let _ = cr.fill();
        }

        // The peak-hold cap: a thin hot bar hovering at the recent maximum.
        let peak = (peaks.get(i).copied().unwrap_or(0.0) as f64).clamp(0.0, 1.0);
        let peak_y = baseline - (peak * bar_zone).max(1.0);
        cr.rectangle(x, peak_y - 1.5, bar_w, 2.0);
        cr.set_source_rgba(hr, hg, hb, 0.85);
        let _ = cr.fill();
    }
}

/// Trace a rectangle with rounded top corners (square bottom, since it sits on the
/// baseline). `h` is the bar height above the baseline.
fn rounded_top_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::PI;
    let r = r.min(w / 2.0).min(h).max(0.0);
    let bottom = y + h;
    cr.new_sub_path();
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.arc(x + w - r, y + r, r, 1.5 * PI, 2.0 * PI);
    cr.line_to(x + w, bottom);
    cr.line_to(x, bottom);
    cr.close_path();
}

/// Unpack a packed `0x00RRGGBB` accent into Cairo's 0..=1 RGB.
fn unpack_rgb(rgb: u32) -> (f64, f64, f64) {
    let r = ((rgb >> 16) & 0xff) as f64 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f64 / 255.0;
    let b = (rgb & 0xff) as f64 / 255.0;
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_splits_channels() {
        assert_eq!(unpack_rgb(0x00ff_0000), (1.0, 0.0, 0.0));
        assert_eq!(unpack_rgb(0x0000_ff00), (0.0, 1.0, 0.0));
        let (r, g, b) = unpack_rgb(0x00c4_746e);
        assert!((r - 0xc4 as f64 / 255.0).abs() < 1e-9);
        assert!((g - 0x74 as f64 / 255.0).abs() < 1e-9);
        assert!((b - 0x6e as f64 / 255.0).abs() < 1e-9);
    }
}
