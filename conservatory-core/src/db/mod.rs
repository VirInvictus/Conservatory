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
pub use models::{
    Album, Artist, AudioState, Book, BookChapter, BookPerson, BookPlayback, Chapter, CompSettings,
    DspState, EQ_BAND_COUNT, EQ_CENTRES, Episode, EqPreset, EqState, Genre, InboxPolicy,
    LevelerSettings, LimiterSettings, ListeningSession, MediaKind, ModuleState, Perspective,
    Playback, PlaybackCursor, PlayedState, QueueItem, ResamplerQuality, Series, Show, ShowSettings,
    Tag, Track,
};
pub use pool::ReadPool;
pub use probe::probe_read;
pub use reads::{
    BookListRow, BookState, EpisodeListRow, LibraryCounts, ListeningTotals, NowPlaying,
    PlaybackStateRow, QueueDisplayRow, SearchRow, SqlParam, TrackRenderRow, TriageBucket,
    WritebackRow, album_track_genres, book_authors, book_chapters, book_metadata, book_narrators,
    episode_metadata, episodes_for_show, episodes_for_tag, episodes_in_bucket, fts_rank, get_album,
    get_artist, get_audio_state, get_book, get_book_playback, get_episode, get_episode_by_guid,
    get_eq_preset, get_eq_state, get_playback, get_show, get_show_settings, get_track, get_tracks,
    library_counts, list_albums, list_all_tags, list_book_rows, list_books, list_chapters,
    list_episodes_for_show, list_eq_presets, list_perspectives, list_shows, list_tags_for_show,
    listening_totals, load_queue, load_queue_display, perspective_expression, read_playback_state,
    search_rows, search_track_ids, series_for_book, show_settings_map, sort_shelf, track_metadata,
    track_render_rows, writeback_rows,
};
pub use worker::{WorkerHandle, spawn_worker};

/// Re-exported so consumers (e.g. the GUI's queue builders) can name the pooled
/// read connection type without depending on `rusqlite` directly.
pub use rusqlite::Connection;
