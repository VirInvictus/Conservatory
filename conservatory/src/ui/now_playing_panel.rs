//! The Now Playing drawer: a bottom slide-up `gtk::Revealer` whose hero is the
//! full-bleed spectrum visualizer (Phase 12d) with a minimal cover + title +
//! artist chip overlaid at the bottom. Clicking the Now-bar cover/title (or
//! Ctrl+I) reveals it; it updates live as the queue advances.
//!
//! The redundant second seekbar and the metadata grid were retired in the
//! full-bleed rebuild: the Now-bar already carries the transport, so the drawer's
//! job is the visual plus what is playing. Spoken-word extras (the Smart Speed /
//! sleep lines and the clickable chapter list) survive because they are
//! functional, not decorative.
//!
//! The window resolves the display title/subtitle/cover from the DB and hands
//! them in, so this module builds no DB reads.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use std::path::Path;

use conservatory_core::db::{Chapter, MediaKind};
use conservatory_core::player::SleepMode;
use conservatory_core::{PlayerHandle, SleepStatus};

use crate::playqueue::fmt_secs;
use crate::ui::now_bar::{fmt_sleep_remaining, sleep_boundary_label};

/// The drawer: the revealer to place, plus the live widgets it fills.
pub struct NowPlayingPanel {
    pub revealer: gtk::Revealer,
    title: gtk::Label,
    /// The `artist · album` (track) / show (episode) / author (book) line.
    subtitle: gtk::Label,
    /// The small cover in the overlaid chip, in an accent-tinted frame.
    cover: gtk::Image,
    cover_frame: gtk::Frame,
    /// The shared accent provider tinting the cover frame, swapped per item.
    accent: crate::ui::accent::AccentProvider,
    /// The "Smart Speed · saved m:ss" line; hidden unless the current item has
    /// Smart Speed on. Updated each poll tick from the snapshot.
    smart_speed: gtk::Label,
    /// The "Sleep · …" line; hidden unless a sleep timer is armed.
    sleep: gtk::Label,
    /// Heading + list wrapper, hidden when the item has no chapters.
    chapters_box: gtk::Box,
    chapters_list: gtk::ListBox,
    /// Per-row chapter start seconds, indexed by row position; the row-activated
    /// handler (wired once at build) reads this to seek. Shared so a single
    /// handler survives list rebuilds (re-connecting would double-fire).
    chapter_starts: Rc<RefCell<Vec<f64>>>,
    /// The spectrum visualizer (Phase 12d): the full-bleed hero, captures the
    /// system audio and draws accent-tinted bars while the drawer is open.
    spectrum: crate::ui::spectrum::Spectrum,
    /// Swaps the populated content for a centered "nothing playing" StatusPage
    /// when idle (Phase 13b).
    stack: gtk::Stack,
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
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-3"])
        .label("Now Playing")
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label"])
        .build();

