//! The persistent Now-bar transport (Phase 4b-ii-a, spec §3.6). A bottom bar on
//! the window's `ToolbarView`: what's playing on the left, the transport in the
//! centre, position + seek + volume on the right. It is a *sampled* display: the
//! window polls the `PlayerHandle` snapshot on a glib timeout and calls
//! [`NowBar::clear`]/the field setters; the buttons send straight to the engine.
//!
//! Symbolic icon-theme glyphs (no bundled-font assumption), matching the rating
//! stars. The seek slider drives playback via `change-value` (user drag only),
//! so the window's programmatic `set_value` during a refresh never loops back.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::MediaKind;
use conservatory_core::player::SleepMode;
use conservatory_core::{PlayerHandle, SleepStatus, quick_seek_target};

use crate::ui::accent::AccentProvider;

/// The Now-bar widgets the window updates each refresh. `root` is what gets
/// attached as the bottom bar.
pub struct NowBar {
    pub root: gtk::CenterBox,
    pub cover: gtk::Image,
    /// The frame around the cover (Phase 12c): carries the per-album accent ring.
    cover_frame: gtk::Frame,
    pub title: gtk::Label,
    pub artist: gtk::Label,
    /// Held so the accent ring + seek-fill rule can be swapped per item.
    accent: AccentProvider,
    /// The cover+title cluster, a clickable handle the window wires to toggle the
    /// Now Playing drawer (v0.0.38).
    pub left: gtk::Box,
    /// Spinning while a streamed item is buffering (v0.0.38).
    pub spinner: gtk::Spinner,
    /// Shown when the current item streams from the network (v0.0.38).
    pub streaming_icon: gtk::Image,
    pub play_btn: gtk::Button,
    /// Previous / next chapter (Phase 6c-iii-b): hidden unless the current item
    /// has chapters (`chapter_count > 0`), flanking the item prev/next.
    pub prev_chapter_btn: gtk::Button,
    pub next_chapter_btn: gtk::Button,
    /// Spoken-word quick-seek (16.5f): skip-back/forward label buttons flanking
    /// play/pause, shown only for episodes and audiobooks; the amounts follow
    /// the show's overrides via [`NowBar::set_quick_seek`].
    pub seek_back_btn: gtk::Button,
    pub seek_fwd_btn: gtk::Button,
    /// The `(back, forward)` seconds the quick-seek buttons currently apply.
    skip_amounts: Rc<Cell<(f64, f64)>>,
    /// Per-show playback (speed / Smart Speed / Voice Boost) for the playing
    /// podcast episode; sits by the transport, hidden unless an episode is playing.
    /// The window wires the click to a per-show settings dialog.
    pub podcast_btn: gtk::Button,
    pub position: gtk::Label,
    pub seek: gtk::Scale,
    pub volume: gtk::ScaleButton,
    /// Sleep-timer menu (Phase 6c-iii-d): a moon button whose menu arms a duration
    /// or a boundary timer. Hidden until something is loaded; `S` pops it.
    pub sleep_btn: gtk::MenuButton,
    /// The remaining-time label inside `sleep_btn`, shown for a duration timer.
    sleep_label: gtk::Label,
}

/// The duration presets the sleep menu offers, in minutes (Belfry §3.6).
const SLEEP_PRESETS_MIN: [u32; 4] = [15, 30, 45, 60];

/// The "end of current item" menu label, adapted to the playing media kind (the
/// user's media-agnostic scope decision: music gets a sleep timer too). Pure.
pub fn sleep_boundary_label(kind: Option<MediaKind>) -> &'static str {
    match kind {
        Some(MediaKind::Episode) => "End of episode",
        Some(MediaKind::Audiobook) => "End of book",
        _ => "End of track",
    }
}

