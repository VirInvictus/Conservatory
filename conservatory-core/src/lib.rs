//! Conservatory headless data layer.
//!
//! The GUI-free core (spec §2.2): the single-writer SQLite worker and
//! read-only pool, tag read/write, the import pipeline (path-template engine,
//! shelf-genre resolver, file mover with dry-run + undo), the libmpv host and
//! playback profiles, the unified-queue model, and the podcast fetch/parse
//! pipeline (ported from `belfry-core` at Phase 6).
//!
//! Phase 1a (spec §17) lands the spine: the single-writer SQLite worker, the
//! read-only pool, and the numbered-migration runner. Phase 1b adds the music
//! data model and FTS5; Phase 1c adds tag read and cover-art accent extraction.
//! The import pipeline, playback, podcasts, and audiobooks follow.

pub mod accent;
pub mod db;
pub mod errors;
pub mod import;
pub mod mover;
pub mod path_template;
pub mod player;
pub mod shelf_genre;
pub mod tags;

pub use accent::{compute_accent, find_cover_bytes};
pub use import::{ImportOptions, ImportReport, import_folder};
pub use mover::{Conflict, MoveKind, MoveMode, MoveOp, MovePlan, plan};
pub use path_template::{DEFAULT_MUSIC_TEMPLATE, PathTemplate, TrackFields, find_collisions};
pub use player::{
    EndReason, HostEvent, MpvHost, MusicProfile, PlaybackConfig, ReplayGain, StateDebounce,
    StateEvent, resolve_music_profile,
};
pub use shelf_genre::{AlbumGenreInput, GenreVocab, normalize, resolve_album, resolve_shelf_genre};
pub use tags::{EmbeddedCover, TrackDraft, read_track};

/// Workspace version, surfaced for the CLI and GUI binaries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
