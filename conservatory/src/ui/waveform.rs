//! The waveform seek widget (Phase 19a-ii): a `gtk::DrawingArea` that replaces
//! the Now-bar seek `Scale`. It draws the track's loudness envelope (computed +
//! cached headless by `conservatory-core::waveform`, Phase 19a-i) mirrored about
//! a centre line, with the played portion in the album accent and the rest
//! dimmed; a drag (or a tap) seeks. The same widget is reused at a larger size
//! in the Now Playing drawer and, later, full-screen (19c).
//!
//! Like the old `Scale`, it is a *sampled* display: the window's 250 ms poll
//! calls [`WaveformSeek::set_position`] (which only repaints, never seeks), while
//! a user drag calls the registered seek callback. The two paths never cross, so
//! the poll's programmatic position can't loop back into a seek (the `Scale`'s
//! `change-value` vs `set_value` guard, kept structurally).
//!
//! The envelope loads off the GTK thread (an ffmpeg decode is far too slow for
//! the main loop): the window kicks a `spawn_blocking` decode and hands the
//! result back through [`WaveformSeek::apply_envelope`], stamped with a
//! generation so a decode that finishes after the track already changed is
//! dropped. Until it lands (or for a remote / undecodable item) the widget falls
//! back to a flat seek line, so it always works as a plain seek bar.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

/// The compact Now-bar height; it expands past this via `vexpand` when reused in
/// the larger surfaces.
const HEIGHT: i32 = 34;
/// The Now-bar width request, matching the old seek `Scale`.
const WIDTH: i32 = 220;
/// Kanagawa dragonRed (`#c4746e`), the bar colour when there is no album accent.
const DEFAULT_ACCENT: (f64, f64, f64) = (
    0xc4 as f64 / 255.0,
    0x74 as f64 / 255.0,
    0x6e as f64 / 255.0,
);

struct WaveformState {
    /// The normalized peak envelope (0..=1), empty when none is loaded yet: the
    /// widget then draws a flat seek line instead of a waveform.
    peak: Vec<f32>,
    position: f64,
    duration: Option<f64>,
    accent: Option<u32>,
    /// Whether seeking is possible (a known duration); a drag no-ops otherwise.
    sensitive: bool,
    /// Bumped on every load so a stale decode result is dropped (`apply_envelope`
    /// only applies when its generation still matches).
    generation: u64,
    /// The window's seek handler; called with a target position in seconds.
    on_seek: Option<Box<dyn Fn(f64)>>,
}

/// The waveform seek bar: the `DrawingArea` to mount plus the shared state the
/// window pokes position / accent / envelope into.
#[derive(Clone)]
pub struct WaveformSeek {
    pub area: gtk::DrawingArea,
    state: Rc<RefCell<WaveformState>>,
}

impl WaveformSeek {
    /// Update the playhead from the poll snapshot. Only repaints; never seeks.
    pub fn set_position(&self, position: f64, duration: Option<f64>) {
        {
            let mut st = self.state.borrow_mut();
            st.position = position;
            st.duration = duration;
        }
        self.area.queue_draw();
    }

    /// Enable or disable seeking (a known, positive duration).
    pub fn set_sensitive(&self, sensitive: bool) {
        self.state.borrow_mut().sensitive = sensitive;
        self.area.set_sensitive(sensitive);
    }

    /// Tint the played portion with the item's accent (`None` = Dragon red).
    pub fn set_accent(&self, accent: Option<u32>) {
        self.state.borrow_mut().accent = accent;
        self.area.queue_draw();
    }

    /// Begin loading a new envelope: clear the old one to the flat fallback and
    /// return the generation the eventual [`apply_envelope`](Self::apply_envelope)
    /// must carry to be accepted.
    pub fn begin_load(&self) -> u64 {
        let mut st = self.state.borrow_mut();
        st.peak.clear();
        st.generation = st.generation.wrapping_add(1);
        let generation = st.generation;
        drop(st);
        self.area.queue_draw();
        generation
    }

    /// Apply a decoded envelope if it is still current (its `generation` matches
    /// the latest [`begin_load`](Self::begin_load)); a late result is dropped.
    pub fn apply_envelope(&self, generation: u64, peak: Option<Vec<f32>>) {
        let mut st = self.state.borrow_mut();
        if st.generation != generation {
            return;
        }
        st.peak = peak.unwrap_or_default();
        drop(st);
        self.area.queue_draw();
    }

    /// Reset to the idle state (nothing playing).
    pub fn clear(&self) {
        let mut st = self.state.borrow_mut();
        st.peak.clear();
        st.position = 0.0;
        st.duration = None;
        st.sensitive = false;
        st.generation = st.generation.wrapping_add(1);
        drop(st);
        self.area.set_sensitive(false);
        self.area.queue_draw();
    }

    /// Register the seek handler, called with a target position in seconds when
    /// the user taps or drags on the bar.
    pub fn connect_seek<F: Fn(f64) + 'static>(&self, f: F) {
        self.state.borrow_mut().on_seek = Some(Box::new(f));
    }
}

