//! The Now Playing drawer (v0.0.38): a bottom slide-up `gtk::Revealer`, the
//! horizontal twin of the right-docked queue drawer. Clicking the Now-bar
//! cover/title (or Ctrl+I) reveals the current item's full metadata; it updates
//! as the queue advances. The content area is also the intended home for the
//! future spectrum visualizer (the deferred Phase 11 item), which would sit
//! beside the metadata.
//!
//! The field projection (`track_fields` / `episode_fields`) is pure and
//! unit-tested; the window resolves the rows from the DB and hands them in, so
//! this module builds no DB reads.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::PlayerHandle;
use conservatory_core::db::{Album, Chapter, Episode, NowPlaying, Show, Track};

use crate::playqueue::fmt_secs;

/// The drawer: the revealer to place, plus the labelled grid it fills and the
/// episode extras (Smart Speed line + clickable chapter list, Phase 6c-iii-c).
pub struct NowPlayingPanel {
    pub revealer: gtk::Revealer,
    title: gtk::Label,
    grid: gtk::Grid,
    /// The "Smart Speed · saved m:ss" line; hidden unless the current item has
    /// Smart Speed on. Updated each poll tick from the snapshot.
    smart_speed: gtk::Label,
    /// Heading + list wrapper, hidden when the item has no chapters.
    chapters_box: gtk::Box,
    chapters_list: gtk::ListBox,
    /// Per-row chapter start seconds, indexed by row position; the row-activated
    /// handler (wired once at build) reads this to seek. Shared so a single
    /// handler survives list rebuilds (re-connecting would double-fire).
    chapter_starts: Rc<RefCell<Vec<f64>>>,
    /// The handle the chapter rows seek through; set on each `set_chapters`.
    player: Rc<RefCell<Option<PlayerHandle>>>,
    /// The currently-highlighted chapter row, so a tick only touches the CSS
    /// class when the playhead crosses a boundary.
    current_chapter: Cell<Option<usize>>,
}

/// Build the drawer (revealed off; the window appends `revealer` above the
/// Now-bar and toggles it).
pub fn build_now_playing_panel() -> NowPlayingPanel {
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-4"])
        .label("Now Playing")
        .build();
    let grid = gtk::Grid::builder()
        .row_spacing(2)
        .column_spacing(16)
        .margin_top(4)
        .build();

    // Episode extras (6c-iii-c): a Smart Speed line and a clickable chapter list,
    // both hidden until the current item calls for them.
    let smart_speed = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["accent", "caption"])
        .margin_top(4)
        .visible(false)
        .build();

    let chapters_heading = gtk::Label::builder()
        .label("Chapters")
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_top(8)
        .build();
    let chapters_list = gtk::ListBox::new();
    chapters_list.set_selection_mode(gtk::SelectionMode::None);
    chapters_list.add_css_class("chapter-list");
    let chapters_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    chapters_box.append(&chapters_heading);
    chapters_box.append(&chapters_list);
    chapters_box.set_visible(false);

    let chapter_starts = Rc::new(RefCell::new(Vec::<f64>::new()));
    let player = Rc::new(RefCell::new(None::<PlayerHandle>));
    // Wire row-activation once: clicking a chapter seeks to its start. The starts
    // + handle are shared cells so the handler outlives the list rebuilds.
    chapters_list.connect_row_activated({
        let starts = chapter_starts.clone();
        let player = player.clone();
        move |_list, row| {
            let idx = row.index();
            if idx < 0 {
                return;
            }
            let start = starts.borrow().get(idx as usize).copied();
            if let (Some(p), Some(start)) = (player.borrow().as_ref(), start) {
                p.seek(start);
            }
        }
    });

    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.add_css_class("background");
    column.add_css_class("now-playing-drawer");
    column.set_margin_top(8);
    column.set_margin_bottom(8);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.append(&title);
    column.append(&grid);
    column.append(&smart_speed);
    column.append(&chapters_box);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(150)
        .max_content_height(280)
        .propagate_natural_height(true)
        .child(&column)
        .build();

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .reveal_child(false)
        .child(&scroller)
        .build();

    NowPlayingPanel {
        revealer,
        title,
        grid,
        smart_speed,
        chapters_box,
        chapters_list,
        chapter_starts,
        player,
        current_chapter: Cell::new(None),
    }
}

