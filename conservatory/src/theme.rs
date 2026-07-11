//! The owned application stylesheet (Phase 26l): Kanagawa Dragon baked
//! directly into one generated sheet, replacing the libadwaita stylesheet and
//! the old `@define-color` overrides. The look is the locked spec §2.4 design
//! language: flat, square, hard 1px borders, denser spacing than the GNOME
//! HIG. Colophon's Phase 6 sheet is the template (ATTRIBUTIONS.md); custom
//! properties are deliberately not used (one fixed palette, and skipping them
//! keeps the gtk4 crate on `v4_14`), so the hexes are spliced by token
//! replacement instead.
//!
//! Deliberate exceptions to "flat": the lifted album/book cover cards keep
//! their radius and drop shadow (the Hermitage cover-as-visual-unit pattern;
//! the runtime accent ring in `ui/accent.rs` layers onto that same shadow).
//! Chrome is flat; content imagery stays lifted.
//!
//! Typography carries over from Phase 13d: exactly three `font-family` rules
//! (Inter body, Fraunces headers, IBM Plex Mono technical), enforced by a
//! unit test, fonts bundled via fontconfig in `main.rs`.

use gtk4 as gtk;

// The Dragon roles (the raw palette lives in docs/theme.md).
pub const BG_WINDOW: &str = "#181616"; // dragonBlack3
pub const BG_VIEW: &str = "#12120f"; // dragonBlack1
pub const BG_HEADER: &str = "#1d1c19"; // dragonBlack2
pub const BG_CARD: &str = "#1d1c19"; // dragonBlack2
pub const FG: &str = "#c5c9c5"; // dragonWhite
pub const FG_DIM: &str = "#a6a69c"; // dragonGray
pub const GRID: &str = "#393836"; // dragonBlack5 (hairlines, borders)
pub const ACCENT: &str = "#c4746e"; // dragonRed (waveRed reserved for errors)
pub const ON_ACCENT: &str = "#12120f"; // dragonBlack1
pub const WARN: &str = "#c4b28a"; // dragonYellow
pub const ERR: &str = "#c4746e"; // dragonRed
pub const OK: &str = "#87a987"; // dragonGreen

/// The sheet template. `%TOKENS%` are replaced with the hexes above by
/// [`sheet`]; no other substitution happens, so plain CSS braces are safe.
const TEMPLATE: &str = "\
/* --- Base widgets (the adwaita sheet's replacement) --- */
window { background-color: %BG_WINDOW%; color: %FG%; }
window.csd { border-radius: 0; box-shadow: none; }
decoration { border-radius: 0; box-shadow: none; }
.background { background-color: %BG_WINDOW%; color: %FG%; }

headerbar {
  background-color: %BG_HEADER%;
  background-image: none;
  color: %FG%;
  box-shadow: none;
  border-bottom: 1px solid %GRID%;
  min-height: 34px;
  padding: 0 4px;
}
headerbar button { min-height: 24px; }

paned > separator { background-color: %GRID%; background-image: none; min-width: 1px; min-height: 1px; }

columnview, listview, list { background-color: %BG_VIEW%; color: %FG%; }
columnview > header { background-color: %BG_VIEW%; border-bottom: 1px solid %GRID%; }
row { border-radius: 0; }
row.activatable:hover { background-color: alpha(currentColor, 0.06); }
row:selected { background-color: alpha(%ACCENT%, 0.35); color: %FG%; }
.navigation-sidebar { background-color: %BG_VIEW%; }
.navigation-sidebar > row { padding: 4px 8px; border-radius: 0; }

.card, list.boxed-list {
  background-color: %BG_CARD%;
  color: %FG%;
  border: 1px solid %GRID%;
  border-radius: 0;
  box-shadow: none;
}
list.boxed-list > row { border-bottom: 1px solid %GRID%; }
list.boxed-list > row:last-child { border-bottom: none; }

button {
  background-color: %BG_CARD%;
  background-image: none;
  color: %FG%;
  border: 1px solid %GRID%;
  border-radius: 0;
  box-shadow: none;
  min-height: 24px;
  padding: 2px 10px;
}
button:hover { background-color: %GRID%; }
button:active, button:checked { background-color: %ACCENT%; color: %ON_ACCENT%; border-color: %ACCENT%; }
button.flat, button.circular { background-color: transparent; border-color: transparent; }
button.flat:hover, button.circular:hover { background-color: %GRID%; }
button.suggested-action { background-color: %ACCENT%; color: %ON_ACCENT%; border-color: %ACCENT%; }
button.destructive-action { background-color: %ERR%; color: %ON_ACCENT%; border-color: %ERR%; }
button.circular { border-radius: 0; }
.linked > button:not(:first-child) { border-left-width: 0; }
.toolbar { padding: 4px 6px; }
.osd { background-color: alpha(%BG_WINDOW%, 0.80); color: %FG%; border-radius: 0; }

