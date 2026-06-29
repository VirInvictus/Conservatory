//! The spectrum visualizer widget (Phase 12d): a `gtk::DrawingArea` of
//! accent-coloured frequency bars in the Now Playing drawer. It is driven by a
//! **frame-clock tick** (display rate), decoupled from the engine's ~10fps
//! snapshot, reading raw band levels from the PipeWire [`SpectrumTap`] and
//! smoothing them (fast attack / slow decay) per frame.
//!
//! The capture is started when the widget maps (the drawer opens + the area is
//! shown) and stopped when it unmaps, so it costs nothing while the drawer is
//! closed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::player::spectrum::SpectrumSmoother;

use crate::viz::{SpectrumTap, N_BANDS};

/// The visualizer's fixed height (the bars draw bottom-up within it).
const HEIGHT: i32 = 72;
/// Kanagawa dragonRed (`#c4746e`), the default bar colour when no album accent.
const DEFAULT_ACCENT: (f64, f64, f64) = (
    0xc4 as f64 / 255.0,
    0x74 as f64 / 255.0,
    0x6e as f64 / 255.0,
);

struct SpectrumState {
    tap: Option<SpectrumTap>,
    smoother: SpectrumSmoother,
    levels: Vec<f32>,
    accent: Option<u32>,
}

/// The visualizer: the `DrawingArea` to mount, plus the shared state the window
/// pokes the accent into.
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
}

pub fn build_spectrum() -> Spectrum {
    let area = gtk::DrawingArea::builder()
        .content_height(HEIGHT)
        .hexpand(true)
        .css_classes(["spectrum"])
        .build();

    let state = Rc::new(RefCell::new(SpectrumState {
        tap: None,
        // Per-frame blend: a brisk attack so peaks pop, a slow decay so bars fall.
        smoother: SpectrumSmoother::new(N_BANDS, 0.5, 0.12),
        levels: vec![0.0; N_BANDS],
        accent: None,
    }));

    area.set_draw_func({
        let state = state.clone();
        move |_, cr, width, height| {
            let st = state.borrow();
            draw_bars(cr, width, height, &st.levels, st.accent);
        }
    });

    // Capture only while the drawer is open: the Revealer unmaps the area when it
    // closes, which stops the PipeWire stream and the per-frame work.
    area.connect_map({
        let state = state.clone();
        move |_| {
            state.borrow_mut().tap = Some(SpectrumTap::start());
        }
    });
    area.connect_unmap({
        let state = state.clone();
        move |_| {
            if let Some(tap) = state.borrow_mut().tap.take() {
                tap.stop();
            }
        }
    });

    // Frame-clock tick: pull the latest raw bands, smooth, and redraw at display
    // rate (independent of the engine snapshot).
    area.add_tick_callback({
        let state = state.clone();
        move |area, _clock| {
            let mut st = state.borrow_mut();
            if let Some(raw) = st.tap.as_ref().map(|t| t.bands()) {
                let smoothed = st.smoother.update(&raw).to_vec();
                st.levels = smoothed;
                drop(st);
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        }
    });

    Spectrum { area, state }
}

/// Draw the bars bottom-up, brightening with level so loud bands read hot.
fn draw_bars(
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    levels: &[f32],
    accent: Option<u32>,
) {
    let n = levels.len();
    if n == 0 || width <= 0 || height <= 0 {
        return;
    }
    let (r, g, b) = accent.map(unpack_rgb).unwrap_or(DEFAULT_ACCENT);
    let w = width as f64;
    let h = height as f64;
    let gap = 2.0;
    let bar_w = ((w - gap * (n as f64 - 1.0)) / n as f64).max(1.0);

    for (i, &level) in levels.iter().enumerate() {
        let level = (level as f64).clamp(0.0, 1.0);
        let bar_h = (level * h).max(1.0);
        let x = i as f64 * (bar_w + gap);
        // Alpha rides the level so quiet bars recede and peaks read solid.
        cr.set_source_rgba(r, g, b, 0.30 + 0.70 * level);
        cr.rectangle(x, h - bar_h, bar_w, bar_h);
        let _ = cr.fill();
    }
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