impl NowPlayingPanel {
    /// Toggle the drawer's visibility.
    pub fn toggle(&self) {
        self.revealer
            .set_reveal_child(!self.revealer.reveals_child());
    }

    pub fn is_open(&self) -> bool {
        self.revealer.reveals_child()
    }

    /// Replace the shown rows. `title` heads the drawer; `fields` are
    /// label/value pairs rendered as a two-column grid (long values wrap).
    pub fn set_fields(&self, title: &str, fields: &[(String, String)]) {
        self.title.set_text(title);
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (row, (label, value)) in fields.iter().enumerate() {
            let key = gtk::Label::builder()
                .label(label)
                .xalign(0.0)
                .yalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build();
            let val = gtk::Label::builder()
                .label(value)
                .xalign(0.0)
                .yalign(0.0)
                .wrap(true)
                .selectable(true)
                .hexpand(true)
                .build();
            self.grid.attach(&key, 0, row as i32, 1, 1);
            self.grid.attach(&val, 1, row as i32, 1, 1);
        }
    }

    /// Rebuild the chapter list for the current item (Phase 6c-iii-c). An empty
    /// slice hides the section (a track / chapter-less episode). `player` is the
    /// handle a chapter click seeks through. Called on item-change, not per tick.
    pub fn set_chapters(&self, chapters: &[Chapter], player: &PlayerHandle) {
        self.current_chapter.set(None);
        while let Some(child) = self.chapters_list.first_child() {
            self.chapters_list.remove(&child);
        }
        *self.chapter_starts.borrow_mut() = chapters.iter().map(|c| c.start_time).collect();
        *self.player.borrow_mut() = Some(player.clone());

        if chapters.is_empty() {
            self.chapters_box.set_visible(false);
            return;
        }
        for (i, ch) in chapters.iter().enumerate() {
            let title = match ch.title.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => t.to_string(),
                None => format!("Chapter {}", i + 1),
            };
            let label = gtk::Label::builder()
                .label(format!("{}   {title}", fmt_secs(ch.start_time)))
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&label));
            row.add_css_class("chapter-row");
            self.chapters_list.append(&row);
        }
        self.chapters_box.set_visible(true);
    }

    /// Highlight the chapter the playhead is in (Phase 6c-iii-c); cheap per tick,
    /// it only touches the CSS class when the index changes.
    pub fn set_current_chapter(&self, idx: Option<usize>) {
        if self.current_chapter.get() == idx {
            return;
        }
        if let Some(prev) = self.current_chapter.get()
            && let Some(row) = self.chapters_list.row_at_index(prev as i32)
        {
            row.remove_css_class("current-chapter");
        }
        if let Some(cur) = idx
            && let Some(row) = self.chapters_list.row_at_index(cur as i32)
        {
            row.add_css_class("current-chapter");
        }
        self.current_chapter.set(idx);
    }

    /// Show / update the Smart Speed indicator (Phase 6c-iii-c). Hidden when the
    /// current item has no Smart Speed; otherwise the saved time ticks up live.
    pub fn set_smart_speed(&self, active: bool, saved_secs: f64) {
        if !active {
            self.smart_speed.set_visible(false);
            return;
        }
        let saved = fmt_secs(saved_secs);
        self.smart_speed
            .set_text(&format!("Smart Speed · saved {saved}"));
        self.smart_speed.set_tooltip_text(Some(&format!(
            "Smart Speed is shortening silences; {saved} saved this session"
        )));
        self.smart_speed.set_visible(true);
    }

    /// The idle "nothing playing" state.
    pub fn clear(&self) {
        self.set_fields("Now Playing", &[("".into(), "Nothing playing.".into())]);
        self.smart_speed.set_visible(false);
        self.chapters_box.set_visible(false);
        self.current_chapter.set(None);
        while let Some(child) = self.chapters_list.first_child() {
            self.chapters_list.remove(&child);
        }
    }
}