/// A countdown clock string (`M:SS`), rounding remaining seconds up so a freshly
/// armed 15-minute timer reads `15:00`, not `14:59`. Pure.
pub fn fmt_sleep_remaining(secs: f64) -> String {
    let total = secs.max(0.0).ceil() as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// The placeholder shown when the album has no cover on disk.
const COVER_PLACEHOLDER: &str = "audio-x-generic-symbolic";

/// The Now-bar's secondary line (Phase 12c): `artist · album` when the album adds
/// information, else just the artist. Folding the duplicate keeps a podcast
/// (whose "artist" and "album" are both the show title) from reading "Show ·
/// Show". Pure.
pub fn now_bar_subtitle(artist: &str, album: Option<&str>) -> String {
    match album.map(str::trim).filter(|a| !a.is_empty()) {
        Some(album) if !album.eq_ignore_ascii_case(artist) && !artist.is_empty() => {
            format!("{artist} · {album}")
        }
        Some(album) if artist.is_empty() => album.to_string(),
        _ => artist.to_string(),
    }
}

/// Build the Now-bar. When a `player` is present, the transport controls are
/// wired to it; without one (no library / libmpv unavailable) the bar renders
/// inert.
pub fn build_now_bar(player: Option<PlayerHandle>) -> NowBar {
    let root = gtk::CenterBox::new();
    root.add_css_class("now-bar");

    // Left: cover thumbnail + title (bold) over artist (dim). The cover sits in a
    // frame so the per-album accent ring (Phase 12c) has a widget to tint without
    // clipping the image.
    let cover = gtk::Image::from_icon_name(COVER_PLACEHOLDER);
    cover.set_pixel_size(56);
    let cover_frame = gtk::Frame::builder()
        .valign(gtk::Align::Center)
        .css_classes(["now-bar-cover"])
        .child(&cover)
        .build();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["heading"])
        .label("Not playing")
        .build();
    let artist = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    let info = gtk::Box::new(gtk::Orientation::Vertical, 0);
    info.set_valign(gtk::Align::Center);
    info.set_width_request(220);
    info.append(&title);
    info.append(&artist);
    // Status cluster: a buffering spinner and a streaming glyph, both hidden
    // until the snapshot says otherwise (v0.0.38).
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    spinner.set_tooltip_text(Some("Buffering from the network\u{2026}"));
    let streaming_icon = gtk::Image::from_icon_name("network-wireless-symbolic");
    streaming_icon.set_visible(false);
    // "Streaming" alone read as a stall warning; say what it actually means
    // (playing from the network because there is no download) (16.5b).
    streaming_icon.set_tooltip_text(Some("Streaming (this episode is not downloaded)"));
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    status.set_valign(gtk::Align::Center);
    status.append(&spinner);
    status.append(&streaming_icon);
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    left.set_valign(gtk::Align::Center);
    left.append(&cover_frame);
    left.append(&info);
    left.append(&status);
    // The cluster is a click handle for the Now Playing drawer; show a pointer so
    // it reads as interactive (the window adds the GestureClick).
    left.set_cursor_from_name(Some("pointer"));
    left.set_tooltip_text(Some("Now Playing details"));
    root.set_start_widget(Some(&left));

    // Centre: [prev-chapter] prev / play-pause / next [next-chapter]. The chapter
    // buttons flank the item transport and stay hidden unless the current item has
    // chapters (Phase 6c-iii-b).
    let prev_chapter_btn = transport_button("media-seek-backward-symbolic", "Previous chapter");
    let prev_btn = transport_button("media-skip-backward-symbolic", "Previous");
    let play_btn = transport_button("media-playback-start-symbolic", "Play / Pause");
    let next_btn = transport_button("media-skip-forward-symbolic", "Next");
    let next_chapter_btn = transport_button("media-seek-forward-symbolic", "Next chapter");
    prev_chapter_btn.set_visible(false);
    next_chapter_btn.set_visible(false);
    // Per-show podcast playback affordance, by the transport; hidden for music /
    // books (the window toggles it for episodes and wires the dialog).
    let podcast_btn = transport_button(
        "preferences-other-symbolic",
        "Speed, Smart Speed & Voice Boost",
    );
    podcast_btn.set_visible(false);
    // Spoken-word quick-seek (16.5f), flanking play/pause the podcast-app way.
    // Label buttons: the media-seek-* icons already mean chapter skip here.
    let seek_back_btn = gtk::Button::with_label("\u{2212}15");
    let seek_fwd_btn = gtk::Button::with_label("+30");
    for b in [&seek_back_btn, &seek_fwd_btn] {
        b.add_css_class("flat");
        b.set_visible(false);
    }
    let skip_amounts: Rc<Cell<(f64, f64)>> = Rc::new(Cell::new((15.0, 30.0)));
    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    transport.set_valign(gtk::Align::Center);
    transport.append(&prev_chapter_btn);
    transport.append(&prev_btn);
    transport.append(&seek_back_btn);
    transport.append(&play_btn);
    transport.append(&seek_fwd_btn);
    transport.append(&next_btn);
    transport.append(&next_chapter_btn);
    transport.append(&podcast_btn);
    root.set_center_widget(Some(&transport));

    // Right: position label, seek slider, volume.
    let position = gtk::Label::builder()
        .css_classes(["numeric", "caption"])
        .label("0:00")
        .build();
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    seek.set_draw_value(false);
    seek.set_hexpand(true);
    seek.set_width_request(220);
    seek.set_sensitive(false);
    seek.add_css_class("now-bar-seek");
    // A ScaleButton with the audio icons (VolumeButton is deprecated since 4.10);
    // its value is the 0..100 volume directly.
    let volume = gtk::ScaleButton::new(
        0.0,
        100.0,
        5.0,
        &[
            "audio-volume-muted-symbolic",
            "audio-volume-high-symbolic",
            "audio-volume-low-symbolic",
            "audio-volume-medium-symbolic",
        ],
    );
    volume.set_value(100.0);
    // Sleep timer (Phase 6c-iii-d): a moon menu button, hidden until something is
    // loaded. The menu reads the snapshot off the handle, so this stays DB-free.
    let (sleep_btn, sleep_label) = build_sleep_button(player.clone());
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    right.set_valign(gtk::Align::Center);
    right.append(&position);
    right.append(&seek);
    right.append(&volume);
    right.append(&sleep_btn);
    root.set_end_widget(Some(&right));

    if let Some(player) = player {
        let p = player.clone();
        play_btn.connect_clicked(move |_| p.toggle_pause());
        let p = player.clone();
        prev_btn.connect_clicked(move |_| p.previous());
        let p = player.clone();
        next_btn.connect_clicked(move |_| p.next());
        let p = player.clone();
        prev_chapter_btn.connect_clicked(move |_| p.skip_chapter(-1));
        let p = player.clone();
        next_chapter_btn.connect_clicked(move |_| p.skip_chapter(1));
        let p = player.clone();
        let amounts = skip_amounts.clone();
        seek_back_btn.connect_clicked(move |_| {
            let snap = p.snapshot();
            let (back, _) = amounts.get();
            p.seek(quick_seek_target(snap.position, -back, snap.duration));
        });
        let p = player.clone();
        let amounts = skip_amounts.clone();
        seek_fwd_btn.connect_clicked(move |_| {
            let snap = p.snapshot();
            let (_, forward) = amounts.get();
            p.seek(quick_seek_target(snap.position, forward, snap.duration));
        });
        let p = player.clone();
        seek.connect_change_value(move |_, _, value| {
            p.seek(value);
            glib::Propagation::Proceed
        });
        let p = player.clone();
        volume.connect_value_changed(move |_, value| {
            p.set_volume(value.round() as i64);
        });
    }

    NowBar {
        root,
        cover,
        cover_frame,
        title,
        artist,
        accent: AccentProvider::new(),
        left,
        spinner,
        streaming_icon,
        play_btn,
        prev_chapter_btn,
        next_chapter_btn,
        seek_back_btn,
        seek_fwd_btn,
        skip_amounts,
        podcast_btn,
        position,
        seek,
        volume,
        sleep_btn,
        sleep_label,
    }
}

