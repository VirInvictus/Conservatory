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
pub mod ape;
pub mod audit;
pub mod config;
pub mod covers;
pub mod db;
pub mod debug;
pub mod dedup;
pub mod edit;
pub mod errors;
pub mod import;
pub mod mover;
pub mod mpris;
pub mod path_template;
pub mod player;
pub mod playlist;
pub mod replaygain;
pub mod scrobble;
pub mod secret;
pub mod shelf_genre;
pub mod stats;
pub mod tags;
pub mod verify;
pub mod waveform;

pub use accent::{compute_accent, find_cover_bytes};
pub use ape::{
    ApeSpan, StripPlan, commit_strip, has_ape, locate_ape, plan_strip, restore_bytes, strip_bytes,
    write_atomic_plain, write_atomic_verified,
};
pub use audit::{
    ArtDeficiency, ArtResDeficiency, AuditOptions, AuditReport, BitrateDeficiency, RgBucket,
    RgCoverage, TagDeficiency, TagFlags, run_audit,
};
pub use config::{Config, ImportMode, ScrobbleConfig, config_path, load_default, save_default};
pub use covers::{resync_album_covers, sync_album_cover, write_cover};
pub use dedup::{
    DedupOptions, DuplicateReport, ExactAlbumDupe, MultiformatDupe, SimilarAlbums, TrackDupe,
    find_duplicates,
};
pub use edit::{
    AlbumEdit, Assignment, Field, TrackEdit, any_path_affecting, build_album_edit,
    build_track_edit, common_value, genres_assignment, parse_assignment, replace_in, split_genres,
};
pub use import::{ImportOptions, ImportReport, import_folder};
pub use mover::{Conflict, MoveKind, MoveMode, MoveOp, MovePlan, organize_ops, plan};
pub use path_template::{
    BookFields, DEFAULT_AUDIOBOOK_TEMPLATE, DEFAULT_MUSIC_TEMPLATE, PathTemplate, TrackFields,
    find_collisions,
};
pub use player::{
    AudioDevice, ChapterMark, EndReason, HostEvent, MpvHost, MusicProfile, PlayableItem,
    PlaybackConfig, PlayerCommand, PlayerHandle, PlayerSnapshot, Repeat, ReplayGain, SleepMode,
    SleepStatus, StateDebounce, StateEvent, build_af_chain, eq_stage, quick_seek_target,
    resolve_book_profile, resolve_episode_profile, resolve_music_profile, resolve_skip_amounts,
    shuffle_order,
};
pub use playlist::{M3uTrack, build_m3u, parse_m3u};
pub use replaygain::{
    DEFAULT_TARGET_LUFS, replaygain_from_file, rsgain_available, scan_album_files,
};
pub use scrobble::{
    DrainReport, LastfmClient, Listen, ListenBrainzClient, ListenSubmitter, ScrobbleService,
    SubmitError, backoff_secs, drain_ready,
};
pub use secret::{BasicAuth, CredentialStore};
pub use shelf_genre::{AlbumGenreInput, GenreVocab, normalize, resolve_album, resolve_shelf_genre};
pub use stats::{
    ArtistStat, BitrateStat, FormatStat, GenreStat, LibraryStats, RatingTally, compute_stats,
    format_size,
};
pub use tags::{EmbeddedCover, TagWrite, TrackDraft, read_track, write_track_tags};
pub use verify::{VerifyVerdict, ffmpeg_available, flac_available, verify_file, verify_files};
pub use waveform::{DEFAULT_BUCKETS, WaveformEnvelope, bucketize, compute_envelope, envelope_for};

/// Workspace version, surfaced for the CLI and GUI binaries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
