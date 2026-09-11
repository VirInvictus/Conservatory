//! The owned application stylesheet (Phase 26l, restructured 2026-09-06):
//! the app-owned remainder over vir-gtk's shared base sheet. The unanimous
//! flat/square widget core (window chrome, headerbar, lists, the button
//! core, entries, popovers, tooltips, scrollbars, utility classes, focus
//! ring) now lives in `vir_gtk::theme::base_css`, installed at USER + 1;
//! this sheet carries only what Conservatory deliberately does differently,
//! installed at USER + 2 (`install_app_stylesheet`) so it wins by priority,
//! not by load order. The look is the locked spec §2.4 design language:
//! flat, square, hard 1px borders, denser spacing than the GNOME HIG.
//! Custom properties are deliberately not used (one fixed palette, and
//! skipping them keeps the gtk4 crate on `v4_14`), so the hexes are spliced
//! by token replacement instead.
//!
//! Deliberate exceptions to "flat" that live HERE, not in the base: the
//! lifted album/book cover cards keep their radius and drop shadow (the
//! Hermitage cover-as-visual-unit pattern; the runtime accent ring in
//! `ui/accent.rs` layers onto that same shadow). Chrome is flat; content
//! imagery stays lifted. A selected row LIFTS with an accent edge, and a
//! checked button is LIT (lift + accent text + underline), not painted.
//!
//! Typography carries over from Phase 13d: exactly three `font-family`
//! rules (Inter body, Fraunces headers, IBM Plex Mono technical), fonts
//! bundled via fontconfig in `main.rs`.

/// The palette with Conservatory's accent override.
fn palette() -> vir_gtk::theme::Palette {
    let mut p = vir_gtk::theme::Palette::dragon();
    p.accent = "#c4746e"; // dragonRed
    p.on_accent = "#12120f";
    p
}

/// The shared base sheet, spliced with Conservatory's palette. Installed at
/// USER + 1 by [`install`]; the app sheet at USER + 2 overrides it.
pub fn base() -> String {
    vir_gtk::theme::base_css(&palette())
}

/// The app-owned remainder. `%TOKENS%` are replaced with [`palette`]'s hexes
/// by [`sheet`]; no other substitution happens, so plain CSS braces are safe.
const TEMPLATE: &str = "\
/* --- Conservatory overrides over the base sheet --- */
/* A selected row LIFTS and gets an accent edge; it is not washed in red.
   alpha(dragonRed, 0.35) over the dark view reads as maroon, and because a
   selection persists (the facet panes always have an [All] row selected, the
   podcast triage always has a list selected) that wash was permanent chrome
   rather than a highlight. The 2px inset edge keeps the accent as a signature
   and reads better against the Columns-UI density this browse surface copies. */
row:selected { background-color: %BG_RAISED%; color: %FG%; box-shadow: inset 2px 0 0 %ACCENT%; }
.navigation-sidebar { background-color: %BG_VIEW%; }
.navigation-sidebar > row { padding: 4px 8px; border-radius: 0; }
columnview > header { background-color: %BG_VIEW%; border-bottom: 1px solid %GRID%; }
/* A checked button is LIT, not PAINTED. dragonRed is a syntax accent: spread
   across a persistent surface like the Music/Podcasts/Audiobooks tab bar it
   stops reading as Kanagawa and starts reading as a pink highlighter, and
   docs/theme.md is explicit that the accent tints highlights only and does not
   recolour the window. So the surface lifts to dragonBlack4 and the accent
   arrives as text plus a 2px underline, which is far more legible about which
   tab is active than a slab of colour was. */
button:active { background-color: %BG_RAISED%; color: %FG%; border-color: %GRID%; }
button:checked {
  background-color: %BG_RAISED%;
  color: %ACCENT%;
  border-color: %GRID%;
  box-shadow: inset 0 -2px 0 %ACCENT%;
}
/* Insensitive controls must read as such: without this the flat sheet leaves
   a disabled button visually identical to a live one. */
entry:disabled, spinbutton:disabled, dropdown:disabled, switch:disabled, check:disabled, scale:disabled { opacity: 0.55; }
/* Icon-only buttons (headerbar / toolbars) read as flat like the rest of the
   chrome: a hard 1px border around every gear/list glyph is what made the top
   bar look boxy. GTK tags icon-only buttons `.image-button`; they de-box here
   and only fill on hover, keeping the flat identity. */