fn transport_button(icon: &str, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::from_icon_name(icon);
    btn.add_css_class("flat");
    btn.add_css_class("circular");
    btn.set_tooltip_text(Some(tooltip));
    btn
}

/// Build the sleep-timer menu button (Phase 6c-iii-d). The popover is rebuilt on
/// each open from the player snapshot (the `build_output_menu_button` idiom), so
/// the active mode is check-marked and the "end of item" row's label follows the
/// playing media kind. Returns the button and the remaining-time label it holds.
fn build_sleep_button(player: Option<PlayerHandle>) -> (gtk::MenuButton, gtk::Label) {
    let icon = gtk::Image::from_icon_name("weather-clear-night-symbolic");
    let label = gtk::Label::builder()
        .css_classes(["numeric", "caption"])
        .visible(false)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.append(&icon);
    content.append(&label);

    let btn = gtk::MenuButton::new();
    btn.add_css_class("flat");
    btn.set_child(Some(&content));
    btn.set_tooltip_text(Some("Sleep timer (S)"));
    btn.set_visible(false);

    btn.set_create_popup_func(move |btn| {
        let popover = gtk::Popover::new();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.set_margin_top(4);
        list.set_margin_bottom(4);

        let snap = player.as_ref().map(|p| p.snapshot());
        let current = snap.as_ref().and_then(|s| s.sleep.map(|sl| sl.mode));
        let kind = snap.as_ref().and_then(|s| s.kind);

        // Off, the duration presets, end-of-item (kind-adapted label), end-of-queue.
        let mut rows: Vec<(String, Option<SleepMode>)> = vec![("Off".to_string(), None)];
        for &m in &SLEEP_PRESETS_MIN {
            rows.push((
                format!("{m} minutes"),
                Some(SleepMode::After(f64::from(m) * 60.0)),
            ));
        }
        rows.push((
            sleep_boundary_label(kind).to_string(),
            Some(SleepMode::EndOfItem),
        ));
        rows.push(("End of queue".to_string(), Some(SleepMode::EndOfQueue)));

        for (text, mode) in rows {
            let selected = mode == current;
            let row = gtk::Button::new();
            row.add_css_class("flat");
            row.add_css_class("sleep-menu-row");
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_visible(selected);
            let lbl = gtk::Label::new(Some(&text));
            lbl.set_xalign(0.0);
            lbl.set_hexpand(true);
            row_box.append(&check);
            row_box.append(&lbl);
            row.set_child(Some(&row_box));

            let player = player.clone();
            let pop_weak = popover.downgrade();
            row.connect_clicked(move |_| {
                if let Some(p) = player.as_ref() {
                    p.set_sleep_timer(mode);
                }
                if let Some(pop) = pop_weak.upgrade() {
                    pop.popdown();
                }
            });
            list.append(&row);
        }

        popover.set_child(Some(&list));
        btn.set_popover(Some(&popover));
    });

    (btn, label)
}

