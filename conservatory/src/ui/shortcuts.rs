//! Hand-built keyboard-shortcuts reference (Phase 13e-iii, `F1`; re-skinned
//! plain-GTK at Phase 26). `gtk::ShortcutsWindow` is deprecated, so this is a
//! modal window over the shared rows. The list is curated to match what is
//! actually wired (no aspirational keys); keep it in step with `docs/keymap.md`.

use gtk::prelude::*;
use gtk4 as gtk;

use crate::ui::rows;

const GROUPS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Playback",
        &[
            ("Space", "Play / pause"),
            ("Ctrl+Right", "Next track"),
            ("Ctrl+Left", "Previous track"),
            ("Ctrl+Up / Ctrl+Down", "Volume up / down"),
            ("Ctrl+0", "Mute / unmute"),
            ("Ctrl+M", "Stop after the current track"),
            ("Ctrl+J", "Jump to the playing track"),
            ("Ctrl+R", "Repeat: off / all / one"),
            ("Ctrl+K", "Shuffle: on / off"),
            ("Ctrl+Shift+Right / Left", "Next / previous chapter"),
            ("S", "Sleep timer"),
        ],
    ),
    (
        "Browse & Queue",
        &[
            ("Double-click / Enter", "Play the track or facet"),
            ("Ctrl+Enter", "Add the selection to the queue"),
            ("Ctrl+E", "Edit the selected tracks"),
            ("Ctrl+F", "Focus the filter"),
            ("Ctrl+L", "Clear the filter"),
            ("Alt+Up / Alt+Down", "Move the queued item"),
            ("Delete", "Remove from the queue"),
            ("Ctrl+Shift+C", "Clear the queue"),
        ],
    ),
    (
        "Panels & View",
        &[
            ("Ctrl+U", "Queue"),
            ("Ctrl+P", "Track properties"),
            ("Ctrl+I", "Now Playing"),
            ("Alt+1 / Alt+2 / Alt+3", "Music / Podcasts / Audiobooks"),
            ("Ctrl+comma", "Preferences"),
            ("F1", "This shortcuts window"),
            ("Ctrl+Q", "Quit"),
        ],
    ),
];

pub fn present(parent: &impl IsA<gtk::Window>) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(14)
        .margin_bottom(14)
        .margin_start(14)
        .margin_end(14)
        .build();
    for (title, entries) in GROUPS {
        let heading = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        content.append(&heading);
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_bottom(6)
            .build();
        for (accel, desc) in entries {
            let keys = gtk::Label::builder()
                .label(*accel)
                .xalign(1.0)
                .css_classes(["dim-label", "numeric"])
                .build();
            list.append(&rows::row(desc, None, Some(keys.upcast_ref())));
        }
        content.append(&list);
    }

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();

    let window = gtk::Window::builder()
        .title("Keyboard Shortcuts")
        .transient_for(parent)
        .modal(true)
        .default_width(460)
        .default_height(580)
        .child(&scroller)
        .build();
    super::close_on_escape(&window);
    window.present();
}
