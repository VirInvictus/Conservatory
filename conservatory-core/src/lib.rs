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
pub mod covers;
pub mod db;
pub mod edit;
pub mod errors;
pub mod import;
pub mod mover;
pub mod mpris;
pub mod path_template;
pub mod player;
pub mod replaygain;
pub mod shelf_genre;
pub mod tags;

pub use accent::{compute_accent, find_cover_bytes};
pub use covers::{resync_album_covers, sync_album_cover, write_cover};
pub use edit::{
    AlbumEdit, Assignment, Field, TrackEdit, any_path_affecting, build_album_edit,
    build_track_edit, genres_assignment, parse_assignment, replace_in, split_genres,
};
pub use import::{ImportOptions, ImportReport, import_folder};
pub use mover::{Conflict, MoveKind, MoveMode, MoveOp, MovePlan, organize_ops, plan};
pub use path_template::{DEFAULT_MUSIC_TEMPLATE, PathTemplate, TrackFields, find_collisions};
pub use player::{
    AudioDevice, EndReason, HostEvent, MpvHost, MusicProfile, PlayableItem, PlaybackConfig,
    PlayerCommand, PlayerHandle, PlayerSnapshot, ReplayGain, StateDebounce, StateEvent,
    build_af_chain, resolve_episode_profile, resolve_music_profile,
};
pub use replaygain::{
    DEFAULT_TARGET_LUFS, replaygain_from_file, rsgain_available, scan_album_files,
};
pub use shelf_genre::{AlbumGenreInput, GenreVocab, normalize, resolve_album, resolve_shelf_genre};
pub use tags::{EmbeddedCover, TagWrite, TrackDraft, read_track, write_track_tags};

/// Workspace version, surfaced for the CLI and GUI binaries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