impl NowBar {
    /// Reset to the idle "nothing playing" state.
    pub fn clear(&self) {
        self.title.set_text("Not playing");
        self.artist.set_text("");
        self.position.set_text("0:00");
        self.play_btn.set_icon_name("media-playback-start-symbolic");
        self.seek.set_sensitive(false);
        self.seek.set_value(0.0);
        self.set_cover(None, None);
        self.set_status(false, false);
        self.set_chapter_nav_visible(false);
        self.set_quick_seek(false, 15.0, 30.0);
        self.sleep_btn.set_visible(false);
        self.set_sleep(None);
    }

    /// Show/hide the spoken-word quick-seek pair and set its amounts (16.5f);
    /// the labels and tooltips follow the resolved seconds.
    pub fn set_quick_seek(&self, visible: bool, back: f64, forward: f64) {
        self.skip_amounts.set((back, forward));
        self.seek_back_btn.set_visible(visible);
        self.seek_fwd_btn.set_visible(visible);
        if visible {
            let (b, f) = (back.round() as i64, forward.round() as i64);
            self.seek_back_btn.set_label(&format!("\u{2212}{b}"));
            self.seek_fwd_btn.set_label(&format!("+{f}"));
            self.seek_back_btn
                .set_tooltip_text(Some(&format!("Back {b} seconds")));
            self.seek_fwd_btn
                .set_tooltip_text(Some(&format!("Forward {f} seconds")));
        }
    }

