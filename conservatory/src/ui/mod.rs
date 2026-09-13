//! The GTK4 browse UI (Phase 3b; plain GTK4 since Phase 26). Programmatic
//! widgets (no `.ui`
//! templates); all data logic lives in `conservatory-core`.

pub mod accent;
#[cfg(feature = "audiobooks")]
pub mod audiobooks;
pub mod coalescing;
pub mod covers;
pub mod facet_pane;
pub mod fields;
pub mod inspector;
pub mod now_bar;
pub mod now_playing_full;
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
pub mod waveform;
pub mod window;

/// Close `window` on Escape (Phase 26). Plain `gtk::Window` has no built-in
/// Escape handling; the adw dialogs this replaces did it for free.
/// Close `window` on Escape; re-exported from vir-gtk's widget kit (1.4.0),
/// whose capture-phase shape supersedes the local default-phase copy.
pub use vir_gtk::widgets::close_on_escape;
