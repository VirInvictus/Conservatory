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
    pub title: gtk::Label,
    pub artist: gtk::Label,
    pub play_btn: gtk::Button,
    pub position: gtk::Label,
    pub seek: gtk::Scale,
    pub volume: gtk::ScaleButton,
}

/// Build the Now-bar. When a `player` is present, the transport controls are
/// wired to it; without one (no library / libmpv unavailable) the bar renders
/// inert.
pub fn build_now_bar(player: Option<PlayerHandle>) -> NowBar {
    let root = gtk::CenterBox::new();
    root.add_css_class("now-bar");

    // Left: title (bold) over artist (dim).
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
    root.set_start_widget(Some(&info));

    // Centre: prev / play-pause / next.
    let prev_btn = transport_button("media-skip-backward-symbolic", "Previous");
    let play_btn = transport_button("media-playback-start-symbolic", "Play / Pause");
    let next_btn = transport_button("media-skip-forward-symbolic", "Next");
    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    transport.set_valign(gtk::Align::Center);
    transport.append(&prev_btn);
    transport.append(&play_btn);
    transport.append(&next_btn);
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
        title,
        artist,
        play_btn,
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
    }
}