    // Episode extras: a Smart Speed line and a sleep line, both hidden until the
    // current item calls for them. They sit in the overlaid chip with the title.
    let smart_speed = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["accent", "caption"])
        .margin_top(2)
        .visible(false)
        .build();
    let sleep = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["accent", "caption"])
        .margin_top(2)
        .visible(false)
        .build();

    // The minimal cover (the larger twin of the Now-bar thumbnail), in an
    // accent-tinted frame; it leads the overlaid chip.
    let cover = gtk::Image::builder()
        .pixel_size(72)
        .icon_name("audio-x-generic-symbolic")
        .build();
    let cover_frame = gtk::Frame::builder()
        .css_classes(["now-playing-cover"])
        .child(&cover)
        .valign(gtk::Align::Center)
        .build();

    // The clickable chapter list (6c-iii-c) for spoken word, hidden otherwise.
    let chapters_heading = gtk::Label::builder()
        .label("Chapters")
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    let chapters_list = gtk::ListBox::new();
    chapters_list.set_selection_mode(gtk::SelectionMode::None);
    chapters_list.add_css_class("chapter-list");
    let chapters_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .max_content_height(160)
        .propagate_natural_height(true)
        .child(&chapters_list)
        .build();
    let chapters_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    chapters_box.set_margin_start(16);
    chapters_box.set_margin_end(16);
    chapters_box.set_margin_bottom(12);
    chapters_box.append(&chapters_heading);
    chapters_box.append(&chapters_scroller);
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

    // The overlaid chip: cover left of the title / subtitle / spoken-word lines,
    // sitting at the bottom of the spectrum behind a legibility scrim.
    let text_col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text_col.set_valign(gtk::Align::Center);
    text_col.append(&title);
    text_col.append(&subtitle);
    text_col.append(&smart_speed);
    text_col.append(&sleep);
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    chip.add_css_class("now-playing-info");
    chip.set_halign(gtk::Align::Start);
    chip.set_valign(gtk::Align::End);
    chip.set_margin_start(16);
    chip.set_margin_end(16);
    chip.set_margin_bottom(14);
    chip.append(&cover_frame);
    chip.append(&text_col);

    // The full-bleed hero: the spectrum fills the overlay, the chip floats over it.
    let spectrum = crate::ui::spectrum::build_spectrum();
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&spectrum.area));
    overlay.add_overlay(&chip);
    overlay.set_height_request(220);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("background");
    column.add_css_class("now-playing-drawer");
    column.append(&overlay);
    column.append(&chapters_box);

    // The idle state (Phase 13b; owned composite since Phase 26): a centered
    // status page shown in place of the populated column when nothing is
    // playing. `clear()` swaps to it; any populate call swaps back. (While it
    // shows, the spectrum's `area` is unmapped, so the capture is idle too.)
    let idle_page = crate::ui::status_page::status_page(
        Some("audio-x-generic-symbolic"),
        "Nothing playing",
        Some("Play something to see it here."),
    );
    let stack = gtk::Stack::new();
    stack.add_named(&column, Some("content"));
    stack.add_named(idle_page.widget(), Some("empty"));
    stack.set_visible_child_name("empty");

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(250)
        .reveal_child(false)
        .child(&stack)
        .build();
    // The spectrum area inside is `vexpand`, which would otherwise propagate up
    // through the revealer; in the vertical content box a collapsed-but-expanding
    // revealer steals a share of the height and leaves a dead gap. A revealer must
    // size to its child (0 when closed), never expand: pin it off explicitly.
    revealer.set_vexpand(false);

    NowPlayingPanel {
        revealer,
        title,
        subtitle,
        cover,
        cover_frame,
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

    /// Drive the spectrum's capture gate from the engine's play-state: the tap runs
    /// only while Conservatory is actually playing (so it taps our own output node,
    /// never the microphone). Cheap to call every poll tick.
    pub fn set_playing(&self, playing: bool) {
        self.spectrum.set_playing(playing);
    }

    /// Set what is playing: the title heads the chip, `subtitle` is the
    /// `artist · album` (or show / author) line. Shows the populated content.
    pub fn set_now_playing(&self, title: &str, subtitle: &str) {
        self.title.set_text(title);
        self.subtitle.set_text(subtitle);
        self.subtitle.set_visible(!subtitle.is_empty());
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

    /// Load the chip cover and tint its frame + the spectrum bars with the item's
    /// accent. A missing cover falls back to a placeholder icon. Called on item
    /// change.
    pub fn set_cover(&self, cover_abs: Option<&Path>, accent: Option<u32>) {
        match cover_abs.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some("audio-x-generic-symbolic")),
        }
        self.accent
            .apply_cover_ring(&self.cover_frame, &["now-playing-cover"], accent);
        self.spectrum.set_accent(accent);
    }

    /// The idle "nothing playing" state.
    pub fn clear(&self) {
        self.set_now_playing("Now Playing", "");
        self.set_cover(None, None);
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
}
