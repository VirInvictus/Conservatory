//! The Now Playing drawer (v0.0.38): a bottom slide-up `gtk::Revealer`, the
//! horizontal twin of the right-docked queue drawer. Clicking the Now-bar
//! cover/title (or Ctrl+I) reveals the current item's full metadata; it updates
//! as the queue advances, with the spectrum visualizer (Phase 12d) alongside it.
//!
//! The window resolves the display rows from the DB (the pure projections live
//! in `ui/fields.rs`) and hands them to `set_fields`, so this module builds no
//! DB reads; what is left here is the live drawer widget and its pure helpers
//! (the audio-engine line, the up-next peek, the sleep line).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use std::path::Path;

use conservatory_core::db::{Chapter, DspState, EqState, MediaKind, QueueDisplayRow};
use conservatory_core::player::SleepMode;
use conservatory_core::{PlayerHandle, SleepStatus};

use crate::playqueue::{fmt_position, fmt_secs};
use crate::ui::now_bar::{fmt_sleep_remaining, sleep_boundary_label};

/// The drawer: the revealer to place, plus the labelled grid it fills and the
/// episode extras (Smart Speed line + clickable chapter list, Phase 6c-iii-c).
pub struct NowPlayingPanel {
    pub revealer: gtk::Revealer,
    title: gtk::Label,
    grid: gtk::Grid,
    /// The full-bleed cover (Phase 11c), in an accent-tinted frame; the larger
    /// twin of the Now-bar thumbnail (spec §3.6, the Hermitage Codex moment).
    cover: gtk::Image,
    cover_frame: gtk::Frame,
    /// The accent-tinted scrubber + its `position / duration` label. Seeks
    /// through the shared `player` handle on drag; updated per tick when open.
    scrubber: gtk::Scale,
    scrub_label: gtk::Label,
    /// The audio-engine state line (Phase 11c): EQ preset / DSP modules / gapless
    /// for a playing track; hidden for episodes / books.
    audio_state: gtk::Label,
    /// The "Up next" queue-tail peek: a heading + a short list of the next items.
    upnext_box: gtk::Box,
    upnext_list: gtk::Box,
    /// The shared accent provider tinting the cover frame + scrubber, swapped per
    /// item (Phase 13c: routed through `ui/accent.rs` instead of an inline copy of
    /// the provider-swap dance).
    accent: crate::ui::accent::AccentProvider,
    /// The "Smart Speed · saved m:ss" line; hidden unless the current item has
    /// Smart Speed on. Updated each poll tick from the snapshot.
    smart_speed: gtk::Label,
    /// The "Sleep · …" line; hidden unless a sleep timer is armed (Phase
    /// 6c-iii-d). Updated each poll tick from the snapshot.
    sleep: gtk::Label,
    /// Heading + list wrapper, hidden when the item has no chapters.
    chapters_box: gtk::Box,
    chapters_list: gtk::ListBox,
    /// Per-row chapter start seconds, indexed by row position; the row-activated
    /// handler (wired once at build) reads this to seek. Shared so a single
    /// handler survives list rebuilds (re-connecting would double-fire).
    chapter_starts: Rc<RefCell<Vec<f64>>>,
    /// The handle the chapter rows seek through; set on each `set_chapters`.
    /// The spectrum visualizer (Phase 12d): captures the system audio and draws
    /// accent-tinted frequency bars while the drawer is open.
    spectrum: crate::ui::spectrum::Spectrum,
    /// Swaps the populated content for a centered "nothing playing" StatusPage
    /// when idle (Phase 13b).
    stack: gtk::Stack,
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
        .row_spacing(8)
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
    let sleep = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["accent", "caption"])
        .margin_top(2)
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
    let chapters_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    chapters_box.append(&chapters_heading);
    chapters_box.append(&chapters_list);
    chapters_box.set_visible(false);

    // The full-bleed cover (Phase 11c) and the accent-tinted scrubber, the spec
    // §3.6 "Codex moment" furniture. The cover sits left of the title; the
    // scrubber + its time label sit under the title.
    let cover = gtk::Image::builder()
        .pixel_size(160)
        .icon_name("audio-x-generic-symbolic")
        .build();
    let cover_frame = gtk::Frame::builder()
        .css_classes(["now-playing-cover"])
        .child(&cover)
        .build();
    cover_frame.set_valign(gtk::Align::Start);

    let scrubber = gtk::Scale::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .draw_value(false)
        .css_classes(["now-playing-scrubber"])
        .build();
    scrubber.set_range(0.0, 1.0);
    scrubber.set_sensitive(false);
    let scrub_label = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();

    let audio_state = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .margin_top(2)
        .visible(false)
        .build();

    let chapter_starts = Rc::new(RefCell::new(Vec::<f64>::new()));
    let player = Rc::new(RefCell::new(None::<PlayerHandle>));
    // The scrubber seeks through the shared handle (the now-bar idiom: the
    // change-value signal fires on user drag, so the per-tick programmatic
    // `set_value` never loops back into a seek).
    scrubber.connect_change_value({
        let player = player.clone();
        move |_, _, value| {
            if let Some(p) = player.borrow().as_ref() {
                p.seek(value);
            }
            gtk::glib::Propagation::Proceed
        }
    });
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

    // "Up next" queue-tail peek (Phase 11c): a heading + a short list of the
    // upcoming items, hidden when the playing item is the last in the queue.
    let upnext_heading = gtk::Label::builder()
        .label("Up next")
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_top(8)
        .build();
    let upnext_list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let upnext_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    upnext_box.append(&upnext_heading);
    upnext_box.append(&upnext_list);
    upnext_box.set_visible(false);

    // The header row: the full-bleed cover left of the title + scrubber + the
    // audio-engine line.
    let header_text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    header_text.set_hexpand(true);
    header_text.set_valign(gtk::Align::Center);
    header_text.append(&title);
    header_text.append(&scrubber);
    header_text.append(&scrub_label);
    header_text.append(&audio_state);
    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header_row.append(&cover_frame);
    header_row.append(&header_text);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column.add_css_class("background");
    column.add_css_class("now-playing-drawer");
    column.set_margin_top(14);
    column.set_margin_bottom(14);
    column.set_margin_start(16);
    column.set_margin_end(16);
    let spectrum = crate::ui::spectrum::build_spectrum();
    column.append(&header_row);
    column.append(&spectrum.area);
    column.append(&grid);
    column.append(&smart_speed);
    column.append(&sleep);
    column.append(&chapters_box);
    column.append(&upnext_box);

    // The idle state (Phase 13b): a centered StatusPage shown in place of the
    // populated column when nothing is playing. `clear()` swaps to it; any
    // `set_fields` swaps back. (While it shows, the spectrum's `area` is unmapped,
    // so the capture is idle too.)
    let idle_page = adw::StatusPage::builder()
        .icon_name("audio-x-generic-symbolic")
        .title("Nothing playing")
        .description("Play something to see it here.")
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&column, Some("content"));
    stack.add_named(&idle_page, Some("empty"));
    stack.set_visible_child_name("empty");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(150)
        .max_content_height(280)
        .propagate_natural_height(true)
        .child(&stack)
        .build();

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(250)
        .reveal_child(false)
        .child(&scroller)
        .build();

    NowPlayingPanel {
        revealer,
        title,
        grid,
        cover,
        cover_frame,
        scrubber,
        scrub_label,
        audio_state,
        upnext_box,
        upnext_list,
        accent: crate::ui::accent::AccentProvider::new(),
        smart_speed,
        sleep,
        chapters_box,
        chapters_list,
        chapter_starts,
        spectrum,
        stack,
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
            if crate::ui::fields::is_tech_field(label) {
                val.add_css_class("tech");
            }
            self.grid.attach(&key, 0, row as i32, 1, 1);
            self.grid.attach(&val, 1, row as i32, 1, 1);
        }
        // Real content present: show the populated column (clear() flips back to
        // the idle StatusPage after its own set_fields call).
        self.stack.set_visible_child_name("content");
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

    /// Show / update the sleep-timer line (Phase 6c-iii-d). Hidden when no timer is
    /// armed; otherwise it reads the remaining time or the chosen boundary, and
    /// invites a tap-to-extend once a duration timer has fired.
    pub fn set_sleep(&self, status: Option<SleepStatus>, kind: Option<MediaKind>) {
        match sleep_drawer_text(status, kind) {
            Some(text) => {
                self.sleep.set_text(&text);
                self.sleep.set_visible(true);
            }
            None => self.sleep.set_visible(false),
        }
    }

    /// Load the full-bleed cover (Phase 11c) and tint its frame + the scrubber
    /// with the item's accent. A missing cover falls back to a placeholder icon.
    /// Called on item change.
    pub fn set_cover(&self, cover_abs: Option<&Path>, accent: Option<u32>) {
        match cover_abs.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some("audio-x-generic-symbolic")),
        }
        self.apply_accent(accent);
        self.spectrum.set_accent(accent);
    }

    /// Tint the cover frame and the scrubber highlight with the item accent. The
    /// shared `AccentProvider` owns the display-wide provider swap; the rule here
    /// is panel-specific (a cover ring *and* the scrubber fill under one class).
    fn apply_accent(&self, accent: Option<u32>) {
        match accent {
            Some(rgb) => {
                let hex = rgb & 0x00ff_ffff;
                let css = format!(
                    ".now-playing-cover.np-acc-{hex:06x} {{ box-shadow: 0 0 0 2px #{hex:06x}; }}\n\
                     .np-acc-{hex:06x} > trough > highlight {{ background-color: #{hex:06x}; }}"
                );
                self.accent.set_css(&css);
                self.cover_frame
                    .set_css_classes(&["now-playing-cover", &format!("np-acc-{hex:06x}")]);
                self.scrubber
                    .set_css_classes(&["now-playing-scrubber", &format!("np-acc-{hex:06x}")]);
            }
            None => {
                self.accent.set_css("");
                self.cover_frame.set_css_classes(&["now-playing-cover"]);
                self.scrubber.set_css_classes(&["now-playing-scrubber"]);
            }
        }
    }

    /// Update the scrubber position + its `m:ss / m:ss` label (Phase 11c). Cheap
    /// per tick; disabled (and blank) when the duration is unknown.
    pub fn set_scrubber(&self, position: f64, duration: Option<f64>) {
        match duration {
            Some(d) if d > 0.0 => {
                self.scrubber.set_sensitive(true);
                self.scrubber.set_range(0.0, d);
                self.scrubber.set_value(position.min(d));
                self.scrub_label.set_text(&fmt_position(position, duration));
            }
            _ => {
                self.scrubber.set_sensitive(false);
                self.scrub_label.set_text("");
            }
        }
    }

    /// Show / update the audio-engine state line (Phase 11c) for a track; `None`
    /// (an episode / book) hides it.
    pub fn set_audio_state(&self, line: Option<&str>) {
        match line {
            Some(text) => {
                self.audio_state.set_text(text);
                self.audio_state.set_visible(true);
            }
            None => self.audio_state.set_visible(false),
        }
    }

    /// Rebuild the "Up next" peek (Phase 11c) from the upcoming queue rows; an
    /// empty slice hides the section (the playing item is last).
    pub fn set_upnext(&self, rows: &[UpNextRow]) {
        while let Some(child) = self.upnext_list.first_child() {
            self.upnext_list.remove(&child);
        }
        if rows.is_empty() {
            self.upnext_box.set_visible(false);
            return;
        }
        for r in rows {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let icon = gtk::Image::from_icon_name(r.icon);
            icon.add_css_class("dim-label");
            let label = gtk::Label::builder()
                .label(&r.text)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption"])
                .build();
            row.append(&icon);
            row.append(&label);
            self.upnext_list.append(&row);
        }
        self.upnext_box.set_visible(true);
    }

    /// The idle "nothing playing" state.
    pub fn clear(&self) {
        self.set_fields("Now Playing", &[("".into(), "Nothing playing.".into())]);
        self.set_cover(None, None);
        self.set_scrubber(0.0, None);
        self.set_audio_state(None);
        self.set_upnext(&[]);
        self.smart_speed.set_visible(false);
        self.sleep.set_visible(false);
        self.chapters_box.set_visible(false);
        self.current_chapter.set(None);
        while let Some(child) = self.chapters_list.first_child() {
            self.chapters_list.remove(&child);
        }
        self.stack.set_visible_child_name("empty");
    }
}

