//! Stage 3 of Now Playing: the full surface (19b-ii).
//!
//! Now Playing has three stages, and each is a deliberate step up in how much of
//! the screen it is worth giving the current track:
//!
//! 1. The **Now-bar** (`now_bar.rs`): always present, transport only.
//! 2. The **drawer** (`now_playing_panel.rs`): slides up, adds the spectrum and
//!    what is playing. Unchanged by this module.
//! 3. **This**: a page that takes over the content area, with a large cover, a
//!    full-height visualizer, lyrics, and the track's technical detail.
//!
//! Stage 3 is built to be *left open*, not glanced at, which drives two choices.
//! The layout is calm at rest (nothing blinks or slides except the visualizer
//! and the lyric highlight), and every value on it comes from columns `tracks`
//! already has. It deliberately does NOT reach for richer credits: roadmap
//! 19b-iii owns that, its scope is undefined, and it must not be dragged into
//! the 0.4.0 tag through the back door of this surface.
//!
//! Lyrics come from `conservatory_core::lyrics`, which reads a local `.lrc`
//! sidecar or the file's embedded tag and never the network.

use std::cell::{Cell, RefCell};
use std::path::Path;

use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::lyrics::Lyrics;

use crate::ui::accent::AccentProvider;
use crate::ui::spectrum::{Spectrum, build_spectrum};

/// How large the hero cover is drawn. Big enough to be the anchor of the page,
/// small enough to leave the visualizer and lyrics real room at a half tile.
const COVER_PX: i32 = 320;

/// The technical detail rows for a track, in display order.
///
/// Pure, so the wording and the "leave it out when it is unknown" rule can be
/// tested without a display. Only columns `tracks` already has: this surface
/// must not become the back door through which the scope-undefined 19b-iii
/// credits work arrives.
pub fn track_meta_rows(track: &conservatory_core::db::Track) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    // Format line: "FLAC · 1411 kbps · 44.1 kHz", skipping whichever parts are
    // unknown rather than printing a row of dashes.
    let mut fmt_parts: Vec<String> = Vec::new();
    if let Some(f) = track.format.as_deref().filter(|f| !f.is_empty()) {
        fmt_parts.push(f.to_uppercase());
    }
    if let Some(b) = track.bitrate.filter(|b| *b > 0) {
        fmt_parts.push(format!("{b} kbps"));
    }
    if let Some(sr) = track.sample_rate.filter(|s| *s > 0) {
        fmt_parts.push(format!("{:.1} kHz", f64::from(sr) / 1000.0));
    }
    if !fmt_parts.is_empty() {
        rows.push(("Format".into(), fmt_parts.join(" · ")));
    }

    if let Some(n) = track.track_no.filter(|n| *n > 0) {
        let text = match track.disc_no.filter(|d| *d > 1) {
            Some(d) => format!("{n} (disc {d})"),
            None => n.to_string(),
        };
        rows.push(("Track".into(), text));
    }

    // ReplayGain is why a track sounds level against its neighbours, so it is
    // worth stating rather than leaving as an invisible engine detail.
    match (track.replaygain_track, track.replaygain_album) {
        (Some(t), Some(a)) => {
            rows.push((
                "ReplayGain".into(),
                format!("{t:+.2} dB track · {a:+.2} dB album"),
            ));
        }
        (Some(t), None) => rows.push(("ReplayGain".into(), format!("{t:+.2} dB track"))),
        (None, Some(a)) => rows.push(("ReplayGain".into(), format!("{a:+.2} dB album"))),
        (None, None) => {}
    }

    if track.rating > 0 {
        rows.push(("Rating".into(), format!("{} of 5", track.rating.min(5))));
    }
    if track.play_count > 0 {
        let plays = track.play_count;
        let word = if plays == 1 { "play" } else { "plays" };
        rows.push(("Played".into(), format!("{plays} {word}")));
    }

    rows
}

/// The full Now Playing page: the widget to place, plus the live parts.
pub struct NowPlayingFull {
    /// The page root, added to the window's view stack.
    pub root: gtk::Box,
    /// Walks back down to stage 2. The window owns what that means.
    pub back: gtk::Button,

