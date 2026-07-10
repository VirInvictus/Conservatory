//! The GTK4/libadwaita browse UI (Phase 3b). Programmatic widgets (no `.ui`
//! templates); all data logic lives in `conservatory-core`.

pub mod accent;
#[cfg(feature = "audiobooks")]
pub mod audiobooks;
pub mod coalescing;
pub mod covers;
pub mod dialogs;
pub mod facet_pane;
pub mod fields;
pub mod inspector;
pub mod now_bar;
pub mod now_playing_panel;
pub mod objects;
#[cfg(feature = "podcasts")]
pub mod podcasts;
pub mod queue_panel;
pub mod rows;
pub mod shortcuts;
pub mod sound;
pub mod spectrum;
pub mod status_page;
pub mod track_list;
pub mod window;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

/// Close `window` on Escape (Phase 26). Plain `gtk::Window` has no built-in
/// Escape handling; the adw dialogs this replaces did it for free.
pub fn close_on_escape(window: &gtk::Window) {
    let key = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gdk::Key::Escape {
            if let Some(win) = weak.upgrade() {
                win.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);
}