/// One "Up next" peek entry (Phase 11c): a kind icon and a `Title — Artist` line.
pub struct UpNextRow {
    pub icon: &'static str,
    pub text: String,
}

/// The "Sleep · …" drawer line for an armed timer, or `None` when none is set
/// (Phase 6c-iii-d). A duration timer shows its remaining `M:SS` (or "tap play to
/// extend" while the tap-to-extend window is open); a boundary timer names where
/// it stops, the label following the playing media kind. Pure.
pub fn sleep_drawer_text(status: Option<SleepStatus>, kind: Option<MediaKind>) -> Option<String> {
    let s = status?;
    let body = if s.fired {
        "tap play to extend".to_string()
    } else {
        match s.remaining {
            Some(r) => fmt_sleep_remaining(r),
            None => match s.mode {
                SleepMode::EndOfQueue => "until end of queue".to_string(),
                // EndOfItem (the After case never reaches here: it has `remaining`).
                _ => format!("until {}", sleep_boundary_label(kind).to_lowercase()),
            },
        }
    };
    Some(format!("Sleep · {body}"))
}

/// The audio-engine state line for a playing track (Phase 11c): the active EQ
/// preset (or "Custom" for a non-preset non-flat curve), the enabled DSP modules,
/// and the gapless state. EQ / DSP segments are omitted when inactive; gapless is
/// always reported. Pure.
pub fn audio_state_line(eq: &EqState, dsp: &DspState, gapless: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    // EQ is "active" when it shapes the sound: a non-flat preset or any non-zero
    // band. A "Flat" / unset preset with zero bands is a no-op, so it is omitted.
    let named_preset = eq
        .preset
        .as_deref()
        .filter(|p| !p.is_empty() && *p != "Flat");
    let eq_active = named_preset.is_some() || eq.bands.iter().any(|b| *b != 0.0);
    if eq_active {
        let name = named_preset.map_or_else(|| "Custom".to_string(), str::to_string);
        parts.push(format!("EQ · {name}"));
    }
    let mut mods: Vec<&str> = Vec::new();
    if dsp.comp.enabled {
        mods.push("Compressor");
    }
    if dsp.limiter.enabled {
        mods.push("Limiter");
    }
    if dsp.leveler.enabled {
        mods.push("Leveler");
    }
    if !mods.is_empty() {
        parts.push(format!("DSP · {}", mods.join(", ")));
    }
    parts.push(format!("Gapless · {}", if gapless { "on" } else { "off" }));
    parts.join("    ")
}