    cover: gtk::Image,
    cover_frame: gtk::Frame,
    accent: AccentProvider,

    title: gtk::Label,
    subtitle: gtk::Label,

    /// Technical detail, rebuilt per item. A grid rather than a wrapped string
    /// so the labels stay aligned as values change width.
    meta: gtk::Grid,

    spectrum: Spectrum,

    /// One label per lyric line, so a single line can be lit without rebuilding
    /// the block on every tick.
    lyric_lines: RefCell<Vec<gtk::Label>>,
    lyric_box: gtk::Box,
    lyric_scroller: gtk::ScrolledWindow,
    /// Swaps the lines for a plain "no lyrics" note.
    lyric_stack: gtk::Stack,
    lyric_plain: gtk::Label,
    lyrics: RefCell<Option<Lyrics>>,
    /// Which line is currently lit, so a tick only touches CSS when the
    /// playhead actually crosses a boundary.
    active_line: Cell<Option<usize>>,

    /// Populated content vs the idle "nothing playing" state.
    stack: gtk::Stack,
}

pub fn build_now_playing_full() -> NowPlayingFull {
    // -- header: just a way back. The page title is the track itself.
    let back = gtk::Button::builder()
        .icon_name("go-previous-symbolic")
        .tooltip_text("Back (Escape)")
        .css_classes(["flat"])
        .build();
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(10)
        .margin_bottom(2)
        .build();
    header.append(&back);
    header.append(
        &gtk::Label::builder()
            .label("Now Playing")
            .css_classes(["heading", "dim-label"])
            .hexpand(true)
            .xalign(0.0)
            .build(),
    );

    // -- hero: cover on the left, identity and detail on the right.
    let cover = gtk::Image::builder()
        .pixel_size(COVER_PX)
        .icon_name("audio-x-generic-symbolic")
        .build();
    let cover_frame = gtk::Frame::builder()
        .css_classes(["now-playing-cover", "now-playing-cover-hero"])
        .child(&cover)
        .valign(gtk::Align::Start)
        .halign(gtk::Align::Start)
        .build();

    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(3)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-1"])
        .label("Nothing playing")
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-4", "dim-label"])
        .build();

    let meta = gtk::Grid::builder()
        .row_spacing(4)
        .column_spacing(18)
        .margin_top(14)
        .build();

    let identity = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .hexpand(true)
        .valign(gtk::Align::Start)
        .build();
    identity.append(&title);
    identity.append(&subtitle);
    identity.append(&meta);

    let hero = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .margin_start(24)
        .margin_end(24)
        .margin_top(12)
        .build();
    hero.append(&cover_frame);
    hero.append(&identity);

    // -- visualizer: the same widget the drawer uses, given real height here.
    let spectrum = build_spectrum();
    spectrum.area.set_content_height(200);
    spectrum.area.set_vexpand(false);
    let vis_frame = gtk::Frame::builder()
        .css_classes(["now-playing-vis"])
        .child(&spectrum.area)
        .margin_start(24)
        .margin_end(24)
        .margin_top(18)
        .build();

    // -- lyrics
    let lyric_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_start(24)
        .margin_end(24)
        .margin_top(8)
        .margin_bottom(16)
        .build();
    let lyric_plain = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["lyric-plain"])
        .margin_start(24)
        .margin_end(24)
        .margin_top(8)
        .margin_bottom(16)
        .build();
    let lyric_none = gtk::Label::builder()
        .label("No lyrics found for this track.")
        .css_classes(["dim-label"])
        .margin_top(24)
        .margin_bottom(24)
        .build();

    let lyric_stack = gtk::Stack::new();
    lyric_stack.add_named(&lyric_box, Some("synced"));
    lyric_stack.add_named(&lyric_plain, Some("plain"));
    lyric_stack.add_named(&lyric_none, Some("none"));
    lyric_stack.set_visible_child_name("none");

    let lyric_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&lyric_stack)
        .build();

    // -- assembly
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&hero);
    content.append(&vis_frame);
    content.append(&lyric_scroller);

    let empty = gtk::Label::builder()
        .label("Nothing playing")
        .css_classes(["dim-label", "title-2"])
        .vexpand(true)
        .valign(gtk::Align::Center)
        .build();

    let stack = gtk::Stack::new();
    stack.add_named(&content, Some("content"));
    stack.add_named(&empty, Some("empty"));
    stack.set_visible_child_name("empty");
    stack.set_vexpand(true);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["now-playing-full"])
        .build();
    root.append(&header);
    root.append(&stack);

    NowPlayingFull {
        root,
        back,
        cover,
        cover_frame,
        accent: AccentProvider::new(),
        title,
        subtitle,
        meta,
        spectrum,
        lyric_lines: RefCell::new(Vec::new()),
        lyric_box,
        lyric_scroller,
        lyric_stack,
        lyric_plain,
        lyrics: RefCell::new(None),
        active_line: Cell::new(None),
        stack,
    }
}