button.flat, button.circular, button.image-button { background-color: transparent; border-color: transparent; }
button.flat:hover, button.circular:hover, button.image-button:hover { background-color: %GRID%; }
button.circular { border-radius: 0; }
button.pill { padding: 6px 18px; background-color: %BG_CARD%; border-color: %FG_DIM%; }
button.pill:hover { background-color: %GRID%; }
modelbutton:hover { background-color: %BG_RAISED%; color: %FG%; }
.osd { background-color: alpha(%BG_WINDOW%, 0.80); color: %FG%; border-radius: 0; }
switch { background-color: %GRID%; border: 1px solid %GRID%; border-radius: 0; }
switch:checked { background-color: %ACCENT%; border-color: %ACCENT%; }
switch > slider { background-color: %FG%; border: 1px solid %GRID%; border-radius: 0; min-width: 18px; min-height: 18px; }
check { background-color: %BG_VIEW%; border: 1px solid %GRID%; border-radius: 0; }
check:checked { background-color: %ACCENT%; color: %ON_ACCENT%; border-color: %ACCENT%; }
scale > trough { background-color: %GRID%; border-radius: 0; }
scale > trough > highlight { background-color: %ACCENT%; border-radius: 0; }
scale > trough > slider { background-color: %FG%; border: 1px solid %GRID%; border-radius: 0; box-shadow: none; }
scale > marks-after, scale > marks-before { color: %FG_DIM%; }
/* --- Typography (Phase 13d): the only three font rules --- */
window, popover, dropdown, tooltip { font-family: 'Inter', sans-serif; }
.title-1, .title-2, .title-3, .title-4, .large-title, .heading { font-family: 'Fraunces', serif; }
.tech { font-family: 'IBM Plex Mono', monospace; }
/* --- App-owned rules (migrated from the old main.rs sheet) --- */
columnview.data-table > listview > row > cell { padding-top: 1px; padding-bottom: 1px; }
columnview.data-table > listview > row { transition: background-color 150ms ease; }
/* Scannability option (the post-0.3.0 follow-on, config `[browse].row_style`):
   a faint line between track rows or a subtle even-row tint, both quieter than
   the pre-0.3.9 grid the density pass removed, and both off by default. The
   zebra rule precedes the hover rule below so hover keeps winning on a hovered
   stripe. */
columnview.row-lines > listview > row { border-bottom: 1px solid alpha(%GRID%, 0.55); }
columnview.zebra-rows > listview > row:nth-child(even) { background-color: alpha(%FG%, 0.032); }
columnview.data-table > listview > row:hover { background: alpha(currentColor, 0.04); }
/* Facet-pane rows match the track list's tightened density (the post-0.3.0
   follow-on): the leaf row box is the 24px cover + 1px cell padding, so the
   pane rows pin to the same height instead of the label's natural one. */
columnview.facet-pane > listview > row { min-height: 26px; }
/* Column headers read as quiet labels, not buttons (the deadbeef/foobar look):
   dimmed, slightly smaller, a touch of tracking. */
columnview > header > button { padding-top: 2px; padding-bottom: 2px; min-height: 0; border-width: 0; background-color: transparent; color: %FG_DIM%; font-size: 0.92em; letter-spacing: 0.02em; transition: background-color 150ms ease; }
columnview > header > button:hover { background: alpha(currentColor, 0.08); }
.rating-stars { color: %WARN%; }  /* dragonYellow: gold reads as a rating; a column of red stars shouted */
.filter-warn text { background-color: alpha(%WARN%, 0.20); }
/* Empty-state call-to-action buttons render in the house idiom as a roomier
   bordered button rather than a rounded capsule. */
.status-bar { padding: 2px 12px; border-top: 1px solid %GRID%; }
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
/* Stage 3 (19b-ii). Built to be left open, so it stays quiet: a deeper shadow
   under the hero cover to seat it, a framed visualizer, and lyrics that sit back
   until their line comes round. */
.now-playing-full { background-color: %BG_WINDOW%; }
.now-playing-cover-hero { border-radius: 12px; box-shadow: 0 2px 6px rgba(0,0,0,0.34), 0 14px 40px rgba(0,0,0,0.38); }
.now-playing-vis { border: 1px solid %GRID%; border-radius: 6px; background: alpha(currentColor, 0.03); }
.lyric-line { color: alpha(%FG%, 0.42); padding: 3px 0; font-size: 1.05em; }
/* The lit line is the one thing on the page that moves, so it earns full
   contrast and a little weight; everything else recedes behind it. */
.lyric-line-active { color: %FG%; font-weight: 700; }
.lyric-plain { color: alpha(%FG%, 0.72); }
.now-playing-info { background-color: alpha(%BG_WINDOW%, 0.72); border-radius: 0; padding: 8px 14px; }
.spectrum { background: alpha(currentColor, 0.03); }
.toast { background-color: %BG_CARD%; color: %FG%; border: 1px solid %GRID%; border-radius: 0; padding: 6px 12px; }
";

/// The full app-owned sheet: the template with every `%TOKEN%` replaced by
/// its baked Dragon hex.
pub fn sheet() -> String {
    palette().replace_tokens(TEMPLATE)
}

/// Install the shared base sheet (USER + 1) then this app sheet (USER + 2),
/// so Conservatory's deliberate divergences outrank the base by priority.
pub fn install() {
    vir_gtk::theme::install_stylesheet(&base());
    vir_gtk::theme::install_app_stylesheet(&sheet());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typography_stays_the_only_font_rules() {
        // Phase 13d contract: exactly three `font-family` rules (Inter body,
        // Fraunces headings, IBM Plex Mono technical), all app-owned.
        assert_eq!(sheet().matches("font-family").count(), 3);
    }

    #[test]
    fn no_tokens_survive_replacement() {
        // A leftover %TOKEN% would ship literally into GTK's CSS parser.
        assert!(!sheet().contains('%'));
    }

    #[test]
    fn row_style_selectors_are_pinned() {
        // The 0.5.0 scannability option (config `[browse].row_style`): both
        // leaf classes must have rules, so a rename breaks this test instead
        // of silently unbinding the config key.
        assert!(sheet().contains("columnview.row-lines > listview > row"));
        assert!(sheet().contains("columnview.zebra-rows > listview > row"));
        // The facet-pane density pin rides the same contract.
        assert!(sheet().contains("columnview.facet-pane > listview > row"));
    }
}