popover > arrow { background-color: %BG_CARD%; }
popover > contents {
  background-color: %BG_CARD%;
  color: %FG%;
  border: 1px solid %GRID%;
  border-radius: 0;
  box-shadow: none;
  padding: 4px;
}
popover.menu modelbutton { border-radius: 0; padding: 5px 8px; }
modelbutton:hover { background-color: %ACCENT%; color: %ON_ACCENT%; }
popover.menu separator { background-color: %GRID%; min-height: 1px; margin: 4px 0; }

entry, spinbutton {
  background-color: %BG_VIEW%;
  color: %FG%;
  border: 1px solid %GRID%;
  border-radius: 0;
  box-shadow: none;
}
entry:focus-within, spinbutton:focus-within { border-color: %ACCENT%; }
spinbutton > button { border-width: 0; background-color: transparent; }
spinbutton > button:hover { background-color: %GRID%; }
dropdown > button { background-color: %BG_CARD%; }

switch { background-color: %GRID%; border: 1px solid %GRID%; border-radius: 0; }
switch:checked { background-color: %ACCENT%; border-color: %ACCENT%; }
switch > slider { background-color: %FG%; border: 1px solid %GRID%; border-radius: 0; min-width: 18px; min-height: 18px; }

check { background-color: %BG_VIEW%; border: 1px solid %GRID%; border-radius: 0; }
check:checked { background-color: %ACCENT%; color: %ON_ACCENT%; border-color: %ACCENT%; }

scale > trough { background-color: %GRID%; border-radius: 0; }
scale > trough > highlight { background-color: %ACCENT%; border-radius: 0; }
scale > trough > slider { background-color: %FG%; border: 1px solid %GRID%; border-radius: 0; box-shadow: none; }
scale > marks-after, scale > marks-before { color: %FG_DIM%; }

tooltip, tooltip.background {
  background-color: %BG_HEADER%;
  color: %FG%;
  border: 1px solid %GRID%;
  border-radius: 0;
  box-shadow: none;
  padding: 4px 8px;
}

scrollbar { background-color: transparent; }
scrollbar slider { background-color: %GRID%; border-radius: 0; min-width: 6px; min-height: 6px; }
scrollbar slider:hover { background-color: %FG_DIM%; }

selection { background-color: alpha(%ACCENT%, 0.35); color: %FG%; }
*:focus-visible { outline: 1px solid %ACCENT%; outline-offset: -1px; }

/* --- Utility classes the adwaita sheet used to provide --- */
.title-1 { font-weight: 800; font-size: 170%; }
.title-2 { font-weight: 800; font-size: 140%; }
.title-3 { font-weight: 700; font-size: 120%; }
.title-4 { font-weight: 700; font-size: 105%; }
.large-title { font-weight: 300; font-size: 200%; }
.heading { font-weight: 700; }
.caption { font-size: 82%; }
.caption-heading { font-weight: 700; font-size: 82%; }
.dim-label { color: %FG_DIM%; }
.success { color: %OK%; }
.accent { color: %ACCENT%; }
.numeric { font-feature-settings: 'tnum'; }

/* --- Typography (Phase 13d): the only three font rules, test-enforced --- */
window, popover, dropdown, tooltip { font-family: 'Inter', sans-serif; }
.title-1, .title-2, .title-3, .title-4, .large-title, .heading { font-family: 'Fraunces', serif; }
.tech { font-family: 'IBM Plex Mono', monospace; }