impl NowPlayingFull {
    /// Mirrors the drawer's setter so the window can drive both the same way.
    pub fn set_now_playing(&self, title: &str, subtitle: &str) {
        self.title.set_text(title);
        self.subtitle.set_text(subtitle);
        self.subtitle.set_visible(!subtitle.is_empty());
        self.stack.set_visible_child_name("content");
    }

    pub fn set_cover(&self, cover_abs: Option<&Path>, accent: Option<u32>) {
        match cover_abs.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some("audio-x-generic-symbolic")),
        }
        self.accent.apply_cover_ring(
            &self.cover_frame,
            &["now-playing-cover", "now-playing-cover-hero"],
            accent,
        );
        self.spectrum.set_accent(accent);
    }

    pub fn set_playing(&self, playing: bool) {
        self.spectrum.set_playing(playing);
    }

    /// Replace the technical detail. Pairs are rendered label / value, and an
    /// empty slice leaves the grid blank rather than showing stale values from
    /// the previous track.
    pub fn set_metadata(&self, pairs: &[(String, String)]) {
        while let Some(child) = self.meta.first_child() {
            self.meta.remove(&child);
        }
        for (row, (key, value)) in pairs.iter().enumerate() {
            let k = gtk::Label::builder()
                .label(key)
                .xalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build();
            let v = gtk::Label::builder()
                .label(value)
                .xalign(0.0)
                .selectable(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            self.meta.attach(&k, 0, row as i32, 1, 1);
            self.meta.attach(&v, 1, row as i32, 1, 1);
        }
    }

    /// Show the track's lyrics, or the "none" note. Called on item change, not
    /// per tick: `tick` only moves the highlight.
    pub fn set_lyrics(&self, lyrics: Option<Lyrics>) {
        self.active_line.set(None);
        while let Some(child) = self.lyric_box.first_child() {
            self.lyric_box.remove(&child);
        }
        self.lyric_lines.borrow_mut().clear();

        match lyrics.filter(|l| !l.is_empty()) {
            Some(Lyrics::Synced(lines)) => {
                let mut labels = Vec::with_capacity(lines.len());
                for line in &lines {
                    // A blank line is an instrumental gap, not a mistake: it
                    // still gets a label so the highlight can rest on nothing
                    // rather than leaving the previous lyric lit through it.
                    let label = gtk::Label::builder()
                        .label(&line.text)
                        .xalign(0.0)
                        .wrap(true)
                        .css_classes(["lyric-line"])
                        .build();
                    self.lyric_box.append(&label);
                    labels.push(label);
                }
                *self.lyric_lines.borrow_mut() = labels;
                *self.lyrics.borrow_mut() = Some(Lyrics::Synced(lines));
                self.lyric_stack.set_visible_child_name("synced");
            }
            Some(Lyrics::Unsynced(text)) => {
                self.lyric_plain.set_text(&text);
                *self.lyrics.borrow_mut() = Some(Lyrics::Unsynced(text));
                self.lyric_stack.set_visible_child_name("plain");
            }
            None => {
                *self.lyrics.borrow_mut() = None;
                self.lyric_stack.set_visible_child_name("none");
            }
        }
    }

    /// Move the lyric highlight to wherever `pos` seconds falls.
    ///
    /// Cheap to call every poll tick: it computes the active line and returns
    /// immediately unless the playhead has actually crossed into a new one.
    pub fn tick(&self, pos: f64) {
        let want = match self.lyrics.borrow().as_ref() {
            Some(l) => l.active_line(pos),
            None => None,
        };
        if want == self.active_line.get() {
            return;
        }
        let labels = self.lyric_lines.borrow();
        if let Some(prev) = self.active_line.get()
            && let Some(l) = labels.get(prev)
        {
            l.remove_css_class("lyric-line-active");
        }
        if let Some(idx) = want
            && let Some(l) = labels.get(idx)
        {
            l.add_css_class("lyric-line-active");
            self.scroll_to(l);
        }
        self.active_line.set(want);
    }

    /// Keep the lit line in view, centred where the block is long enough.
    fn scroll_to(&self, label: &gtk::Label) {
        let vadj = self.lyric_scroller.vadjustment();
        // compute_bounds, not the 4.12-deprecated allocation/translate_coordinates.
        // It returns None before the label has been laid out, which is exactly
        // when scrolling would jump to the wrong place, so bail and let the next
        // tick place it.
        let Some(bounds) = label.compute_bounds(&self.lyric_box) else {
            return;
        };
        let target =
            f64::from(bounds.y()) - (vadj.page_size() / 2.0) + f64::from(bounds.height()) / 2.0;
        let clamped = target.clamp(vadj.lower(), (vadj.upper() - vadj.page_size()).max(0.0));
        vadj.set_value(clamped);
    }

    /// The idle "nothing playing" state.
    pub fn clear(&self) {
        self.set_now_playing("Nothing playing", "");
        self.set_cover(None, None);
        self.set_metadata(&[]);
        self.set_lyrics(None);
        self.stack.set_visible_child_name("empty");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conservatory_core::db::Track;

    fn bare_track() -> Track {
        Track {
            id: 1,
            album_id: None,
            artist_id: None,
            title: "T".into(),
            track_no: None,
            disc_no: None,
            duration: None,
            file_path: "a.flac".into(),
            format: None,
            bitrate: None,
            sample_rate: None,
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: None,
        }
    }

    fn value_for<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
        rows.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_track_with_nothing_known_shows_nothing() {
        // Better an empty panel than a column of "Unknown".
        assert!(track_meta_rows(&bare_track()).is_empty());
    }

    #[test]
    fn format_line_joins_only_what_is_known() {
        let mut t = bare_track();
        t.format = Some("flac".into());
        t.sample_rate = Some(44100);
        let rows = track_meta_rows(&t);
        assert_eq!(value_for(&rows, "Format"), Some("FLAC · 44.1 kHz"));
    }

    #[test]
    fn zero_valued_fields_are_treated_as_unknown() {
        // The importer writes 0 where a decoder reported nothing; printing
        // "0 kbps" would state a falsehood about the file.
        let mut t = bare_track();
        t.bitrate = Some(0);
        t.sample_rate = Some(0);
        t.track_no = Some(0);
        assert!(track_meta_rows(&t).is_empty());
    }

    #[test]
    fn disc_number_appears_only_on_a_multi_disc_release() {
        let mut t = bare_track();
        t.track_no = Some(4);
        t.disc_no = Some(1);
        assert_eq!(value_for(&track_meta_rows(&t), "Track"), Some("4"));
        t.disc_no = Some(2);
        assert_eq!(value_for(&track_meta_rows(&t), "Track"), Some("4 (disc 2)"));
    }

    #[test]
    fn replaygain_reports_whichever_halves_exist() {
        let mut t = bare_track();
        t.replaygain_track = Some(-7.25);
        assert_eq!(
            value_for(&track_meta_rows(&t), "ReplayGain"),
            Some("-7.25 dB track")
        );
        t.replaygain_album = Some(3.5);
        assert_eq!(
            value_for(&track_meta_rows(&t), "ReplayGain"),
            Some("-7.25 dB track · +3.50 dB album")
        );
    }

    #[test]
    fn play_count_is_pluralised() {
        let mut t = bare_track();
        t.play_count = 1;
        assert_eq!(value_for(&track_meta_rows(&t), "Played"), Some("1 play"));
        t.play_count = 2;
        assert_eq!(value_for(&track_meta_rows(&t), "Played"), Some("2 plays"));
    }
}