    /// Reflect the armed sleep timer (Phase 6c-iii-d): an active duration timer
    /// shows its `M:SS` remaining beside an accent-tinted moon; a boundary timer
    /// just tints the moon; no timer clears both. (The button's own visibility is
    /// the window's call, keyed on whether anything is loaded.)
    pub fn set_sleep(&self, status: Option<SleepStatus>) {
        match status {
            Some(s) => {
                self.sleep_btn.add_css_class("accent");
                let text = s.remaining.map(fmt_sleep_remaining).unwrap_or_default();
                self.sleep_label.set_text(&text);
                self.sleep_label.set_visible(!text.is_empty());
            }
            None => {
                self.sleep_btn.remove_css_class("accent");
                self.sleep_label.set_text("");
                self.sleep_label.set_visible(false);
            }
        }
    }

    /// Show or hide the chapter-skip buttons (Phase 6c-iii-b): visible only when
    /// the current item has chapters.
    pub fn set_chapter_nav_visible(&self, visible: bool) {
        self.prev_chapter_btn.set_visible(visible);
        self.next_chapter_btn.set_visible(visible);
    }

    /// Show/hide the buffering spinner and the streaming glyph (v0.0.38).
    pub fn set_status(&self, buffering: bool, streaming: bool) {
        self.spinner.set_visible(buffering);
        if buffering {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
        self.streaming_icon.set_visible(streaming);
    }

    /// Show the album cover from `path` (or the placeholder when absent) and tint
    /// the cover frame's ring + the seek fill with the item's `accent` (Phase
    /// 12c). Both go through one swapped display-wide rule.
    pub fn set_cover(&self, path: Option<&std::path::Path>, accent: Option<u32>) {
        match path.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some(COVER_PLACEHOLDER)),
        }
        self.apply_accent(accent);
    }

    /// Tint the cover ring and the seek fill with the album accent. The ring rule
    /// is the shared cover-ring CSS; the seek rule colours the slider's filled
    /// trough. `None` clears both back to the Dragon defaults.
    fn apply_accent(&self, accent: Option<u32>) {
        match accent {
            Some(rgb) => {
                let hex = rgb & 0x00ff_ffff;
                let css = format!(
                    "{}\n.now-bar-seek.cover-acc-{hex:06x} > trough > highlight \
                     {{ background-color: #{hex:06x}; }}",
                    crate::ui::accent::cover_ring_css(rgb)
                );
                self.accent.set_css(&css);
                let cls = crate::ui::accent::accent_class(rgb);
                self.cover_frame.set_css_classes(&["now-bar-cover", &cls]);
                self.seek.set_css_classes(&["now-bar-seek", &cls]);
            }
            None => {
                self.accent.set_css("");
                self.cover_frame.set_css_classes(&["now-bar-cover"]);
                self.seek.set_css_classes(&["now-bar-seek"]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_rounds_up_to_clock() {
        assert_eq!(fmt_sleep_remaining(900.0), "15:00");
        assert_eq!(fmt_sleep_remaining(59.2), "1:00"); // rounds up, never 0:59
        assert_eq!(fmt_sleep_remaining(61.0), "1:01");
        assert_eq!(fmt_sleep_remaining(0.0), "0:00");
        assert_eq!(fmt_sleep_remaining(-5.0), "0:00"); // clamped
    }

    #[test]
    fn subtitle_folds_redundant_album() {
        // Music: artist and album both inform.
        assert_eq!(
            now_bar_subtitle("Aphex Twin", Some("SAW 85-92")),
            "Aphex Twin · SAW 85-92"
        );
        // Podcast: artist == album (the show title), so the album is folded out.
        assert_eq!(now_bar_subtitle("Cortex", Some("Cortex")), "Cortex");
        // No album: just the artist.
        assert_eq!(now_bar_subtitle("Aesop Rock", None), "Aesop Rock");
        assert_eq!(now_bar_subtitle("Aesop Rock", Some("  ")), "Aesop Rock");
        // No artist (rare): the album carries the line.
        assert_eq!(now_bar_subtitle("", Some("Some Album")), "Some Album");
    }

    #[test]
    fn boundary_label_follows_kind() {
        assert_eq!(
            sleep_boundary_label(Some(MediaKind::Episode)),
            "End of episode"
        );
        assert_eq!(
            sleep_boundary_label(Some(MediaKind::Audiobook)),
            "End of book"
        );
        assert_eq!(sleep_boundary_label(Some(MediaKind::Track)), "End of track");
        assert_eq!(sleep_boundary_label(None), "End of track");
    }
}
