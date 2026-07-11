//! Per-item accent tinting (Phase 12a): one shared, swappable display-wide CSS
//! provider so every cover frame in the app (the browse cover column and panel,
//! the Now-bar, the inspector, the Now Playing drawer) gets the album's accent
//! ring through a single technique instead of the copy-pasted per-module
//! versions. GTK4 deprecated per-widget `StyleContext` providers, so the
//! non-deprecated route is a display-wide provider keyed by a unique CSS class.

use std::cell::RefCell;

use gtk::prelude::*;
use gtk4 as gtk;

/// The drop shadow every lifted cover carries (kept in step with `main.rs`'s
/// `.cover-art`); the accent ring rule re-states it so the 2px ring and the lift
/// coexist in one `box-shadow` (a later rule replaces the property wholesale).
const COVER_DROP: &str = "0 2px 10px rgba(0,0,0,0.35)";

/// The unique CSS class for an accent colour, e.g. `cover-acc-c4746e`.
pub fn accent_class(accent: u32) -> String {
    format!("cover-acc-{:06x}", accent & 0x00ff_ffff)
}

/// The CSS rule tinting a cover frame with a 2px accent ring over the drop shadow.
pub fn cover_ring_css(accent: u32) -> String {
    let hex = accent & 0x00ff_ffff;
    format!(".cover-acc-{hex:06x} {{ box-shadow: 0 0 0 2px #{hex:06x}, {COVER_DROP}; }}")
}

/// Owns one display-wide `CssProvider`, swapped on each `apply`. Hold one per
/// call site (the widget whose accent changes over its lifetime).
#[derive(Default)]
pub struct AccentProvider {
    provider: RefCell<Option<gtk::CssProvider>>,
}

impl AccentProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the held provider with one serving `css` (an empty string just
    /// clears the previous rule). The old provider is removed from the display
    /// first, so rules never accumulate.
    pub fn set_css(&self, css: &str) {
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        if let Some(old) = self.provider.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        if css.is_empty() {
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        // USER + 2: one step above the owned base sheet (USER + 1, theme.rs),
        // preserving the pre-26l layering where the runtime ring outranked it.
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 2,
        );
        *self.provider.borrow_mut() = Some(provider);
    }

    /// Tint `frame` with the album's 2px accent ring (the cover-frame idiom).
    /// `base` are the always-present classes (e.g. `["cover-art"]`); `accent`
    /// `None` clears back to just `base`.
    pub fn apply_cover_ring(
        &self,
        frame: &impl IsA<gtk::Widget>,
        base: &[&str],
        accent: Option<u32>,
    ) {
        match accent {
            Some(rgb) => {
                self.set_css(&cover_ring_css(rgb));
                let cls = accent_class(rgb);
                let mut classes: Vec<&str> = base.to_vec();
                classes.push(cls.as_str());
                frame.set_css_classes(&classes);
            }
            None => {
                self.set_css("");
                frame.set_css_classes(base);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_class_is_six_hex_digits() {
        assert_eq!(accent_class(0x00c4_746e), "cover-acc-c4746e");
        // The high byte is ignored (the colour is packed 0x00RRGGBB).
        assert_eq!(accent_class(0xffc4_746e), "cover-acc-c4746e");
        assert_eq!(accent_class(0x0000_0000), "cover-acc-000000");
    }

    #[test]
    fn ring_css_layers_ring_over_drop() {
        let css = cover_ring_css(0x00c4_746e);
        assert!(css.contains(".cover-acc-c4746e"));
        assert!(css.contains("0 0 0 2px #c4746e"));
        // The drop shadow is preserved so the lift survives the accent.
        assert!(css.contains("0 2px 10px"));
    }
}