/// The upcoming queue rows after the current item (Phase 11c queue-tail peek): up
/// to `n` rows past `current`. Empty when the current item is last. Pure.
pub fn upcoming(rows: &[QueueDisplayRow], current: Option<usize>, n: usize) -> &[QueueDisplayRow] {
    let start = current.map_or(0, |c| c + 1);
    if start >= rows.len() {
        return &[];
    }
    let end = (start + n).min(rows.len());
    &rows[start..end]
}

/// The symbolic icon for a queue row's media kind (the "Up next" peek + the queue
/// drawer share this vocabulary).
pub fn kind_icon(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Track => "audio-x-generic-symbolic",
        MediaKind::Episode => "audio-speakers-symbolic",
        MediaKind::Audiobook => "book-open-variant-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(mode: SleepMode, remaining: Option<f64>, fired: bool) -> SleepStatus {
        SleepStatus {
            mode,
            remaining,
            fired,
        }
    }

    #[test]
    fn sleep_drawer_text_cases() {
        // No timer: no line.
        assert_eq!(sleep_drawer_text(None, None), None);

        // A duration timer shows its remaining clock.
        assert_eq!(
            sleep_drawer_text(
                Some(status(SleepMode::After(900.0), Some(125.0), false)),
                Some(MediaKind::Episode),
            ),
            Some("Sleep · 2:05".to_string()),
        );

        // A fired timer invites the tap-to-extend, regardless of kind.
        assert_eq!(
            sleep_drawer_text(Some(status(SleepMode::After(900.0), Some(0.0), true)), None,),
            Some("Sleep · tap play to extend".to_string()),
        );

        // The boundary modes name where they stop, the label following the kind.
        assert_eq!(
            sleep_drawer_text(
                Some(status(SleepMode::EndOfItem, None, false)),
                Some(MediaKind::Episode),
            ),
            Some("Sleep · until end of episode".to_string()),
        );
        assert_eq!(
            sleep_drawer_text(
                Some(status(SleepMode::EndOfItem, None, false)),
                Some(MediaKind::Track),
            ),
            Some("Sleep · until end of track".to_string()),
        );
        assert_eq!(
            sleep_drawer_text(
                Some(status(SleepMode::EndOfQueue, None, false)),
                Some(MediaKind::Episode),
            ),
            Some("Sleep · until end of queue".to_string()),
        );
    }

    fn qrow(position: i64, title: &str) -> QueueDisplayRow {
        QueueDisplayRow {
            position,
            kind: MediaKind::Track,
            track_id: Some(position),
            episode_id: None,
            book_id: None,
            show_id: None,
            title: title.into(),
            artist: None,
            audio_path: None,
            audio_url: None,
        }
    }

    #[test]
    fn upcoming_takes_next_n_after_current() {
        let rows = [qrow(0, "A"), qrow(1, "B"), qrow(2, "C"), qrow(3, "D")];
        // The two items after index 0.
        let next = upcoming(&rows, Some(0), 2);
        assert_eq!(
            next.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["B", "C"]
        );
        // The last item has no tail.
        assert!(upcoming(&rows, Some(3), 4).is_empty());
        // No current cursor peeks from the head.
        assert_eq!(upcoming(&rows, None, 1).len(), 1);
        assert_eq!(upcoming(&rows, None, 1)[0].title, "A");
        // An empty queue is always empty.
        assert!(upcoming(&[], Some(0), 4).is_empty());
    }

    #[test]
    fn audio_state_line_segments() {
        // A flat EQ + everything off: only the gapless segment.
        assert_eq!(
            audio_state_line(&EqState::flat(), &DspState::default(), true),
            "Gapless · on"
        );
        // A named preset + two DSP modules + gapless off.
        let eq = EqState {
            bands: [0.0; conservatory_core::db::EQ_BAND_COUNT],
            preset: Some("Rock".into()),
        };
        let mut dsp = DspState::default();
        dsp.comp.enabled = true;
        dsp.limiter.enabled = true;
        assert_eq!(
            audio_state_line(&eq, &dsp, false),
            "EQ · Rock    DSP · Compressor, Limiter    Gapless · off"
        );
        // Non-zero bands with no named preset read as "Custom".
        let mut custom = EqState::flat();
        custom.bands[3] = 4.0;
        custom.preset = None;
        assert!(audio_state_line(&custom, &DspState::default(), true).starts_with("EQ · Custom"));
    }
}