/// The played fraction (0..=1) for a pointer at `x` over a bar of `width`, used
/// by both the draw split and the seek target. Pure.
fn fraction_at(x: f64, width: f64) -> f64 {
    if width <= 0.0 {
        0.0
    } else {
        (x / width).clamp(0.0, 1.0)
    }
}

pub fn build_waveform() -> WaveformSeek {
    let area = gtk::DrawingArea::builder()
        .content_height(HEIGHT)
        .width_request(WIDTH)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["waveform-seek"])
        .build();
    area.set_sensitive(false);

    let state = Rc::new(RefCell::new(WaveformState {
        peak: Vec::new(),
        position: 0.0,
        duration: None,
        accent: None,
        sensitive: false,
        generation: 0,
        on_seek: None,
    }));

    area.set_draw_func({
        let state = state.clone();
        move |_, cr, width, height| {
            let st = state.borrow();
            let frac = match st.duration {
                Some(d) if d > 0.0 => (st.position / d).clamp(0.0, 1.0),
                _ => 0.0,
            };
            draw_waveform(cr, width, height, &st.peak, frac, st.accent, st.sensitive);
        }
    });

    // A drag doubles as a tap (a press fires drag-begin with no movement), so one
    // gesture covers both click-to-seek and scrub. The poll's `set_position`
    // never runs through here, so it can't loop back into a seek.
    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin({
        let state = state.clone();
        let area = area.clone();
        move |_, start_x, _| seek_to(&state, start_x, area.width())
    });
    drag.connect_drag_update({
        let state = state.clone();
        let area = area.clone();
        move |g, off_x, _| {
            let start_x = g.start_point().map(|(sx, _)| sx).unwrap_or(0.0);
            seek_to(&state, start_x + off_x, area.width())
        }
    });
    area.add_controller(drag);

    WaveformSeek { area, state }
}

/// Fire the seek callback for a pointer at `x` (widget-local) if seeking is on.
fn seek_to(state: &Rc<RefCell<WaveformState>>, x: f64, width: i32) {
    let st = state.borrow();
    if !st.sensitive {
        return;
    }
    if let (Some(dur), Some(cb)) = (st.duration, st.on_seek.as_ref()) {
        cb(fraction_at(x, width as f64) * dur);
    }
}

/// Draw the envelope mirrored about a centre line: played columns in the accent,
/// the rest dimmed. With no envelope, draw a flat seek line so the bar still
/// reads (and works) as a plain scrubber.
fn draw_waveform(
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    peak: &[f32],
    frac: f64,
    accent: Option<u32>,
    sensitive: bool,
) {
    if width <= 0 || height <= 0 {
        return;
    }
    let (r, g, b) = accent.map(unpack_rgb).unwrap_or(DEFAULT_ACCENT);
    let played_a = if sensitive { 1.0 } else { 0.55 };
    let unplayed_a = 0.30;
    let w = width as f64;
    let h = height as f64;
    let mid = h / 2.0;
    let split = (frac * w).clamp(0.0, w);

    let set = |cr: &gtk::cairo::Context, x: f64| {
        if x < split {
            cr.set_source_rgba(r, g, b, played_a);
        } else {
            cr.set_source_rgba(r, g, b, unplayed_a);
        }
    };

    if peak.is_empty() {
        // Flat fallback: a 2px centre line, split played / unplayed.
        let y = mid - 1.0;
        cr.rectangle(0.0, y, split, 2.0);
        cr.set_source_rgba(r, g, b, played_a);
        let _ = cr.fill();
        cr.rectangle(split, y, w - split, 2.0);
        cr.set_source_rgba(r, g, b, unplayed_a);
        let _ = cr.fill();
        return;
    }

    // One filled column per pixel: average the envelope buckets that fall in the
    // column so any bucket count downsamples cleanly to the current width.
    let cols = width as usize;
    let n = peak.len();
    let max_half = mid * 0.9;
    for col in 0..cols {
        let lo = col * n / cols;
        let hi = ((col + 1) * n / cols).max(lo + 1).min(n);
        let v = peak[lo..hi].iter().copied().sum::<f32>() / (hi - lo) as f32;
        let half = (v as f64 * max_half).max(0.75);
        let x = col as f64;
        set(cr, x + 0.5);
        cr.rectangle(x, mid - half, 1.0, half * 2.0);
        let _ = cr.fill();
    }
}

/// Unpack a packed `0x00RRGGBB` accent into Cairo's 0..=1 RGB (the spectrum
/// widget's helper).
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
    fn fraction_clamps_to_unit() {
        assert_eq!(fraction_at(0.0, 200.0), 0.0);
        assert_eq!(fraction_at(100.0, 200.0), 0.5);
        assert_eq!(fraction_at(250.0, 200.0), 1.0); // past the end clamps
        assert_eq!(fraction_at(-5.0, 200.0), 0.0); // before the start clamps
        assert_eq!(fraction_at(50.0, 0.0), 0.0); // zero width is safe
    }

    #[test]
    fn unpack_splits_channels() {
        assert_eq!(unpack_rgb(0x00ff_0000), (1.0, 0.0, 0.0));
        assert_eq!(unpack_rgb(0x0000_00ff), (0.0, 0.0, 1.0));
    }
}