/* --- App-owned rules (migrated from the old main.rs sheet) --- */
columnview.data-table > listview > row > cell { padding-top: 1px; padding-bottom: 1px; }
columnview.data-table > listview > row { transition: background-color 150ms ease; }
columnview.data-table > listview > row:hover { background: alpha(currentColor, 0.04); }
columnview > header > button { padding-top: 2px; padding-bottom: 2px; min-height: 0; border-width: 0; background-color: transparent; transition: background-color 150ms ease; }
columnview > header > button:hover { background: alpha(currentColor, 0.08); }
.rating-stars { color: %ACCENT%; }
.filter-warn text { background-color: alpha(%WARN%, 0.20); }
.now-bar { padding: 6px 12px; border-top: 1px solid %GRID%; }
.now-bar-cover { border-radius: 6px; box-shadow: 0 1px 5px rgba(0,0,0,0.40); background: alpha(currentColor, 0.06); }
.queue-row { padding: 4px 8px; border-radius: 0; }
.queue-row.playing { background: alpha(%ACCENT%, 0.16); }
.queue-list { border-left: 1px solid %GRID%; }
.chapter-row { padding: 3px 6px; border-radius: 0; transition: background-color 150ms ease; }
.chapter-row:hover { background: alpha(currentColor, 0.06); }
.chapter-row.current-chapter { background: alpha(%ACCENT%, 0.16); font-weight: bold; }
.sleep-menu-row { transition: background-color 150ms ease; }
.sleep-menu-row:hover { background: alpha(currentColor, 0.08); }
.book-tile { padding: 8px; border-radius: 0; }
.book-tile:selected { background: alpha(%ACCENT%, 0.18); }
.cover-art { border-radius: 10px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 4px 14px rgba(0,0,0,0.28); background: alpha(currentColor, 0.05); }
.cover-thumb { border-radius: 4px; background: alpha(currentColor, 0.06); }
.book-cover { border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 4px 12px rgba(0,0,0,0.26); background: alpha(currentColor, 0.06); }
.inspector-cover { border-radius: 10px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 6px 18px rgba(0,0,0,0.30); background: alpha(currentColor, 0.06); }
.now-playing-cover { border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 6px 18px rgba(0,0,0,0.30); background: alpha(currentColor, 0.06); }
.now-playing-drawer { border-top: 1px solid %GRID%; }
.now-playing-info { background-color: alpha(%BG_WINDOW%, 0.72); border-radius: 0; padding: 8px 14px; }
.spectrum { background: alpha(currentColor, 0.03); }
.toast { background-color: %BG_CARD%; color: %FG%; border: 1px solid %GRID%; border-radius: 0; padding: 6px 12px; }
";

/// The full generated sheet: the template with every `%TOKEN%` replaced by
/// its baked Dragon hex.
pub fn sheet() -> String {
    TEMPLATE
        .replace("%BG_WINDOW%", BG_WINDOW)
        .replace("%BG_VIEW%", BG_VIEW)
        .replace("%BG_HEADER%", BG_HEADER)
        .replace("%BG_CARD%", BG_CARD)
        .replace("%FG_DIM%", FG_DIM)
        .replace("%FG%", FG)
        .replace("%GRID%", GRID)
        .replace("%ACCENT%", ACCENT)
        .replace("%ON_ACCENT%", ON_ACCENT)
        .replace("%WARN%", WARN)
        .replace("%ERR%", ERR)
        .replace("%OK%", OK)
}

/// Install the sheet display-wide, one step above USER priority: a themed
/// `~/.config/gtk-4.0/gtk.css` loads at USER (800) and outranks APPLICATION
/// (600), silently half-overriding an in-app theme (the Colophon discovery);
/// USER + 1 keeps the owned sheet authoritative over it.
pub fn install() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&sheet());
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_palette_hex_reaches_the_sheet() {
        let sheet = sheet();
        for hex in [
            BG_WINDOW, BG_VIEW, BG_HEADER, BG_CARD, FG, FG_DIM, GRID, ACCENT, ON_ACCENT, WARN, OK,
        ] {
            assert!(sheet.contains(hex), "missing {hex}");
        }
        // Every placeholder was replaced (bare `%` is fine: font sizes use it;
        // ERR aliases ACCENT, so it is covered above).
        for token in [
            "%BG_WINDOW%",
            "%BG_VIEW%",
            "%BG_HEADER%",
            "%BG_CARD%",
            "%FG%",
            "%FG_DIM%",
            "%GRID%",
            "%ACCENT%",
            "%ON_ACCENT%",
            "%WARN%",
            "%ERR%",
            "%OK%",
        ] {
            assert!(!sheet.contains(token), "unreplaced {token}");
        }
    }

    #[test]
    fn exactly_three_font_family_rules() {
        // The Phase 13d typography and nothing else (the no-assumed-fonts rule).
        assert_eq!(sheet().matches("font-family").count(), 3);
    }

    #[test]
    fn key_owned_rules_exist() {
        let sheet = sheet();
        for needle in [
            "headerbar",
            ".destructive-action",
            ".suggested-action",
            ".toast",
            ".dim-label",
            ".boxed-list",
            "switch",
            "scale > trough",
        ] {
            assert!(sheet.contains(needle), "missing rule {needle}");
        }
        assert!(!sheet.contains("@define-color"));
    }
}
