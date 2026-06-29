//! The GTK4/libadwaita browse UI (Phase 3b). Programmatic widgets (no `.ui`
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
pub mod now_playing_panel;
pub mod objects;
#[cfg(feature = "podcasts")]
pub mod podcasts;
pub mod queue_panel;
pub mod sound;
pub mod spectrum;
pub mod track_list;
pub mod window;