/// Push a non-empty `(label, value)` onto `out`; skips empty values so absent
/// fields do not render blank rows.
fn push(out: &mut Vec<(String, String)>, label: &str, value: impl Into<String>) {
    let value = value.into();
    if !value.is_empty() {
        out.push((label.to_string(), value));
    }
}

/// The drawer rows for a playing track: display fields from `np` (joined
/// title/artist/album) plus the technical detail from `track`/`album`. Pure.
pub fn track_fields(
    np: &NowPlaying,
    track: &Track,
    album: Option<&Album>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Artist", np.artist.clone().unwrap_or_default());
    push(&mut out, "Album", np.album.clone().unwrap_or_default());
    if let Some(al) = album {
        if let Some(y) = al.year {
            push(&mut out, "Year", y.to_string());
        }
        push(
            &mut out,
            "Genre",
            al.shelf_genre.clone().unwrap_or_default(),
        );
    }
    if let Some(len) = np.length {
        push(&mut out, "Duration", fmt_secs(len));
    }
    push(&mut out, "Format", track.format.clone().unwrap_or_default());
    if let Some(br) = track.bitrate.filter(|b| *b > 0) {
        push(&mut out, "Bitrate", format!("{} kbps", br / 1000));
    }
    if let Some(sr) = track.sample_rate.filter(|s| *s > 0) {
        push(
            &mut out,
            "Sample rate",
            format!("{:.1} kHz", sr as f64 / 1000.0),
        );
    }
    match (track.replaygain_track, track.replaygain_album) {
        (Some(t), Some(a)) => push(
            &mut out,
            "ReplayGain",
            format!("{t:+.2} dB track / {a:+.2} dB album"),
        ),
        (Some(t), None) => push(&mut out, "ReplayGain", format!("{t:+.2} dB track")),
        (None, Some(a)) => push(&mut out, "ReplayGain", format!("{a:+.2} dB album")),
        (None, None) => {}
    }
    if track.rating > 0 {
        push(&mut out, "Rating", "★".repeat(track.rating as usize));
    }
    if track.play_count > 0 {
        push(&mut out, "Plays", track.play_count.to_string());
    }
    push(&mut out, "File", track.file_path.clone());
    out
}

