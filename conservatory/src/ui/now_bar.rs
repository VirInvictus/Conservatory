//! The persistent Now-bar transport (Phase 4b-ii-a, spec §3.6). A bottom bar on
//! the window's `ToolbarView`: what's playing on the left, the transport in the
//! centre, position + seek + volume on the right. It is a *sampled* display: the
//! window polls the `PlayerHandle` snapshot on a glib timeout and calls
//! [`NowBar::clear`]/the field setters; the buttons send straight to the engine.
//!
//! Symbolic icon-theme glyphs (no bundled-font assumption), matching the rating
//! stars. The seek slider drives playback via `change-value` (user drag only),
//! so the window's programmatic `set_value` during a refresh never loops back.

use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::PlayerHandle;

/// The Now-bar widgets the window updates each refresh. `root` is what gets
/// attached as the bottom bar.
pub struct NowBar {
    pub root: gtk::CenterBox,
    pub cover: gtk::Image,
    pub title: gtk::Label,
    pub artist: gtk::Label,
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
    pub position: gtk::Label,
    pub seek: gtk::Scale,
    pub volume: gtk::ScaleButton,
}

/// The placeholder shown when the album has no cover on disk.
const COVER_PLACEHOLDER: &str = "audio-x-generic-symbolic";

/// Build the Now-bar. When a `player` is present, the transport controls are
/// wired to it; without one (no library / libmpv unavailable) the bar renders
/// inert.
pub fn build_now_bar(player: Option<PlayerHandle>) -> NowBar {
    let root = gtk::CenterBox::new();
    root.add_css_class("now-bar");

    // Left: cover thumbnail + title (bold) over artist (dim).
    let cover = gtk::Image::from_icon_name(COVER_PLACEHOLDER);
    cover.set_pixel_size(40);
    cover.add_css_class("now-bar-cover");
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
    spinner.set_tooltip_text(Some("Buffering"));
    let streaming_icon = gtk::Image::from_icon_name("network-wireless-symbolic");
    streaming_icon.set_visible(false);
    streaming_icon.set_tooltip_text(Some("Streaming"));
    let status = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    status.set_valign(gtk::Align::Center);
    status.append(&spinner);
    status.append(&streaming_icon);
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    left.set_valign(gtk::Align::Center);
    left.append(&cover);
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
    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    transport.set_valign(gtk::Align::Center);
    transport.append(&prev_chapter_btn);
    transport.append(&prev_btn);
    transport.append(&play_btn);
    transport.append(&next_btn);
    transport.append(&next_chapter_btn);
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
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    right.set_valign(gtk::Align::Center);
    right.append(&position);
    right.append(&seek);
    right.append(&volume);
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
        title,
        artist,
        left,
        spinner,
        streaming_icon,
        play_btn,
        prev_chapter_btn,
        next_chapter_btn,
        position,
        seek,
        volume,
    }
}

fn transport_button(icon: &str, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::from_icon_name(icon);
    btn.add_css_class("flat");
    btn.add_css_class("circular");
    btn.set_tooltip_text(Some(tooltip));
    btn
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
        self.set_cover(None);
        self.set_status(false, false);
        self.set_chapter_nav_visible(false);
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

    /// Show the album cover from `path`, or the placeholder when absent.
    pub fn set_cover(&self, path: Option<&std::path::Path>) {
        match path.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some(COVER_PLACEHOLDER)),
        }
    }
}
