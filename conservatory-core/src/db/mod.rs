//! SQLite worker, read pool, and schema migrations (spec §2.1, §4).

mod command;
mod connection;
pub mod facets;
pub mod fixtures;
pub mod migrations;
pub mod models;
pub mod pool;
mod probe;
pub mod reads;
mod worker;
mod writes;

#[cfg(test)]
mod fts_tests;

pub use facets::{
    FacetField, FacetFilter, FacetRow, TrackBrief, TrackSort, cmp_tracks, facet_rows, facet_tracks,
    sort_tracks,
};
pub use migrations::CURRENT_VERSION;
pub use models::{Album, Artist, Genre, Perspective, Track};
pub use pool::ReadPool;
pub use probe::probe_read;
pub use reads::{
    LibraryCounts, SearchRow, SqlParam, TrackRenderRow, album_track_genres, fts_rank, get_album,
    get_artist, get_track, library_counts, list_albums, list_perspectives, perspective_expression,
    search_rows, search_track_ids, track_render_rows,
};
pub use worker::{WorkerHandle, spawn_worker};