/// The drawer rows for a playing episode: show / date / runtime / source plus
/// the show notes. `streaming` reflects whether it is playing from the network.
/// Pure.
pub fn episode_fields(
    np: &NowPlaying,
    ep: &Episode,
    show: Option<&Show>,
    streaming: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Show", np.artist.clone().unwrap_or_default());
    if let Some(d) = ep.pub_date {
        push(&mut out, "Published", d.format("%Y-%m-%d").to_string());
    }
    if let Some(len) = np.length {
        push(&mut out, "Duration", fmt_secs(len));
    }
    push(
        &mut out,
        "Source",
        if streaming { "Streaming" } else { "Downloaded" },
    );
    if let Some(bytes) = ep.file_size.filter(|b| *b > 0) {
        push(
            &mut out,
            "Size",
            format!("{:.1} MB", bytes as f64 / 1_048_576.0),
        );
    }
    match (ep.season, ep.episode_number) {
        (Some(s), Some(e)) => push(&mut out, "Season/Episode", format!("S{s} E{e}")),
        (None, Some(e)) => push(&mut out, "Episode", e.to_string()),
        _ => {}
    }
    if let Some(t) = ep.episode_type.as_deref().filter(|t| *t != "full") {
        push(&mut out, "Type", t);
    }
    if let Some(sh) = show {
        push(&mut out, "Author", sh.author.clone().unwrap_or_default());
    }
    push(
        &mut out,
        "Notes",
        ep.description.clone().unwrap_or_default(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn track() -> Track {
        Track {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            title: "Xtal".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: Some(294.0),
            file_path: "Music/E/Album/01 - Xtal.flac".into(),
            format: Some("flac".into()),
            bitrate: Some(900_000),
            sample_rate: Some(44_100),
            replaygain_track: Some(-6.5),
            replaygain_album: Some(-6.0),
            rating: 4,
            play_count: 3,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: None,
        }
    }

    fn np(title: &str, artist: &str, album: Option<&str>) -> NowPlaying {
        NowPlaying {
            title: title.into(),
            artist: Some(artist.into()),
            album: album.map(str::to_string),
            length: Some(294.0),
            album_cover_path: None,
        }
    }

    #[test]
    fn track_fields_render_technical_detail() {
        let al = Album {
            id: 1,
            title: "Selected Ambient Works 85-92".into(),
            album_artist_id: Some(1),
            shelf_genre: Some("Electronic".into()),
            year: Some(1992),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "Music/Electronic/Aphex Twin/...".into(),
            added_at: None,
        };
        let rows = track_fields(
            &np("Xtal", "Aphex Twin", Some("SAW 85-92")),
            &track(),
            Some(&al),
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Artist"], "Aphex Twin");
        assert_eq!(map["Year"], "1992");
        assert_eq!(map["Genre"], "Electronic");
        assert_eq!(map["Format"], "flac");
        assert_eq!(map["Bitrate"], "900 kbps");
        assert_eq!(map["Sample rate"], "44.1 kHz");
        assert_eq!(map["Rating"], "★★★★");
        assert_eq!(map["Plays"], "3");
        assert!(map["ReplayGain"].contains("track"));
        assert!(map["File"].ends_with("Xtal.flac"));
    }

    #[test]
    fn episode_fields_mark_streaming_and_skip_empties() {
        let ep = Episode {
            id: 5,
            show_id: 9,
            guid: "g".into(),
            title: "180: ...".into(),
            description: Some("Notes here.".into()),
            pub_date: Some(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            duration: Some(5162),
            file_size: Some(73_400_320),
            audio_url: Some("https://cdn/ep.mp3".into()),
            audio_path: None,
            folder_path: "Podcasts/cortex/...".into(),
            mime_type: Some("audio/mpeg".into()),
            season: None,
            episode_number: Some(180),
            episode_type: Some("full".into()),
        };
        let rows = episode_fields(&np("180: ...", "Cortex", None), &ep, None, true);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Show"], "Cortex");
        assert_eq!(map["Published"], "2026-06-22");
        assert_eq!(map["Source"], "Streaming");
        assert_eq!(map["Episode"], "180");
        assert_eq!(map["Notes"], "Notes here.");
        // "full" type and absent season are skipped.
        assert!(!map.contains_key("Type"));
        assert!(!map.contains_key("Season/Episode"));
    }

    #[test]
    fn episode_fields_downloaded_label() {
        let ep = Episode {
            id: 6,
            show_id: 9,
            guid: "g".into(),
            title: "t".into(),
            description: None,
            pub_date: None,
            duration: None,
            file_size: None,
            audio_url: None,
            audio_path: Some("Podcasts/x/y.mp3".into()),
            folder_path: "Podcasts/x".into(),
            mime_type: None,
            season: Some(2),
            episode_number: Some(4),
            episode_type: Some("bonus".into()),
        };
        let rows = episode_fields(&np("t", "Show", None), &ep, None, false);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Source"], "Downloaded");
        assert_eq!(map["Season/Episode"], "S2 E4");
        assert_eq!(map["Type"], "bonus");
        assert!(!map.contains_key("Notes")); // empty description skipped
    }
}
