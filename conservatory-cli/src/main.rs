//! Conservatory headless CLI. The batch surface that pairs with the GUI (the
//! Hermitage / CalibreQuarry / Belfry pattern). Phase 1a ships a single debug
//! verb that exercises the worker + read-pool round-trip; the real verbs
//! (import, organize, search, tag, queue, podcast, stats) land at Phase 2+
//! (spec §9).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    MediaKind, ReadPool, ResamplerQuality, SearchRow, SqlParam, fts_rank, get_album, get_episode,
    get_show_settings, get_track, library_counts, load_queue, probe_read, read_playback_state,
    search_rows, search_track_ids, spawn_worker, track_render_rows, writeback_rows,
};
use conservatory_core::mover::{self, MoveKind, MoveMode, organize_ops};
use conservatory_core::{
    AlbumEdit, Assignment, DEFAULT_TARGET_LUFS, Field, GenreVocab, ImportOptions, ImportReport,
    PathTemplate, PlayableItem, PlaybackConfig, SleepMode, TagWrite, TrackDraft, TrackEdit,
    TrackFields, any_path_affecting, build_af_chain, build_album_edit, build_track_edit,
    compute_accent, find_collisions, find_cover_bytes, genres_assignment, import_folder,
    parse_assignment, read_track, replace_in, replaygain_from_file, resolve_album,
    resolve_episode_profile, resolve_music_profile, resync_album_covers, rsgain_available,
    scan_album_files, sync_album_cover, write_track_tags,
};
use conservatory_search::{
    SearchItem, SqlValue, blend_relevance, collect_text_terms, parse, try_translate,
};

/// Output format for the report-producing verbs (spec §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Tab-separated (the default; pipe-friendly).
    Tsv,
    /// A compact JSON summary object.
    Json,
    /// Human-readable lines.
    Human,
}

#[derive(Parser)]
#[command(
    name = "conservatory-cli",
    version,
    about = "Conservatory headless CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Phase 1a smoke test: open the DB, run migrations, and round-trip a row
    /// through the single-writer worker and the read-only pool.
    DebugRoundtrip {
        /// Path to the SQLite database (created if absent).
        db: PathBuf,
    },

    /// Phase 1b smoke test: load a synthetic library into the schema through the
    /// worker, then report the counts read back through the read pool.
    DebugFixture {
        /// Path to the SQLite database (created if absent).
        db: PathBuf,
        /// Fixture scale: small | medium | large.
        #[arg(long, default_value = "small")]
        scale: String,
    },

    /// Phase 1c smoke test: read an audio file's embedded tags into a draft and
    /// extract the cover-art accent. Pure read, no database.
    DebugTags {
        /// Path to an audio file (flac / mp3 / opus / m4a / ...).
        file: PathBuf,
    },

    /// Phase 2a smoke test: render the target path for every track in the DB
    /// from the default template, and report any colliding paths. Read-only.
    DebugPaths {
        /// Path to the SQLite database.
        db: PathBuf,
    },

    /// Phase 5.5a smoke test: resolve the playback profile for a track and print
    /// the libmpv `af` chain it renders to (the ReplayGain head stage), plus the
    /// ReplayGain / gapless / speed breakdown. Read-only.
    DebugDsp {
        /// Path to the SQLite database.
        db: PathBuf,
        /// A track id (omit for the first track in the library).
        track_id: Option<i64>,
    },

    /// Phase 2b smoke test: derive each album's shelf genre from its track tags
    /// and compare against the stored value. Read-only.
    DebugShelfGenre {
        /// Path to the SQLite database.
        db: PathBuf,
    },

    /// Import a folder (or file) into the library: scan, read tags, resolve, and
    /// move/copy into the managed tree (spec §5.4). Copies by default.
    Import {
        /// Path to the SQLite database (created if absent).
        db: PathBuf,
        /// Folder or file to import.
        source: PathBuf,
        /// Library root the managed tree is rendered under.
        root: PathBuf,
        /// Consume the originals (move) instead of copying them.
        #[arg(long)]
        r#move: bool,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },

    /// Re-render the managed tree from the database and move files to match
    /// (after a shelf-genre or metadata change). Dry-run by default.
    Organize {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Library root the relative DB paths hang off.
        root: PathBuf,
        /// Execute the move (default is a dry-run preview).
        #[arg(long)]
        apply: bool,
        /// Copy instead of move (leave the source files in place).
        #[arg(long)]
        copy: bool,
        /// Undo a previously-applied job by id instead of organizing.
        #[arg(long, value_name = "JOB_ID")]
        undo: Option<i64>,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },

    /// Play the unified queue through the libmpv engine (spec §6, Phase 4b):
    /// gapless + ReplayGain, advancing item to item, position persisted so a
    /// restart resumes. With a track id, replaces the queue with that one track
    /// ("play this now"); with none, plays the existing queue from the cursor.
    Play {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Library root the relative track paths hang off (as for `organize`).
        root: PathBuf,
        /// Track id to play now. Omit to play the existing queue from the cursor.
        track_id: Option<i64>,
        /// Arm a sleep timer (Phase 6c-iii-d): a number of minutes (e.g. `15`,
        /// `30`), `episode`/`item` (end of the current item), or `queue` (end of
        /// the queue). Playback pauses at the boundary; the run then exits.
        #[arg(long)]
        sleep: Option<String>,
    },

    /// Inspect and edit the unified queue (spec §4.3, Phase 4b).
    Queue {
        #[command(subcommand)]
        action: QueueAction,
    },

    /// Set an album's shelf genre (a path-affecting edit; run `organize` after to
    /// move the album).
    ShelfGenreSet {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Album id.
        album_id: i64,
        /// The new shelf genre.
        value: String,
    },

    /// Edit metadata across the tracks matching a search expression (spec §3.5).
    /// Path-affecting edits (album / albumartist / year / shelfgenre) move files
    /// through the Phase 2c mover (dry-run by default; `--apply` to execute).
    Tag {
        #[command(subcommand)]
        action: TagAction,
    },

    /// Write the curated DB metadata back into the matched files' embedded tags
    /// (spec §5.5). Dry-run by default (shows the per-file field diffs); `--apply`
    /// writes. Re-derivable from the DB, so there is no undo.
    EmbedTags {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Search expression selecting the tracks to write.
        query: String,
        /// Library root the relative track paths hang off.
        #[arg(long)]
        root: PathBuf,
        /// Write the tags (default is a dry-run diff).
        #[arg(long)]
        apply: bool,
    },

    /// Scan + write ReplayGain for the matched tracks via rsgain (spec §16.7,
    /// Phase 5c). Per-album album gain; refreshes the DB columns the player reads.
    Replaygain {
        #[command(subcommand)]
        action: ReplaygainAction,
    },

    /// Set an album's cover image: write it into the album folder as cover.jpg
    /// and record `cover_path` + a refreshed accent (Phase 5d).
    SetCover {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Album id.
        album_id: i64,
        /// The image file to use as the cover.
        image: PathBuf,
        /// Library root the album folder hangs off.
        #[arg(long)]
        root: PathBuf,
    },

    /// Filter the library with the search grammar (spec §3.4). Uses the SQL fast
    /// path when the whole expression translates, else the in-memory evaluator.
    Search {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The search expression.
        query: String,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },

    /// Manage podcast subscriptions (spec §8, Phase 6): subscribe to a feed,
    /// remove one, or refresh episodes via conditional GET. Only present when
    /// built with the `podcasts` plugin (the default).
    #[cfg(feature = "podcasts")]
    Podcast {
        #[command(subcommand)]
        action: PodcastAction,
    },

    /// Import subscriptions from an OPML file (spec §8): creates the shows and
    /// their tags, network-free. Run `podcast refresh` afterwards to pull
    /// episodes. Only present with the `podcasts` plugin.
    #[cfg(feature = "podcasts")]
    ImportOpml {
        /// Path to the SQLite database (created if absent).
        db: PathBuf,
        /// The OPML file to import.
        file: PathBuf,
    },

    /// Export every subscription (with tags + applePodcastsID) as OPML, to a
    /// file or stdout. Read-only. Only present with the `podcasts` plugin.
    #[cfg(feature = "podcasts")]
    ExportOpml {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// The graphic equalizer (Phase 5.5b, spec §6.2): show the active EQ, set a
    /// band, or manage named presets. The values apply to playback from the next
    /// loaded track; the live (instant) GUI sliders are Phase 5.5b-ii.
    Eq {
        #[command(subcommand)]
        action: EqAction,
    },

    /// The DSP modules (Phase 5.5c, spec §6.2): show or toggle the compressor,
    /// brick-wall limiter, and volume leveler. Applied to playback from the next
    /// loaded track.
    Dsp {
        #[command(subcommand)]
        action: DspAction,
    },

    /// Output settings (Phase 5.5c-ii, spec §6.5): the audio backend (mpv `ao`)
    /// and the resampler quality. The device picker is the GUI's (Phase 4c-ii).
    Output {
        #[command(subcommand)]
        action: OutputAction,
    },

    /// Phase 3b smoke test: dump the faceted-browse panes (Genre → Album Artist
    /// → Album) with counts and the leaf track total. Read-only.
    DebugFacets {
        /// Path to the SQLite database.
        db: PathBuf,
    },

    /// Audiobook tools (spec §3.8, Phase 7). Only present with the `audiobooks`
    /// plugin (the default). `import` / `set` land at 7a-iii.
    #[cfg(feature = "audiobooks")]
    Audiobook {
        #[command(subcommand)]
        action: AudiobookAction,
    },
}

/// Audiobook verbs. Gated behind the `audiobooks` plugin so a music-only build
/// has no audiobook surface.
#[cfg(feature = "audiobooks")]
#[derive(Subcommand)]
enum AudiobookAction {
    /// Read a folder or a single audio file into a book draft and print it: the
    /// resolved title / authors / narrators / series and the ordered chapter
    /// list. Network-free, read-only, no database (Phase 7a-ii). Embedded M4B
    /// chapters need `ffprobe` on PATH; without it a single file reads as one
    /// whole-file chapter.
    DebugRead {
        /// The book folder (multi-file) or a single audio file.
        path: PathBuf,
    },

    /// Import one book (a folder or a single `.m4b`) into the library: resolve it
    /// into rows and move its files into the managed `Audiobooks/` tree via the
    /// journaled, undoable mover (spec §5.4, §5.7). Defaults to copy; a move/undo
    /// conflict refuses the import (nonzero exit) with nothing written. One book
    /// per call; a whole-`Author/*`-tree batch is a later phase.
    Import {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The book folder (multi-file) or a single audio file.
        source: PathBuf,
        /// The managed library root the `Audiobooks/` tree hangs off.
        root: PathBuf,
        /// Consume the source files (move) instead of copying them.
        #[arg(long)]
        r#move: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },

    /// Edit a book's metadata (Phase 7b-iii). Non-path fields (rating / starred /
    /// shelf genre / narrator) apply immediately. Path-affecting fields (title /
    /// year / author / series / series index) re-render the folder, so they need
    /// `--root` and re-shelve the book's files through the journaled mover; without
    /// `--apply` the move is previewed (dry-run). Undo is `organize --undo <job>`.
    Set {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The book id to edit.
        book_id: i64,
        /// Set the title (path-affecting).
        #[arg(long)]
        title: Option<String>,
        /// Set the release year (path-affecting).
        #[arg(long)]
        year: Option<i32>,
        /// Replace the author(s) (path-affecting); repeat or `;`-separate names.
        #[arg(long)]
        author: Vec<String>,
        /// Replace the narrator(s); repeat or `;`-separate names.
        #[arg(long)]
        narrator: Vec<String>,
        /// Set the series, or `""` to clear to standalone (path-affecting).
        #[arg(long)]
        series: Option<String>,
        /// Set the series index, e.g. `1` or `1.5` (path-affecting).
        #[arg(long)]
        series_index: Option<f64>,
        /// Set the rating (0–5).
        #[arg(long)]
        rating: Option<u8>,
        /// Set or clear the starred flag.
        #[arg(long)]
        starred: Option<bool>,
        /// Set the shelf genre (the single-valued path input, spec §5.2).
        #[arg(long)]
        shelf_genre: Option<String>,
        /// Library root (required when a path-affecting field changes).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Execute the re-shelve move (default previews it).
        #[arg(long)]
        apply: bool,
    },

    /// Play a book through the libmpv engine as one queue item (Phase 7c): the
    /// engine advances file to file across the book's chapters internally (no gap
    /// for an M4B; one loadfile per file for a multi-file book) and completes the
    /// book at the last file's EOF. Position persistence + resume land at 7c-ii.
    Play {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The book id to play.
        book_id: i64,
        /// Library root the relative chapter paths hang off (as for `organize`).
        root: PathBuf,
        /// Resume at the book's saved absolute position (spec §6.4) instead of
        /// starting from the beginning.
        #[arg(long)]
        resume: bool,
        /// Arm a sleep timer (Phase 6c-iii-d): minutes (e.g. `30`), `book`/`item`
        /// (end of the book), or `queue`. Playback pauses at the boundary.
        #[arg(long)]
        sleep: Option<String>,
    },

    /// Set a book's per-book playback overrides (Phase 7c-ii, spec §6.3): variable
    /// speed, Smart Speed, Voice Boost. Omitted flags are left unchanged; the
    /// resume position and finished state are preserved.
    Settings {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The book id.
        book_id: i64,
        /// Playback speed (e.g. `1.0`, `1.5`); clamped to the spoken-word range.
        #[arg(long)]
        speed: Option<f64>,
        /// Smart Speed (silence trimming) on/off.
        #[arg(long)]
        smart_speed: Option<bool>,
        /// Voice Boost (compression + voice EQ + leveler) on/off.
        #[arg(long)]
        voice_boost: Option<bool>,
    },

    /// List the audiobook shelf: every book with its denormalized author /
    /// narrator / series, progress, and derived state (New / In progress /
    /// Finished), ordered in-progress first (Phase 7b-i). Read-only; the headless
    /// view of the GUI shelf.
    List {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Optional filter expression (the §3.4 grammar, with the audiobook
        /// fields `author:`/`narrator:`/`series:`/`is:finished`). Omitted lists
        /// the whole shelf. Evaluated in memory, like the GUI shelf.
        expr: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },
}

/// Equalizer verbs (Phase 5.5b).
#[derive(Subcommand)]
enum EqAction {
    /// Print the active EQ: each band's centre + gain, the selected preset, and
    /// the resolved `@eq` chain. Read-only.
    Show {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// Set one band's gain in dB (clamped to ±24); marks the EQ a custom edit.
    #[command(allow_negative_numbers = true)]
    Set {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Band index, 0 (lowest) to 9 (highest).
        band: usize,
        /// Gain in dB (may be negative).
        gain: f64,
    },
    /// Manage named presets.
    Preset {
        #[command(subcommand)]
        action: EqPresetAction,
    },
}

/// Equalizer-preset verbs (Phase 5.5b).
#[derive(Subcommand)]
enum EqPresetAction {
    /// List every named preset with its band gains. Read-only.
    List {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// Save the current EQ as a named preset (overwrites a same-named one).
    Save {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Preset name.
        name: String,
    },
    /// Load a named preset into the active EQ.
    Load {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Preset name.
        name: String,
    },
    /// Delete a named preset (`Flat` cannot be deleted).
    Delete {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Preset name.
        name: String,
    },
}

/// Whether a DSP module is on or off (the `dsp` verb's required state argument).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn enabled(self) -> bool {
        matches!(self, OnOff::On)
    }
}

/// DSP-module verbs (Phase 5.5c). Each module's parameters persist while it is
/// off, so toggling it back on restores them; the optional flags edit those
/// parameters. Negative dB values are allowed (thresholds/ceilings).
#[derive(Subcommand)]
enum DspAction {
    /// Print the active DSP modules and the resolved `af` chain. Read-only.
    Show {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// Enable/disable the compressor (`acompressor`), optionally setting params.
    #[command(allow_negative_numbers = true)]
    Comp {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Turn the compressor on or off.
        #[arg(value_enum)]
        state: OnOff,
        /// Threshold in dBFS (below which compression eases off).
        #[arg(long)]
        threshold: Option<f64>,
        /// Compression ratio (N:1).
        #[arg(long)]
        ratio: Option<f64>,
        /// Attack time in milliseconds.
        #[arg(long)]
        attack: Option<f64>,
        /// Release time in milliseconds.
        #[arg(long)]
        release: Option<f64>,
    },
    /// Enable/disable the brick-wall limiter (`alimiter`), optionally setting the
    /// ceiling.
    #[command(allow_negative_numbers = true)]
    Limiter {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Turn the limiter on or off.
        #[arg(value_enum)]
        state: OnOff,
        /// Output ceiling in dBFS.
        #[arg(long)]
        ceiling: Option<f64>,
    },
    /// Enable/disable the volume leveler (`dynaudnorm`), optionally setting params.
    Leveler {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Turn the leveler on or off.
        #[arg(value_enum)]
        state: OnOff,
        /// Target peak (0..1).
        #[arg(long)]
        target: Option<f64>,
        /// Gaussian window size (odd, 3..301; larger smooths the gain curve).
        #[arg(long)]
        gausssize: Option<u32>,
    },
}

/// Output verbs (Phase 5.5c-ii). The persisted backend / resampler are consumed
/// by the player host on the next load (and live, for the backend, via `ao-reload`
/// in the GUI). Avoid-resample stays the default; `high` only raises quality for
/// the unavoidable-resample case.
#[derive(Subcommand)]
enum OutputAction {
    /// Print the active output backend and resampler quality. Read-only.
    Show {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// Set the output backend (mpv `ao` driver).
    Backend {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The backend: `auto` (mpv autoprobe) or a pinned driver.
        #[arg(value_enum)]
        backend: BackendArg,
    },
    /// Set the resampler quality.
    Resampler {
        /// Path to the SQLite database.
        db: PathBuf,
        /// `default` (mpv's resampler) or `high` (raised `audio-resample-*`).
        #[arg(value_enum)]
        quality: ResamplerArg,
    },
}

/// The output-backend choices (Phase 5.5c-ii). The kebab-case value is both the
/// stored `audio_state.output_backend` and the mpv `ao` driver name.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum BackendArg {
    Auto,
    Pipewire,
    Pulse,
    Alsa,
    Jack,
}

impl BackendArg {
    fn as_str(self) -> &'static str {
        match self {
            BackendArg::Auto => "auto",
            BackendArg::Pipewire => "pipewire",
            BackendArg::Pulse => "pulse",
            BackendArg::Alsa => "alsa",
            BackendArg::Jack => "jack",
        }
    }
}

/// The resampler-quality choices (Phase 5.5c-ii), mapped to core's enum.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum ResamplerArg {
    Default,
    High,
}

impl ResamplerArg {
    fn to_core(self) -> ResamplerQuality {
        match self {
            ResamplerArg::Default => ResamplerQuality::Default,
            ResamplerArg::High => ResamplerQuality::High,
        }
    }
}

/// Podcast subscription verbs (spec §9). Gated behind the `podcasts` plugin so
/// the music-only build does not expose them.
#[cfg(feature = "podcasts")]
#[derive(Subcommand)]
enum PodcastAction {
    /// Subscribe to a feed URL: fetch it, create the show, and pull its
    /// episodes. Re-adding an existing feed just refreshes it (idempotent).
    Add {
        /// Path to the SQLite database (created if absent).
        db: PathBuf,
        /// The RSS/Atom feed URL.
        url: String,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },
    /// Unsubscribe: delete a show and cascade its episodes / state / queue rows.
    Remove {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The show id to remove.
        show_id: i64,
    },
    /// Re-poll subscriptions with conditional GET and upsert new episodes. With
    /// a show id, refreshes just that show; otherwise refreshes all.
    Refresh {
        /// Path to the SQLite database.
        db: PathBuf,
        /// A single show id to refresh (omit to refresh every subscription).
        show_id: Option<i64>,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },
    /// Download an episode's audio into the managed tree (spec §5.3) and record
    /// its `audio_path`. Uses the show's stored Basic-auth credentials if any.
    Download {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The episode id to download.
        episode_id: i64,
        /// Library root the managed `Podcasts/` tree hangs off.
        #[arg(long)]
        root: PathBuf,
    },
    /// Prune downloaded episodes beyond a show's `keep_count` (retention, Phase
    /// 6b-ii-c-3-b): delete the oldest downloads' files and revert them to
    /// stream-only. Dry-run by default (lists what would go); `--apply` deletes.
    Prune {
        /// Path to the SQLite database.
        db: PathBuf,
        /// A single show id (omit to prune every subscription).
        show_id: Option<i64>,
        /// Library root the managed `Podcasts/` tree hangs off.
        #[arg(long)]
        root: PathBuf,
        /// Actually delete the files (default: dry-run preview only).
        #[arg(long)]
        apply: bool,
    },
    /// List episodes with their triage state (spec §3.7): a show's episodes, or
    /// a triage bucket across all subscriptions. Read-only.
    Episodes {
        /// Path to the SQLite database.
        db: PathBuf,
        /// A single show id (its episodes, newest first).
        #[arg(long, conflicts_with = "bucket")]
        show: Option<i64>,
        /// A triage bucket across all shows: inbox | queue | played (default inbox).
        #[arg(long)]
        bucket: Option<String>,
        #[arg(long, value_enum, default_value_t = Format::Tsv)]
        format: Format,
    },
    /// Set an episode's played state (triage, spec §3.7): played | unplayed |
    /// archived. Preserves the starred flag; `unplayed` rewinds the position.
    Mark {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The episode id.
        episode_id: i64,
        /// played | unplayed | archived.
        state: String,
    },
    /// Star or unstar an episode (triage, spec §3.7).
    Star {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The episode id.
        episode_id: i64,
        /// Unstar instead of star.
        #[arg(long)]
        off: bool,
    },
    /// Show or set a show's per-show overrides (spec §3.7). With no flags it
    /// prints the current settings; `--speed` sets the playback speed (Phase
    /// 6b-ii-c-3-a). Smart Speed / Voice Boost filters are Phase 6c.
    Settings {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The show id.
        show_id: i64,
        /// Set the playback speed (e.g. 1.5); omit to just view.
        #[arg(long)]
        speed: Option<f64>,
    },
    /// Print the resolved `af` chain for an episode (Phase 6c debug): its
    /// spoken-word profile (Smart Speed / Voice Boost from the show settings)
    /// composed with the persisted EQ + DSP. Read-only.
    DebugChain {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The episode id.
        episode_id: i64,
    },
    /// Show listening totals: sessions, time listened, audio covered, and the
    /// wall-clock time Smart Speed saved (Phase 6c-ii). Read-only.
    Stats {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// List an episode's stored chapters (Phase 6c-iii). Read-only.
    Chapters {
        /// Path to the SQLite database.
        db: PathBuf,
        /// The episode id.
        episode_id: i64,
    },
}

#[derive(Subcommand)]
enum ReplaygainAction {
    /// Scan the matched tracks' albums and write ReplayGain (dry-run by default).
    Scan {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Search expression selecting the tracks to scan.
        query: String,
        /// Library root the relative track paths hang off.
        #[arg(long)]
        root: PathBuf,
        /// Run rsgain and write tags (default is a dry-run report).
        #[arg(long)]
        apply: bool,
        /// Reference loudness in LUFS (RG 2.0 default is -18).
        #[arg(long, default_value_t = DEFAULT_TARGET_LUFS)]
        target_lufs: f64,
    },
}

#[derive(Subcommand)]
enum TagAction {
    /// Set one or more `field=value` across the matched tracks. Fields:
    /// title, artist, rating (track); album, albumartist, year, shelfgenre
    /// (album); genre (raw multi-value, `;`-separated).
    Set {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Search expression selecting the tracks to edit.
        query: String,
        /// One or more assignments, e.g. `year=1992` `genre=Electronic; Ambient`.
        #[arg(required = true)]
        assignments: Vec<String>,
        /// Library root (required only when a path-affecting field changes).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Execute path-affecting moves (default previews them).
        #[arg(long)]
        apply: bool,
    },
    /// Search-and-replace a substring within a single text field across the
    /// matched tracks. Fields: title, artist (track); album, shelfgenre (album).
    Replace {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Search expression selecting the tracks to edit.
        query: String,
        /// Field to edit: title | artist | album | shelfgenre.
        field: String,
        /// Substring to find.
        find: String,
        /// Replacement text.
        replace: String,
        /// Library root (required only when a path-affecting field changes).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Execute path-affecting moves (default previews them).
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum QueueAction {
    /// Append tracks to the queue tail.
    Add {
        /// Path to the SQLite database.
        db: PathBuf,
        /// Track ids to enqueue, in order.
        track_ids: Vec<i64>,
    },
    /// Print the queue in order.
    List {
        /// Path to the SQLite database.
        db: PathBuf,
    },
    /// Remove the entry at a 0-based position.
    Remove {
        /// Path to the SQLite database.
        db: PathBuf,
        /// 0-based position to remove.
        position: i64,
    },
    /// Empty the queue.
    Clear {
        /// Path to the SQLite database.
        db: PathBuf,
    },
}

/// The compile-time plugins this binary was built with (spec §2.2). The match
/// on an empty slice (rather than `is_empty`) keeps clippy's compile-time-
/// constant lints quiet across both feature sets.
fn plugin_list() -> String {
    let plugins: &[&str] = &[
        #[cfg(feature = "podcasts")]
        "podcasts",
        #[cfg(feature = "audiobooks")]
        "audiobooks",
    ];
    match plugins {
        [] => "none (music-only build)".to_string(),
        _ => plugins.join(", "),
    }
}

fn main() -> Result<()> {
    init_tracing();
    match Cli::parse().command {
        Some(Command::DebugRoundtrip { db }) => debug_roundtrip(db),
        Some(Command::DebugFixture { db, scale }) => debug_fixture(db, scale),
        Some(Command::DebugTags { file }) => debug_tags(file),
        Some(Command::DebugPaths { db }) => debug_paths(db),
        Some(Command::DebugDsp { db, track_id }) => debug_dsp(db, track_id),
        Some(Command::DebugShelfGenre { db }) => debug_shelf_genre(db),
        Some(Command::Import {
            db,
            source,
            root,
            r#move,
            format,
        }) => import(db, source, root, r#move, format),
        Some(Command::Organize {
            db,
            root,
            apply,
            copy,
            undo,
            format,
        }) => organize(db, root, apply, copy, undo, format),
        Some(Command::ShelfGenreSet {
            db,
            album_id,
            value,
        }) => shelf_genre_set(db, album_id, value),
        Some(Command::Play {
            db,
            root,
            track_id,
            sleep,
        }) => play(db, root, track_id, sleep),
        Some(Command::Queue { action }) => queue(action),
        Some(Command::Tag { action }) => tag(action),
        Some(Command::EmbedTags {
            db,
            query,
            root,
            apply,
        }) => embed_tags(db, query, root, apply),
        Some(Command::Replaygain {
            action:
                ReplaygainAction::Scan {
                    db,
                    query,
                    root,
                    apply,
                    target_lufs,
                },
        }) => block_on(run_replaygain_scan(db, query, root, apply, target_lufs)),
        Some(Command::SetCover {
            db,
            album_id,
            image,
            root,
        }) => block_on(run_set_cover(db, album_id, image, root)),
        Some(Command::Search { db, query, format }) => search(db, query, format),
        #[cfg(feature = "podcasts")]
        Some(Command::Podcast { action }) => podcast(action),
        #[cfg(feature = "podcasts")]
        Some(Command::ImportOpml { db, file }) => block_on(run_import_opml(db, file)),
        #[cfg(feature = "podcasts")]
        Some(Command::ExportOpml { db, out }) => block_on(run_export_opml(db, out)),
        Some(Command::Eq { action }) => eq(action),
        Some(Command::Dsp { action }) => dsp(action),
        Some(Command::Output { action }) => output(action),
        Some(Command::DebugFacets { db }) => debug_facets(db),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action: AudiobookAction::DebugRead { path },
        }) => audiobook_debug_read(path),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action:
                AudiobookAction::Import {
                    db,
                    source,
                    root,
                    r#move,
                    format,
                },
        }) => block_on(run_audiobook_import(db, source, root, r#move, format)),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action:
                AudiobookAction::Set {
                    db,
                    book_id,
                    title,
                    year,
                    author,
                    narrator,
                    series,
                    series_index,
                    rating,
                    starred,
                    shelf_genre,
                    root,
                    apply,
                },
        }) => block_on(run_audiobook_set(AudiobookSetArgs {
            db,
            book_id,
            title,
            year,
            author,
            narrator,
            series,
            series_index,
            rating,
            starred,
            shelf_genre,
            root,
            apply,
        })),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action: AudiobookAction::List { db, expr, format },
        }) => run_audiobook_list(db, expr, format),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action:
                AudiobookAction::Play {
                    db,
                    book_id,
                    root,
                    resume,
                    sleep,
                },
        }) => run_audiobook_play(db, book_id, root, sleep, resume),
        #[cfg(feature = "audiobooks")]
        Some(Command::Audiobook {
            action:
                AudiobookAction::Settings {
                    db,
                    book_id,
                    speed,
                    smart_speed,
                    voice_boost,
                },
        }) => block_on(run_audiobook_settings(
            db,
            book_id,
            speed,
            smart_speed,
            voice_boost,
        )),
        None => {
            println!("conservatory-cli {}", conservatory_core::VERSION);
            println!("plugins: {}", plugin_list());
            println!("Try `import <db> <folder> <root>`, then `organize <db> <root> --apply`.");
            Ok(())
        }
    }
}

fn debug_roundtrip(db: PathBuf) -> Result<()> {
    // Write commands spin up the worker on a current-thread runtime and shut
    // down cleanly (the Atrium/Belfry pattern, spec §9).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building current-thread runtime")?;
    runtime.block_on(run_roundtrip(db))
}

async fn run_roundtrip(db: PathBuf) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    worker
        .probe_write("hello", "world")
        .await
        .context("probe write")?;

    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let value = probe_read(&pool, "hello")
        .context("probe read")?
        .context("round-trip failed: value missing after write")?;

    worker.shutdown_ack().await.context("shutdown ack")?;

    println!("OK: hello={value}");
    Ok(())
}

fn debug_fixture(db: PathBuf, scale: String) -> Result<()> {
    let scale: FixtureScale = scale
        .parse()
        .with_context(|| format!("invalid scale {scale:?} (expected small|medium|large)"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building current-thread runtime")?;
    runtime.block_on(run_fixture(db, scale))
}

fn debug_tags(file: PathBuf) -> Result<()> {
    let draft = read_track(&file).with_context(|| format!("reading tags from {file:?}"))?;

    println!("source:       {}", draft.source_path.display());
    println!("format:       {}", opt(&draft.format));
    println!("title:        {}", opt(&draft.title));
    println!("artist:       {}", opt(&draft.artist));
    println!("album artist: {}", opt(&draft.album_artist));
    println!("album:        {}", opt(&draft.album));
    println!(
        "track:        {}",
        num_of(draft.track_no, draft.track_total)
    );
    println!("disc:         {}", num_of(draft.disc_no, draft.disc_total));
    println!("year:         {}", opt(&draft.year));
    println!("genres:       {}", join(&draft.genres));
    println!(
        "replaygain:   {}",
        num_of_f(draft.replaygain_track, draft.replaygain_album)
    );
    println!("bitrate:      {}", opt(&draft.bitrate));
    println!("sample rate:  {}", opt(&draft.sample_rate));
    println!("duration:     {}", opt(&draft.duration));

    match find_cover_bytes(&file, &draft) {
        Some(bytes) => {
            let accent = compute_accent(&bytes).context("computing accent")?;
            println!("cover:        {} bytes", bytes.len());
            println!("accent:       #{accent:06X}");
        }
        None => println!("cover:        (none)"),
    }
    Ok(())
}

/// Read a folder or single file into a [`conservatory_audiobooks::BookDraft`] and
/// print it (Phase 7a-ii). The headless artifact for the audiobook reader: no
/// database, no move, no covers/accent (all 7a-iii).
#[cfg(feature = "audiobooks")]
fn audiobook_debug_read(path: PathBuf) -> Result<()> {
    use conservatory_audiobooks::{PersonDraft, read_book};

    let draft = read_book(&path).with_context(|| format!("reading book from {path:?}"))?;

    let names = |ps: &[PersonDraft]| -> String {
        if ps.is_empty() {
            "(none)".to_string()
        } else {
            ps.iter()
                .map(|p| format!("{} [{}]", p.name, p.sort_name))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };

    println!("source dir:   {}", draft.source_dir.display());
    println!("title:        {}", opt(&draft.title));
    println!("subtitle:     {}", opt(&draft.subtitle));
    println!("authors:      {}", names(&draft.authors));
    println!("narrators:    {}", names(&draft.narrators));
    let series = match (&draft.series, draft.series_sequence) {
        (Some(s), Some(n)) => format!("{s} #{n}"),
        (Some(s), None) => s.clone(),
        _ => "(none)".to_string(),
    };
    println!("series:       {series}");
    println!("year:         {}", opt(&draft.year));
    println!("publisher:    {}", opt(&draft.publisher));
    println!("isbn:         {}", opt(&draft.isbn));
    println!("asin:         {}", opt(&draft.asin));
    println!("language:     {}", opt(&draft.language));
    match &draft.cover {
        Some(bytes) => println!("cover:        {} bytes", bytes.len()),
        None => println!("cover:        (none)"),
    }
    if let Some(desc) = &draft.description {
        let head: String = desc.chars().take(160).collect();
        let tail = if desc.chars().count() > 160 {
            "..."
        } else {
            ""
        };
        println!("description:  {head}{tail}");
    }

    println!("chapters:     {}", draft.chapters.len());
    for ch in &draft.chapters {
        let title = ch.title.as_deref().unwrap_or("(untitled)");
        let dur = ch
            .duration
            .map(|d| format!("{d:.0}s"))
            .unwrap_or_else(|| "?".to_string());
        let file = ch
            .file_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("?");
        println!(
            "  {:>3}. @{:>8.1}s  {:>6}  {}  [{}]",
            ch.idx, ch.file_offset, dur, title, file
        );
    }
    Ok(())
}

/// Import one book into the managed tree (Phase 7a-iii). Mirrors `run_import`:
/// spawn the worker + read pool, run the journaled mover, print a report; a
/// conflict refuses the import (nonzero exit) with nothing written.
#[cfg(feature = "audiobooks")]
async fn run_audiobook_import(
    db: PathBuf,
    source: PathBuf,
    root: PathBuf,
    r#move: bool,
    format: Format,
) -> Result<()> {
    use conservatory_audiobooks::{BookImportOptions, import_book};

    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    // Heal any job interrupted by a previous crash before starting a new one.
    mover::recover(&worker, &pool).await.context("recovery")?;

    let opts = BookImportOptions {
        library_root: root,
        mode: if r#move {
            MoveMode::Move
        } else {
            MoveMode::Copy
        },
    };
    let report = import_book(&worker, &pool, &source, &opts)
        .await
        .context("import")?;
    worker.shutdown_ack().await.context("shutdown ack")?;

    print_book_import_report(&report, format);
    if !report.conflicts.is_empty() {
        anyhow::bail!(
            "import refused: {} conflict(s); nothing imported",
            report.conflicts.len()
        );
    }
    Ok(())
}

#[cfg(feature = "audiobooks")]
fn print_book_import_report(r: &conservatory_audiobooks::BookImportReport, format: Format) {
    let title = r.title.as_deref().unwrap_or("(untitled)");
    let job = r.job_id.map(|j| j.to_string());
    match format {
        Format::Json => println!(
            "{{\"title\":{:?},\"authors\":{},\"narrators\":{},\"chapters\":{},\"files\":{},\"book_id\":{},\"job_id\":{},\"conflicts\":{}}}",
            title,
            r.authors,
            r.narrators,
            r.chapters,
            r.files,
            r.book_id
                .map(|b| b.to_string())
                .as_deref()
                .unwrap_or("null"),
            job.as_deref().unwrap_or("null"),
            r.conflicts.len(),
        ),
        Format::Tsv => {
            println!("title\tauthors\tnarrators\tchapters\tfiles\tbook_id\tjob_id\tconflicts");
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                title,
                r.authors,
                r.narrators,
                r.chapters,
                r.files,
                r.book_id.map(|b| b.to_string()).unwrap_or_default(),
                job.as_deref().unwrap_or(""),
                r.conflicts.len(),
            );
        }
        Format::Human => match r.book_id {
            Some(id) => println!(
                "imported \"{title}\" (book {id}): {} chapter(s) across {} file(s), {} author(s) / {} narrator(s) (job {})",
                r.chapters,
                r.files,
                r.authors,
                r.narrators,
                job.as_deref().unwrap_or("?"),
            ),
            None if !r.conflicts.is_empty() => {
                println!(
                    "refused: {} conflict(s); nothing imported",
                    r.conflicts.len()
                );
                for c in &r.conflicts {
                    println!("  {c:?}");
                }
            }
            None => println!("nothing to import"),
        },
    }
}

/// The parsed `audiobook set` flags (grouped to keep the handler signature sane).
#[cfg(feature = "audiobooks")]
struct AudiobookSetArgs {
    db: PathBuf,
    book_id: i64,
    title: Option<String>,
    year: Option<i32>,
    author: Vec<String>,
    narrator: Vec<String>,
    series: Option<String>,
    series_index: Option<f64>,
    rating: Option<u8>,
    starred: Option<bool>,
    shelf_genre: Option<String>,
    root: Option<PathBuf>,
    apply: bool,
}

/// Edit a book's metadata, re-shelving its files when a path-affecting field
/// changes (Phase 7b-iii). Non-path edits apply immediately; a path-affecting
/// edit needs `--root` and, without `--apply`, only previews the move.
#[cfg(feature = "audiobooks")]
async fn run_audiobook_set(args: AudiobookSetArgs) -> Result<()> {
    use conservatory_audiobooks::edit::{BookEdit, SeriesEdit, split_people};
    use conservatory_audiobooks::{apply_book_edit, apply_book_reorg};
    use conservatory_core::db::{book_authors, get_book, series_for_book};
    use conservatory_core::mover::MoveMode;

    if let Some(r) = args.rating
        && r > 5
    {
        anyhow::bail!("rating must be 0–5");
    }

    // `--author X --author Y` and `--author "X; Y"` both work (the dialog's `;`
    // convention). An empty flag list leaves the credited set unchanged.
    let people = |flags: &[String]| -> Option<Vec<String>> {
        if flags.is_empty() {
            None
        } else {
            Some(split_people(&flags.join("; ")))
        }
    };
    // `--series ""` clears to standalone; omitted leaves it unchanged.
    let series = match &args.series {
        None => None,
        Some(s) if s.trim().is_empty() => Some(SeriesEdit::Clear),
        Some(s) => Some(SeriesEdit::Set(s.clone())),
    };

    let edit = BookEdit {
        title: args.title.clone(),
        year: args.year,
        series,
        series_index: args.series_index,
        authors: people(&args.author),
        narrators: people(&args.narrator),
        shelf_genre: args.shelf_genre.clone(),
        rating: args.rating,
        starred: args.starred,
    };
    if edit.is_empty() {
        anyhow::bail!("nothing to set: pass at least one field flag");
    }

    let path_affecting = edit.is_path_affecting();
    if path_affecting && args.root.is_none() {
        anyhow::bail!(
            "a path-affecting edit (title / year / author / series / series index) needs --root"
        );
    }

    let pool = ReadPool::new(args.db.clone(), 3).context("opening read pool")?;

    // Dry-run: show the intended changes and the folder the book would move to,
    // writing nothing (the trust model — the move is the risk).
    if path_affecting && !args.apply {
        let conn = pool.open().context("opening pool connection")?;
        let book = get_book(&conn, args.book_id)
            .context("reading book")?
            .ok_or_else(|| anyhow::anyhow!("no book with id {}", args.book_id))?;
        let cur_series = series_for_book(&conn, args.book_id).context("reading series")?;
        let cur_authors = book_authors(&conn, args.book_id).context("reading authors")?;
        let new_folder = conservatory_audiobooks::edit::rendered_folder(
            &book,
            cur_series.as_ref().map(|s| s.name.as_str()),
            cur_authors.first().map(|p| p.sort_name.as_str()),
            &edit,
        );
        println!("dry run (pass --apply to write and re-shelve):");
        println!("  current folder: {}", book.folder_path);
        println!("  new folder:     {}", new_folder.display());
        return Ok(());
    }

    let worker = spawn_worker(args.db).context("spawning worker")?;
    apply_book_edit(&worker, args.book_id, &edit)
        .await
        .context("applying edit")?;

    if path_affecting {
        let root = args.root.expect("checked above");
        match apply_book_reorg(&worker, &pool, args.book_id, &root, MoveMode::Move)
            .await
            .context("re-shelving book")?
        {
            Some(job) => println!("updated book {} and re-shelved (job {job})", args.book_id),
            None => println!("updated book {} (already in place)", args.book_id),
        }
    } else {
        println!("updated book {}", args.book_id);
    }
    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

/// Print the audiobook shelf (Phase 7b-i): the denormalized rows in shelf order
/// (in-progress first), the headless view of the GUI shelf. Read-only.
#[cfg(feature = "audiobooks")]
fn run_audiobook_list(db: PathBuf, expr: Option<String>, format: Format) -> Result<()> {
    use conservatory_core::db::{list_book_rows, sort_shelf};
    use conservatory_search::evaluate;

    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let mut rows = list_book_rows(&conn).context("reading shelf")?;
    sort_shelf(&mut rows);

    // Optional grammar filter, evaluated in memory (the audiobook fields have no
    // music `tracks` column, so there is no SQL fast path; the shelf is small).
    // `vl:` degrades to text, the same as the `search` verb (no resolver here).
    if let Some(query) = expr.as_deref().filter(|q| !q.trim().is_empty()) {
        let today = Utc::now().date_naive();
        let parsed = parse(query);
        for w in &parsed.warnings {
            eprintln!("warning: {w}");
        }
        rows.retain(|r| evaluate(&parsed.expr, &book_search_item(r), today));
    }

    print_book_rows(&rows, format);
    Ok(())
}

/// Play one book through the libmpv engine as a single queue item (Phase 7c-i):
/// the engine advances file to file across the book's chapters internally and
/// completes it at the last file's EOF. A headless exercise of the segment
/// engine; resume + per-book profile are 7c-ii. Teardown order: player -> worker
/// -> runtime (the engine thread `block_on`s the worker).
#[cfg(feature = "audiobooks")]
fn run_audiobook_play(
    db: PathBuf,
    book_id: i64,
    root: PathBuf,
    sleep: Option<String>,
    resume: bool,
) -> Result<()> {
    use conservatory_core::db::{book_chapters, get_book, get_book_playback};
    use conservatory_core::player::build_book_item;
    use conservatory_core::resolve_book_profile;

    let sleep_mode = sleep.as_deref().map(parse_sleep_spec).transpose()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .context("building runtime")?;
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).context("spawning worker")?
    };
    let pool = ReadPool::new(db, 3).context("opening read pool")?;

    let (book, chapters, playback) = {
        let conn = pool.open().context("opening pool connection")?;
        let book = get_book(&conn, book_id)
            .context("reading the book")?
            .with_context(|| format!("no book with id {book_id}"))?;
        let chapters = book_chapters(&conn, book_id).context("reading chapters")?;
        let playback = get_book_playback(&conn, book_id).context("reading playback")?;
        (book, chapters, playback)
    };
    if chapters.is_empty() {
        anyhow::bail!("book {book_id} ({}) has no chapters to play", book.title);
    }

    // The spoken-word profile resolved with the book's per-book overrides
    // (speed / Smart Speed / Voice Boost), spec §6.3.
    let profile = resolve_book_profile(playback.as_ref());
    let item = build_book_item(book_id, &chapters, &root, profile)
        .context("the book resolved to no playable file")?;
    let segments = item.segments.len();

    // Resume to the absolute book position (spec §6.4) when asked, unless the
    // book already finished.
    let resume_pos = resume
        .then_some(playback.as_ref())
        .flatten()
        .filter(|p| !p.finished && p.position > 0.0)
        .map(|p| p.position);

    let player = conservatory_core::player::spawn(worker.clone(), runtime.handle().clone())
        .context("starting the player engine")?;
    if let Ok(conn) = pool.open() {
        if let Ok(eq) = conservatory_core::db::get_eq_state(&conn) {
            player.set_eq(eq);
        }
        if let Ok(audio) = conservatory_core::db::get_audio_state(&conn) {
            player.set_dsp(audio.dsp);
        }
    }
    println!(
        "Playing \"{}\" ({segments} file(s), {} chapter(s)).",
        book.title,
        item.chapters.len()
    );
    player.play_queue(vec![item], 0);
    if let Some(pos) = resume_pos {
        player.seek(pos);
        println!("Resuming at {pos:.1}s.");
    }
    if let Some(mode) = sleep_mode {
        player.set_sleep_timer(Some(mode));
        println!("Sleep timer armed ({mode:?}).");
    }

    // Poll the snapshot until the book finishes (or a duration sleep timer fires),
    // printing the current chapter as it advances.
    let mut last_chapter: Option<usize> = None;
    loop {
        let snap = player.snapshot();
        if snap.current_chapter != last_chapter {
            if let Some(ch) = snap.current_chapter {
                println!(
                    "  > chapter {}/{}  ({:.0}s of {:.0}s)",
                    ch + 1,
                    snap.chapter_count,
                    snap.position,
                    snap.duration.unwrap_or(0.0),
                );
            }
            last_chapter = snap.current_chapter;
        }
        if snap.ended {
            break;
        }
        if snap.sleep.is_some_and(|s| s.fired) {
            println!("Sleep timer elapsed; playback paused.");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }

    player.shutdown();
    let _ = runtime.block_on(worker.shutdown_ack());
    println!("Done.");
    Ok(())
}

/// Set a book's per-book playback overrides (Phase 7c-ii). Reads the current
/// `book_playback` row (or starts from a default), applies only the provided
/// flags, and upserts it (preserving position / finished), then prints the
/// resolved profile.
#[cfg(feature = "audiobooks")]
async fn run_audiobook_settings(
    db: PathBuf,
    book_id: i64,
    speed: Option<f64>,
    smart_speed: Option<bool>,
    voice_boost: Option<bool>,
) -> Result<()> {
    use conservatory_core::db::{BookPlayback, get_book, get_book_playback};
    use conservatory_core::resolve_book_profile;

    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;

    let mut pb = {
        let conn = pool.open().context("opening pool connection")?;
        get_book(&conn, book_id)
            .context("reading the book")?
            .with_context(|| format!("no book with id {book_id}"))?;
        get_book_playback(&conn, book_id)
            .context("reading playback")?
            .unwrap_or(BookPlayback {
                book_id,
                position: 0.0,
                finished: false,
                last_played: None,
                speed: None,
                smart_speed: None,
                voice_boost: None,
            })
    };
    if let Some(s) = speed {
        pb.speed = Some(s);
    }
    if let Some(b) = smart_speed {
        pb.smart_speed = Some(b);
    }
    if let Some(b) = voice_boost {
        pb.voice_boost = Some(b);
    }
    worker
        .upsert_book_playback(pb.clone())
        .await
        .context("saving playback overrides")?;

    let profile = resolve_book_profile(Some(&pb));
    println!(
        "book {book_id}: speed {:.2}x, smart_speed {}, voice_boost {}",
        profile.speed, profile.smart_speed, profile.voice_boost
    );
    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

/// Project a shelf row into the search grammar's [`SearchItem`] (the CLI twin of
/// the GUI `book_query::book_item`): split the comma-joined people back into the
/// multi-valued candidates, expose runtime as `duration:`.
#[cfg(feature = "audiobooks")]
fn book_search_item(r: &conservatory_core::db::BookListRow) -> SearchItem {
    let split = |d: &Option<String>| -> Vec<String> {
        d.as_deref()
            .map(|s| {
                s.split(", ")
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    SearchItem {
        title: r.title.clone(),
        authors: split(&r.author_display),
        narrators: split(&r.narrator_display),
        series: r.series_name.clone(),
        year: r.year,
        rating: r.rating,
        starred: r.starred,
        finished: r.finished,
        duration: (r.total_duration > 0.0).then_some(r.total_duration),
        ..SearchItem::default()
    }
}

#[cfg(feature = "audiobooks")]
fn print_book_rows(rows: &[conservatory_core::db::BookListRow], format: Format) {
    // h:mm of a duration in seconds, "-" when unknown (0).
    let dur = |secs: f64| -> String {
        if secs <= 0.0 {
            return "-".to_string();
        }
        let total = secs as u64;
        format!("{}:{:02}", total / 3600, (total % 3600) / 60)
    };
    let series = |r: &conservatory_core::db::BookListRow| match (&r.series_name, r.series_sequence)
    {
        (Some(s), Some(n)) => format!("{s} #{n}"),
        (Some(s), None) => s.clone(),
        _ => String::new(),
    };

    match format {
        Format::Tsv => {
            println!("id\ttitle\tauthor\tnarrator\tseries\tstate\tprogress\tduration\tstarred");
            for r in rows {
                let pct = progress_pct(r);
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{pct}%\t{}\t{}",
                    r.id,
                    r.title,
                    r.author_display.as_deref().unwrap_or(""),
                    r.narrator_display.as_deref().unwrap_or(""),
                    series(r),
                    r.state().as_str(),
                    dur(r.total_duration),
                    r.starred,
                );
            }
        }
        Format::Json => {
            print!("[");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    print!(",");
                }
                print!(
                    "{{\"id\":{},\"title\":{:?},\"author\":{:?},\"narrator\":{:?},\"series\":{:?},\"state\":{:?},\"progress\":{},\"duration\":{:.1}}}",
                    r.id,
                    r.title,
                    r.author_display.as_deref().unwrap_or(""),
                    r.narrator_display.as_deref().unwrap_or(""),
                    series(r),
                    r.state().as_str(),
                    progress_pct(r),
                    r.total_duration,
                );
            }
            println!("]");
        }
        Format::Human => {
            if rows.is_empty() {
                println!("no audiobooks (import one with `audiobook import`)");
                return;
            }
            for r in rows {
                let mut line = format!("{:>4}  [{}]  {}", r.id, r.state().as_str(), r.title);
                if let Some(a) = &r.author_display {
                    line.push_str(&format!(" — {a}"));
                }
                let s = series(r);
                if !s.is_empty() {
                    line.push_str(&format!("  ({s})"));
                }
                if r.state() == conservatory_core::db::BookState::InProgress {
                    line.push_str(&format!("  {}%", progress_pct(r)));
                }
                println!("{line}");
            }
            println!("\n{} book(s)", rows.len());
        }
    }
}

/// Whole-percent progress through a book (0 when the total duration is unknown).
#[cfg(feature = "audiobooks")]
fn progress_pct(r: &conservatory_core::db::BookListRow) -> u32 {
    if r.finished {
        return 100;
    }
    if r.total_duration <= 0.0 {
        return 0;
    }
    ((r.position / r.total_duration) * 100.0).clamp(0.0, 100.0) as u32
}

fn eq(action: EqAction) -> Result<()> {
    match action {
        EqAction::Show { db } => eq_show(db),
        EqAction::Set { db, band, gain } => block_on(run_eq_set(db, band, gain)),
        EqAction::Preset { action } => match action {
            EqPresetAction::List { db } => eq_preset_list(db),
            EqPresetAction::Save { db, name } => block_on(run_eq_preset_save(db, name)),
            EqPresetAction::Load { db, name } => block_on(run_eq_preset_load(db, name)),
            EqPresetAction::Delete { db, name } => block_on(run_eq_preset_delete(db, name)),
        },
    }
}

fn eq_show(db: PathBuf) -> Result<()> {
    use conservatory_core::db::{EQ_CENTRES, get_eq_state};
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let state = get_eq_state(&conn).context("reading EQ state")?;
    println!("preset:  {}", state.preset.as_deref().unwrap_or("(custom)"));
    for (i, (centre, gain)) in EQ_CENTRES.iter().zip(state.bands.iter()).enumerate() {
        println!("  [{i}] {centre:>6} Hz  {gain:+.1} dB");
    }
    let chain = conservatory_core::eq_stage(&state);
    println!(
        "af @eq:  {}",
        chain.as_deref().unwrap_or("(flat — no stage)")
    );
    Ok(())
}

async fn run_eq_set(db: PathBuf, band: usize, gain: f64) -> Result<()> {
    use conservatory_core::db::{EQ_BAND_COUNT, get_eq_state};
    anyhow::ensure!(
        band < EQ_BAND_COUNT,
        "band must be 0..={}",
        EQ_BAND_COUNT - 1
    );
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut state = {
        let conn = pool.open().context("opening pool connection")?;
        get_eq_state(&conn).context("reading EQ state")?
    };
    let clamped = gain.clamp(-24.0, 24.0);
    state.bands[band] = clamped;
    state.preset = None; // a manual edit detaches from any preset
    worker
        .set_eq_state(state)
        .await
        .context("saving EQ state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Set band {band} to {clamped:+.1} dB.");
    Ok(())
}

fn eq_preset_list(db: PathBuf) -> Result<()> {
    use conservatory_core::db::list_eq_presets;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    for p in list_eq_presets(&conn).context("listing presets")? {
        let bands = p
            .bands
            .iter()
            .map(|g| format!("{g:+}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{:<16} {bands}", p.name);
    }
    Ok(())
}

async fn run_eq_preset_save(db: PathBuf, name: String) -> Result<()> {
    use conservatory_core::db::get_eq_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut state = {
        let conn = pool.open().context("opening pool connection")?;
        get_eq_state(&conn).context("reading EQ state")?
    };
    worker
        .save_eq_preset(name.clone(), state.bands)
        .await
        .context("saving preset")?;
    // The active EQ is now exactly this preset.
    state.preset = Some(name.clone());
    worker
        .set_eq_state(state)
        .await
        .context("updating EQ state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Saved preset {name:?}.");
    Ok(())
}

async fn run_eq_preset_load(db: PathBuf, name: String) -> Result<()> {
    use conservatory_core::db::{EqState, get_eq_preset};
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let bands = {
        let conn = pool.open().context("opening pool connection")?;
        get_eq_preset(&conn, &name)
            .context("reading preset")?
            .ok_or_else(|| anyhow::anyhow!("no preset named {name:?}"))?
    };
    worker
        .set_eq_state(EqState {
            bands,
            preset: Some(name.clone()),
        })
        .await
        .context("applying preset")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Loaded preset {name:?}.");
    Ok(())
}

async fn run_eq_preset_delete(db: PathBuf, name: String) -> Result<()> {
    anyhow::ensure!(name != "Flat", "the Flat preset cannot be deleted");
    let worker = spawn_worker(db).context("spawning worker")?;
    worker
        .delete_eq_preset(name.clone())
        .await
        .context("deleting preset")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Deleted preset {name:?}.");
    Ok(())
}

fn dsp(action: DspAction) -> Result<()> {
    match action {
        DspAction::Show { db } => dsp_show(db),
        DspAction::Comp {
            db,
            state,
            threshold,
            ratio,
            attack,
            release,
        } => block_on(run_dsp_comp(db, state, threshold, ratio, attack, release)),
        DspAction::Limiter { db, state, ceiling } => block_on(run_dsp_limiter(db, state, ceiling)),
        DspAction::Leveler {
            db,
            state,
            target,
            gausssize,
        } => block_on(run_dsp_leveler(db, state, target, gausssize)),
    }
}

fn dsp_show(db: PathBuf) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    use conservatory_core::player::{comp_stage, leveler_stage, limiter_stage};
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let dsp = get_audio_state(&conn).context("reading audio state")?.dsp;
    println!(
        "compressor: {}  (threshold={:+} dB ratio={}:1 attack={} ms release={} ms)",
        on_off(dsp.comp.enabled),
        dsp.comp.settings.threshold_db,
        dsp.comp.settings.ratio,
        dsp.comp.settings.attack_ms,
        dsp.comp.settings.release_ms,
    );
    println!(
        "limiter:    {}  (ceiling={:+} dB)",
        on_off(dsp.limiter.enabled),
        dsp.limiter.settings.ceiling_db,
    );
    println!(
        "leveler:    {}  (target_peak={} gausssize={})",
        on_off(dsp.leveler.enabled),
        dsp.leveler.settings.target_peak,
        dsp.leveler.settings.gausssize,
    );
    for stage in [
        comp_stage(&dsp.comp),
        limiter_stage(&dsp.limiter),
        leveler_stage(&dsp.leveler),
    ]
    .into_iter()
    .flatten()
    {
        println!("af:         {stage}");
    }
    Ok(())
}

async fn run_dsp_comp(
    db: PathBuf,
    state: OnOff,
    threshold: Option<f64>,
    ratio: Option<f64>,
    attack: Option<f64>,
    release: Option<f64>,
) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut audio = {
        let conn = pool.open().context("opening pool connection")?;
        get_audio_state(&conn).context("reading audio state")?
    };
    audio.dsp.comp.enabled = state.enabled();
    if let Some(v) = threshold {
        audio.dsp.comp.settings.threshold_db = v;
    }
    if let Some(v) = ratio {
        audio.dsp.comp.settings.ratio = v;
    }
    if let Some(v) = attack {
        audio.dsp.comp.settings.attack_ms = v;
    }
    if let Some(v) = release {
        audio.dsp.comp.settings.release_ms = v;
    }
    worker
        .set_audio_state(audio)
        .await
        .context("saving audio state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Compressor {}.", on_off(state.enabled()));
    Ok(())
}

async fn run_dsp_limiter(db: PathBuf, state: OnOff, ceiling: Option<f64>) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut audio = {
        let conn = pool.open().context("opening pool connection")?;
        get_audio_state(&conn).context("reading audio state")?
    };
    audio.dsp.limiter.enabled = state.enabled();
    if let Some(v) = ceiling {
        audio.dsp.limiter.settings.ceiling_db = v;
    }
    worker
        .set_audio_state(audio)
        .await
        .context("saving audio state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Limiter {}.", on_off(state.enabled()));
    Ok(())
}

async fn run_dsp_leveler(
    db: PathBuf,
    state: OnOff,
    target: Option<f64>,
    gausssize: Option<u32>,
) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut audio = {
        let conn = pool.open().context("opening pool connection")?;
        get_audio_state(&conn).context("reading audio state")?
    };
    audio.dsp.leveler.enabled = state.enabled();
    if let Some(v) = target {
        audio.dsp.leveler.settings.target_peak = v;
    }
    if let Some(v) = gausssize {
        audio.dsp.leveler.settings.gausssize = v;
    }
    worker
        .set_audio_state(audio)
        .await
        .context("saving audio state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Leveler {}.", on_off(state.enabled()));
    Ok(())
}

fn output(action: OutputAction) -> Result<()> {
    match action {
        OutputAction::Show { db } => output_show(db),
        OutputAction::Backend { db, backend } => block_on(run_output_backend(db, backend)),
        OutputAction::Resampler { db, quality } => block_on(run_output_resampler(db, quality)),
    }
}

fn output_show(db: PathBuf) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let audio = get_audio_state(&conn).context("reading audio state")?;
    println!("backend:   {}", audio.output_backend);
    println!("resampler: {}", audio.resampler.as_str());
    Ok(())
}

async fn run_output_backend(db: PathBuf, backend: BackendArg) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut audio = {
        let conn = pool.open().context("opening pool connection")?;
        get_audio_state(&conn).context("reading audio state")?
    };
    audio.output_backend = backend.as_str().to_string();
    worker
        .set_audio_state(audio)
        .await
        .context("saving audio state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Output backend set to {}.", backend.as_str());
    Ok(())
}

async fn run_output_resampler(db: PathBuf, quality: ResamplerArg) -> Result<()> {
    use conservatory_core::db::get_audio_state;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let mut audio = {
        let conn = pool.open().context("opening pool connection")?;
        get_audio_state(&conn).context("reading audio state")?
    };
    audio.resampler = quality.to_core();
    worker
        .set_audio_state(audio)
        .await
        .context("saving audio state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Resampler quality set to {}.", quality.to_core().as_str());
    Ok(())
}

fn debug_dsp(db: PathBuf, track_id: Option<i64>) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;

    // The given track, or the first in the library.
    let id = match track_id {
        Some(id) => id,
        None => track_render_rows(&conn)
            .context("reading tracks")?
            .first()
            .map(|r| r.track_id)
            .ok_or_else(|| anyhow::anyhow!("the library has no tracks"))?,
    };
    let track = conservatory_core::db::get_track(&conn, id)
        .context("looking up track")?
        .ok_or_else(|| anyhow::anyhow!("no track with id {id}"))?;

    let cfg = PlaybackConfig::default();
    let profile = resolve_music_profile(&track, &cfg);
    let eq = conservatory_core::db::get_eq_state(&conn).context("reading EQ state")?;
    let audio = conservatory_core::db::get_audio_state(&conn).context("reading audio state")?;
    let dsp = &audio.dsp;
    let chain = build_af_chain(&profile, &eq, dsp);

    println!("track:        {} {}", track.id, track.title);
    println!(
        "replaygain:   mode={} (track={} album={})",
        cfg.replaygain.as_str(),
        track
            .replaygain_track
            .map_or("none".to_string(), |g| format!("{g} dB")),
        track
            .replaygain_album
            .map_or("none".to_string(), |g| format!("{g} dB")),
    );
    println!(
        "  preamp={:+} dB  clip={}  -> net={}",
        cfg.replaygain_preamp,
        cfg.replaygain_clip,
        profile
            .replaygain_db
            .map_or("off".to_string(), |g| format!("{g} dB")),
    );
    println!(
        "gapless:      {}",
        if profile.gapless { "weak" } else { "no" }
    );
    println!(
        "speed:        {}  pitch_correction={}",
        profile.speed, profile.pitch_correction
    );
    println!("dsp:");
    println!(
        "  compressor: {}  (threshold={:+} dB ratio={}:1 attack={} ms release={} ms)",
        on_off(dsp.comp.enabled),
        dsp.comp.settings.threshold_db,
        dsp.comp.settings.ratio,
        dsp.comp.settings.attack_ms,
        dsp.comp.settings.release_ms,
    );
    println!(
        "  limiter:    {}  (ceiling={:+} dB)",
        on_off(dsp.limiter.enabled),
        dsp.limiter.settings.ceiling_db,
    );
    println!(
        "  leveler:    {}  (target_peak={} gausssize={})",
        on_off(dsp.leveler.enabled),
        dsp.leveler.settings.target_peak,
        dsp.leveler.settings.gausssize,
    );
    println!(
        "output:       backend={}  resampler={}",
        audio.output_backend,
        audio.resampler.as_str(),
    );
    println!(
        "af chain:     {}",
        if chain.is_empty() { "(empty)" } else { &chain }
    );
    Ok(())
}

/// "on" / "off" for a module-enabled flag (the `debug-dsp` / `dsp show` surface).
fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn debug_paths(db: PathBuf) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let rows = track_render_rows(&conn).context("reading track render rows")?;

    let template = PathTemplate::default_music();
    let mut paths = Vec::with_capacity(rows.len());
    for row in &rows {
        let fields = TrackFields {
            shelf_genre: row.shelf_genre.as_deref(),
            albumartist: row.album_artist_sort.as_deref(),
            album: row.album.as_deref(),
            year: row.year,
            track_no: row.track_no,
            disc_no: row.disc_no,
            title: Some(row.title.as_str()),
            artist: row.track_artist.as_deref(),
            ext: row.format.as_deref(),
        };
        let path = template.render(&fields);
        println!("{:>6}  {}", row.track_id, path.display());
        paths.push(path);
    }

    let collisions = find_collisions(&paths);
    println!(
        "\n{} tracks, {} colliding path(s)",
        rows.len(),
        collisions.len()
    );
    for (path, idx) in &collisions {
        println!("  collision: {} ({} tracks)", path.display(), idx.len());
    }
    Ok(())
}

fn debug_shelf_genre(db: PathBuf) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let vocab = GenreVocab::load(&conn).context("loading genre vocabulary")?;

    let albums = conservatory_core::db::list_albums(&conn).context("listing albums")?;
    let mut mismatches = 0;
    for album in &albums {
        let derived = resolve_album(&conn, album.id, &vocab).context("resolving shelf genre")?;
        let stored = album.shelf_genre.as_deref().unwrap_or("-");
        let flag = if stored == derived { " " } else { "*" };
        if stored != derived {
            mismatches += 1;
        }
        println!(
            "{flag} {:>4}  stored={stored:<16} derived={derived}",
            album.id
        );
    }
    println!(
        "\n{} albums, {} differ from stored (*)",
        albums.len(),
        mismatches
    );
    Ok(())
}

fn import(db: PathBuf, source: PathBuf, root: PathBuf, r#move: bool, format: Format) -> Result<()> {
    block_on(run_import(db, source, root, r#move, format))
}

async fn run_import(
    db: PathBuf,
    source: PathBuf,
    root: PathBuf,
    r#move: bool,
    format: Format,
) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    // Heal any job interrupted by a previous crash before starting a new one.
    mover::recover(&worker, &pool).await.context("recovery")?;

    let opts = ImportOptions {
        library_root: root,
        mode: if r#move {
            MoveMode::Move
        } else {
            MoveMode::Copy
        },
    };
    let report = import_folder(&worker, &pool, &source, &opts)
        .await
        .context("import")?;
    worker.shutdown_ack().await.context("shutdown ack")?;

    print_import_report(&report, format);
    if !report.conflicts.is_empty() {
        anyhow::bail!(
            "import refused: {} conflict(s); nothing imported",
            report.conflicts.len()
        );
    }
    Ok(())
}

fn print_import_report(r: &ImportReport, format: Format) {
    let job = r.job_id.map(|j| j.to_string());
    match format {
        Format::Json => println!(
            "{{\"files_scanned\":{},\"skipped\":{},\"artists\":{},\"albums\":{},\"tracks\":{},\"job_id\":{},\"conflicts\":{}}}",
            r.files_scanned,
            r.skipped_unreadable,
            r.artists,
            r.albums,
            r.tracks,
            job.as_deref().unwrap_or("null"),
            r.conflicts.len(),
        ),
        Format::Tsv => {
            println!("files_scanned\tskipped\tartists\talbums\ttracks\tjob_id\tconflicts");
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                r.files_scanned,
                r.skipped_unreadable,
                r.artists,
                r.albums,
                r.tracks,
                job.as_deref().unwrap_or(""),
                r.conflicts.len(),
            );
        }
        Format::Human => {
            print!("scanned {} file(s)", r.files_scanned);
            if r.skipped_unreadable > 0 {
                print!(", {} unreadable", r.skipped_unreadable);
            }
            println!();
            match r.job_id {
                Some(j) => println!(
                    "imported {} track(s) across {} album(s) / {} artist(s) (job {j})",
                    r.tracks, r.albums, r.artists
                ),
                None if !r.conflicts.is_empty() => {
                    println!(
                        "refused: {} conflict(s); nothing imported",
                        r.conflicts.len()
                    );
                    for c in &r.conflicts {
                        println!("  {c:?}");
                    }
                }
                None => println!("nothing to import"),
            }
        }
    }
}

fn organize(
    db: PathBuf,
    root: PathBuf,
    apply: bool,
    copy: bool,
    undo: Option<i64>,
    format: Format,
) -> Result<()> {
    block_on(run_organize(db, root, apply, copy, undo, format))
}

async fn run_organize(
    db: PathBuf,
    root: PathBuf,
    apply: bool,
    copy: bool,
    undo: Option<i64>,
    format: Format,
) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;

    if let Some(job_id) = undo {
        mover::undo(&worker, &pool, job_id)
            .await
            .with_context(|| format!("undoing job {job_id}"))?;
        println!("undid job {job_id}");
        worker.shutdown_ack().await.context("shutdown ack")?;
        return Ok(());
    }

    // Build the operations: src = current managed path, dst = re-rendered target
    // (the shared core builder, so the render mapping lives in one place).
    let ops = {
        let conn = pool.open().context("opening pool connection")?;
        organize_ops(
            &track_render_rows(&conn).context("reading track render rows")?,
            &root,
            None,
        )
    };

    if apply {
        let recovered = mover::recover(&worker, &pool).await.context("recovery")?;
        if recovered > 0 {
            println!("recovered {recovered} interrupted job(s)");
        }
        let mode = if copy { MoveMode::Copy } else { MoveMode::Move };
        let count = ops.len();
        let job_id = mover::apply(
            &worker,
            &pool,
            MoveKind::Organize,
            mode,
            &root,
            now_secs(),
            ops,
        )
        .await
        .context("applying move job")?;
        // Covers follow their albums after the move (Phase 5d, idempotent).
        let covers = resync_album_covers(&worker, &pool, &root)
            .await
            .context("resyncing covers")?;
        match format {
            Format::Json => {
                println!("{{\"job_id\":{job_id},\"tracks\":{count},\"covers\":{covers}}}")
            }
            _ => println!(
                "applied job {job_id}: {count} track(s) organized under {}{}",
                root.display(),
                if covers > 0 {
                    format!(" ({covers} cover(s) moved)")
                } else {
                    String::new()
                }
            ),
        }
    } else {
        let preview = mover::plan(ops);
        match format {
            Format::Json => println!(
                "{{\"to_move\":{},\"in_place\":{},\"conflicts\":{}}}",
                preview.ops.len(),
                preview.skipped,
                preview.conflicts.len(),
            ),
            Format::Tsv => {
                for op in &preview.ops {
                    println!("{}\t{}", op.src.display(), op.dst.display());
                }
            }
            Format::Human => {
                for op in &preview.ops {
                    println!("{}  ->  {}", op.src.display(), op.dst.display());
                }
                println!(
                    "\n{} to move, {} already in place, {} conflict(s)",
                    preview.ops.len(),
                    preview.skipped,
                    preview.conflicts.len()
                );
                for conflict in &preview.conflicts {
                    println!("  conflict: {conflict:?}");
                }
                println!("(dry-run; pass --apply to execute)");
            }
        }
    }

    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

fn shelf_genre_set(db: PathBuf, album_id: i64, value: String) -> Result<()> {
    block_on(run_shelf_genre_set(db, album_id, value))
}

async fn run_shelf_genre_set(db: PathBuf, album_id: i64, value: String) -> Result<()> {
    let worker = spawn_worker(db).context("spawning worker")?;
    worker
        .set_album_shelf_genre(album_id, value.clone())
        .await
        .with_context(|| format!("setting shelf genre for album {album_id}"))?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("album {album_id} shelf genre set to {value:?}; run `organize` to move it");
    Ok(())
}

fn tag(action: TagAction) -> Result<()> {
    match action {
        TagAction::Set {
            db,
            query,
            assignments,
            root,
            apply,
        } => block_on(run_tag_set(db, query, assignments, root, apply)),
        TagAction::Replace {
            db,
            query,
            field,
            find,
            replace,
            root,
            apply,
        } => block_on(run_tag_replace(
            db, query, field, find, replace, root, apply,
        )),
    }
}

/// Resolve a search expression to the set of matching track ids (the dual SQL /
/// eval path the `search` verb uses, membership only).
fn resolve_selector(pool: &ReadPool, query: &str) -> Result<std::collections::HashSet<i64>> {
    let conn = pool.open().context("opening pool connection")?;
    let today = Utc::now().date_naive();
    let parsed = parse(query);
    for w in &parsed.warnings {
        eprintln!("warning: {w}");
    }
    let ids = match try_translate(&parsed.expr, today) {
        Some(clause) => {
            let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
            search_track_ids(&conn, &clause.sql, &params)
                .context("running selector SQL")?
                .into_iter()
                .collect()
        }
        None => search_rows(&conn)
            .context("loading rows")?
            .into_iter()
            .filter(|r| conservatory_search::evaluate(&parsed.expr, &to_item(r), today))
            .map(|r| r.track_id)
            .collect(),
    };
    Ok(ids)
}

/// The matched track ids and their distinct album ids, in a stable order.
fn matched_tracks_and_albums(
    pool: &ReadPool,
    ids: &std::collections::HashSet<i64>,
) -> Result<(Vec<i64>, Vec<i64>)> {
    let conn = pool.open().context("opening pool connection")?;
    let rows = track_render_rows(&conn).context("reading render rows")?;
    let mut tracks = Vec::new();
    let mut albums = Vec::new();
    for r in &rows {
        if ids.contains(&r.track_id) {
            tracks.push(r.track_id);
            if let Some(a) = r.album_id
                && !albums.contains(&a)
            {
                albums.push(a);
            }
        }
    }
    Ok((tracks, albums))
}

async fn run_tag_set(
    db: PathBuf,
    query: String,
    assignment_strs: Vec<String>,
    root: Option<PathBuf>,
    apply: bool,
) -> Result<()> {
    let assignments: Vec<Assignment> = assignment_strs
        .iter()
        .map(|s| parse_assignment(s).map_err(|e| anyhow::anyhow!(e.to_string())))
        .collect::<Result<_>>()?;

    // Validate up front: a path-affecting edit needs the root to move files. Fail
    // before any DB write so the DB and the tree never diverge (spec §3.5).
    if any_path_affecting(&assignments) && root.is_none() {
        anyhow::bail!("a path-affecting field changed; pass --root <root> to move the files");
    }

    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    mover::recover(&worker, &pool).await.context("recovery")?;

    let ids = resolve_selector(&pool, &query)?;
    if ids.is_empty() {
        println!("no tracks match {query:?}");
        worker.shutdown_ack().await.context("shutdown ack")?;
        return Ok(());
    }
    let (track_ids, albums) = matched_tracks_and_albums(&pool, &ids)?;

    let track_edit = build_track_edit(&assignments);
    let album_edit = build_album_edit(&assignments);
    let genres = genres_assignment(&assignments);

    if !track_edit.is_empty() {
        for &tid in &track_ids {
            worker
                .update_track(tid, track_edit.clone())
                .await
                .context("updating track")?;
        }
    }
    if let Some(g) = &genres {
        for &tid in &track_ids {
            worker
                .set_track_genres(tid, g.clone())
                .await
                .context("setting genres")?;
        }
    }
    if !album_edit.is_empty() {
        for &aid in &albums {
            worker
                .update_album(aid, album_edit.clone())
                .await
                .context("updating album")?;
        }
    }
    println!(
        "edited {} track(s) across {} album(s)",
        track_ids.len(),
        albums.len()
    );

    if any_path_affecting(&assignments) {
        let root = root.ok_or_else(|| {
            anyhow::anyhow!("a path-affecting field changed; pass --root <root> to move files")
        })?;
        scoped_organize(&worker, &pool, &root, &albums, apply).await?;
    }

    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

async fn run_tag_replace(
    db: PathBuf,
    query: String,
    field_str: String,
    find: String,
    replace: String,
    root: Option<PathBuf>,
    apply: bool,
) -> Result<()> {
    let field =
        Field::parse(&field_str).ok_or_else(|| anyhow::anyhow!("unknown field {field_str:?}"))?;

    // Validate up front (as `tag set` does): a path-affecting field needs --root.
    if field.is_path_affecting() && root.is_none() {
        anyhow::bail!("a path-affecting field changed; pass --root <root> to move the files");
    }

    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    mover::recover(&worker, &pool).await.context("recovery")?;

    let ids = resolve_selector(&pool, &query)?;
    if ids.is_empty() {
        println!("no tracks match {query:?}");
        worker.shutdown_ack().await.context("shutdown ack")?;
        return Ok(());
    }
    let rows = {
        let conn = pool.open().context("opening pool connection")?;
        track_render_rows(&conn).context("reading render rows")?
    };
    let matched: Vec<_> = rows.iter().filter(|r| ids.contains(&r.track_id)).collect();

    let mut edited = 0usize;
    let mut albums: Vec<i64> = Vec::new();
    match field {
        Field::Title => {
            for r in &matched {
                let nv = replace_in(&r.title, &find, &replace);
                if nv != r.title {
                    worker
                        .update_track(
                            r.track_id,
                            TrackEdit {
                                title: Some(nv),
                                ..Default::default()
                            },
                        )
                        .await
                        .context("updating track")?;
                    edited += 1;
                }
            }
        }
        Field::Artist => {
            for r in &matched {
                if let Some(cur) = &r.track_artist {
                    let nv = replace_in(cur, &find, &replace);
                    if &nv != cur {
                        worker
                            .update_track(
                                r.track_id,
                                TrackEdit {
                                    artist: Some(nv),
                                    ..Default::default()
                                },
                            )
                            .await
                            .context("updating track")?;
                        edited += 1;
                    }
                }
            }
        }
        Field::Album => {
            let mut seen = std::collections::HashSet::new();
            for r in &matched {
                let Some(aid) = r.album_id else { continue };
                if !seen.insert(aid) {
                    continue;
                }
                if let Some(cur) = &r.album {
                    let nv = replace_in(cur, &find, &replace);
                    if &nv != cur {
                        worker
                            .update_album(
                                aid,
                                AlbumEdit {
                                    title: Some(nv),
                                    ..Default::default()
                                },
                            )
                            .await
                            .context("updating album")?;
                        edited += 1;
                        albums.push(aid);
                    }
                }
            }
        }
        Field::ShelfGenre => {
            let mut seen = std::collections::HashSet::new();
            for r in &matched {
                let Some(aid) = r.album_id else { continue };
                if !seen.insert(aid) {
                    continue;
                }
                if let Some(cur) = &r.shelf_genre {
                    let nv = replace_in(cur, &find, &replace);
                    if &nv != cur {
                        worker
                            .update_album(
                                aid,
                                AlbumEdit {
                                    shelf_genre: Some(nv),
                                    ..Default::default()
                                },
                            )
                            .await
                            .context("updating album")?;
                        edited += 1;
                        albums.push(aid);
                    }
                }
            }
        }
        _ => anyhow::bail!("search-and-replace supports title | artist | album | shelfgenre"),
    }
    println!("replaced in {edited} item(s)");

    if field.is_path_affecting() && !albums.is_empty() {
        let root = root.ok_or_else(|| {
            anyhow::anyhow!("a path-affecting field changed; pass --root <root> to move files")
        })?;
        scoped_organize(&worker, &pool, &root, &albums, apply).await?;
    }

    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

/// Re-render the given albums' tracks and move files to match (the `organize`
/// flow scoped to the albums a tag edit touched). Dry-run unless `apply`.
async fn scoped_organize(
    worker: &conservatory_core::db::WorkerHandle,
    pool: &ReadPool,
    root: &Path,
    albums: &[i64],
    apply: bool,
) -> Result<()> {
    let ops = {
        let conn = pool.open().context("opening pool connection")?;
        organize_ops(
            &track_render_rows(&conn).context("reading render rows")?,
            root,
            Some(albums),
        )
    };

    if apply {
        let count = ops.len();
        let job_id = mover::apply(
            worker,
            pool,
            MoveKind::Organize,
            MoveMode::Move,
            root,
            now_secs(),
            ops,
        )
        .await
        .context("applying move job")?;
        // Covers follow their albums after the move (Phase 5d, idempotent).
        let covers = resync_album_covers(worker, pool, root)
            .await
            .context("resyncing covers")?;
        println!(
            "applied move job {job_id}: {count} file(s) re-shelved{}",
            if covers > 0 {
                format!(" ({covers} cover(s) moved)")
            } else {
                String::new()
            }
        );
    } else {
        let preview = mover::plan(ops);
        println!(
            "{} to move, {} already in place, {} conflict(s) (dry-run; pass --apply to move)",
            preview.ops.len(),
            preview.skipped,
            preview.conflicts.len()
        );
        for op in &preview.ops {
            println!("  {}  ->  {}", op.src.display(), op.dst.display());
        }
    }
    Ok(())
}

/// Human-readable "field: old -> new" lines for the fields a write-back would
/// change (the dry-run preview), comparing the file's current tags to the DB.
fn diff_fields(cur: &TrackDraft, target: &TagWrite) -> Vec<String> {
    let mut diffs = Vec::new();
    let opt = |o: &Option<String>| o.clone().unwrap_or_default();
    if cur.title.as_deref() != Some(target.title.as_str()) {
        diffs.push(format!(
            "title: {:?} -> {:?}",
            opt(&cur.title),
            target.title
        ));
    }
    if cur.artist != target.track_artist {
        diffs.push(format!(
            "artist: {:?} -> {:?}",
            opt(&cur.artist),
            opt(&target.track_artist)
        ));
    }
    if cur.album_artist != target.album_artist {
        diffs.push(format!(
            "albumartist: {:?} -> {:?}",
            opt(&cur.album_artist),
            opt(&target.album_artist)
        ));
    }
    if cur.album != target.album {
        diffs.push(format!(
            "album: {:?} -> {:?}",
            opt(&cur.album),
            opt(&target.album)
        ));
    }
    if cur.year != target.year {
        diffs.push(format!("year: {:?} -> {:?}", cur.year, target.year));
    }
    if cur.track_no != target.track_no {
        diffs.push(format!(
            "track: {:?} -> {:?}",
            cur.track_no, target.track_no
        ));
    }
    if cur.disc_no != target.disc_no {
        diffs.push(format!("disc: {:?} -> {:?}", cur.disc_no, target.disc_no));
    }
    // Genres are a set: compare order-insensitively so a mere reorder is not a
    // change (the embedded write is deterministically ordered anyway).
    let (mut cur_g, mut tgt_g) = (cur.genres.clone(), target.genres.clone());
    cur_g.sort();
    tgt_g.sort();
    if cur_g != tgt_g {
        diffs.push(format!("genres: {:?} -> {:?}", cur.genres, target.genres));
    }
    diffs
}

fn embed_tags(db: PathBuf, query: String, root: PathBuf, apply: bool) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let ids = resolve_selector(&pool, &query)?;
    if ids.is_empty() {
        println!("no tracks match {query:?}");
        return Ok(());
    }
    let ids: Vec<i64> = ids.into_iter().collect();
    let rows = {
        let conn = pool.open().context("opening pool connection")?;
        writeback_rows(&conn, &ids).context("reading write-back rows")?
    };

    let (mut changed, mut written, mut errors) = (0usize, 0usize, 0usize);
    for r in &rows {
        let path = root.join(&r.file_path);
        let target = TagWrite::from(r);
        // Read the current tags and diff: a file already in sync is skipped, so
        // re-running embed-tags is idempotent and never churns unchanged files.
        let cur = match read_track(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  ! {}: {e}", path.display());
                errors += 1;
                continue;
            }
        };
        let diffs = diff_fields(&cur, &target);
        if diffs.is_empty() {
            continue;
        }
        if apply {
            match write_track_tags(&path, &target) {
                Ok(()) => written += 1,
                Err(e) => {
                    eprintln!("  ! {}: {e}", path.display());
                    errors += 1;
                }
            }
        } else {
            changed += 1;
            println!("{}", path.display());
            for d in &diffs {
                println!("    {d}");
            }
        }
    }
    let err_note = if errors > 0 {
        format!(", {errors} error(s)")
    } else {
        String::new()
    };
    if apply {
        println!(
            "wrote tags to {written} file(s) ({} already in sync){err_note}",
            rows.len() - written - errors
        );
    } else {
        println!(
            "{changed} of {} file(s) would change (dry-run; pass --apply to write){err_note}",
            rows.len()
        );
    }
    Ok(())
}

async fn run_replaygain_scan(
    db: PathBuf,
    query: String,
    root: PathBuf,
    apply: bool,
    target_lufs: f64,
) -> Result<()> {
    let pool = ReadPool::new(db.clone(), 3).context("opening read pool")?;
    let ids = resolve_selector(&pool, &query)?;
    if ids.is_empty() {
        println!("no tracks match {query:?}");
        return Ok(());
    }

    // Group the matched tracks by album (rsgain computes album gain per set).
    let rows = {
        let conn = pool.open().context("opening pool connection")?;
        track_render_rows(&conn).context("reading render rows")?
    };
    let mut by_album: std::collections::BTreeMap<Option<i64>, Vec<(i64, String)>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        if ids.contains(&r.track_id) {
            by_album
                .entry(r.album_id)
                .or_default()
                .push((r.track_id, r.file_path.clone()));
        }
    }

    if !apply {
        let albums = by_album.len();
        let tracks: usize = by_album.values().map(Vec::len).sum();
        for group in by_album.values() {
            let folder = group
                .first()
                .and_then(|(_, fp)| root.join(fp).parent().map(|p| p.display().to_string()))
                .unwrap_or_default();
            println!("{}\t{} track(s)", folder, group.len());
        }
        println!("{albums} album(s) / {tracks} track(s) would be scanned (dry-run; pass --apply)");
        return Ok(());
    }

    if !rsgain_available() {
        anyhow::bail!("rsgain not found on PATH; install it to scan ReplayGain");
    }
    let worker = spawn_worker(db).context("spawning worker")?;
    let mut scanned = 0usize;
    for group in by_album.values() {
        let files: Vec<PathBuf> = group.iter().map(|(_, fp)| root.join(fp)).collect();
        scan_album_files(&files, target_lufs).context("running rsgain")?;
        for (track_id, fp) in group {
            let (track_gain, album_gain) = replaygain_from_file(&root.join(fp))?;
            worker
                .set_track_replaygain(*track_id, track_gain, album_gain)
                .await
                .context("writing replaygain to the DB")?;
            scanned += 1;
        }
    }
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!(
        "scanned {scanned} track(s) across {} album(s)",
        by_album.len()
    );
    Ok(())
}

async fn run_set_cover(db: PathBuf, album_id: i64, image: PathBuf, root: PathBuf) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let album = {
        let conn = pool.open().context("opening pool connection")?;
        get_album(&conn, album_id)
            .context("reading album")?
            .ok_or_else(|| anyhow::anyhow!("no album with id {album_id}"))?
    };
    if album.folder_path.is_empty() {
        anyhow::bail!("album {album_id} has no managed folder yet; import/organize it first");
    }
    let bytes = std::fs::read(&image).with_context(|| format!("reading {image:?}"))?;
    let cover_path = sync_album_cover(
        &root,
        &album.folder_path,
        &bytes,
        album.cover_path.as_deref(),
    )?;
    let accent = compute_accent(&bytes).ok();
    worker
        .set_album_cover_path(album_id, Some(cover_path.clone()), accent)
        .await
        .context("recording the cover path")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("set cover for album {album_id}: {cover_path}");
    Ok(())
}

/// Resolve the queue rows into `PlayableItem`s the engine can play. Tracks and
/// episodes interleave in the one unified queue (spec §4.3); each kind resolves
/// its own source. `tracks.file_path` / a downloaded `episodes.audio_path` are
/// stored relative to the library root, so they are joined with `root`; an
/// undownloaded episode streams its `audio_url` (libmpv loads a URL as-is).
/// Rows whose source cannot be resolved are skipped.
fn resolve_queue_items(
    pool: &ReadPool,
    root: &Path,
    cfg: &PlaybackConfig,
) -> Result<Vec<PlayableItem>> {
    let conn = pool.open().context("opening pool connection")?;
    let mut items = Vec::new();
    for row in load_queue(&conn).context("loading queue")? {
        match row.kind {
            MediaKind::Track => {
                let Some(track_id) = row.track_id else {
                    continue;
                };
                if let Some(track) = get_track(&conn, track_id).context("looking up track")? {
                    items.push(PlayableItem {
                        track_id,
                        source: root.join(&track.file_path),
                        profile: resolve_music_profile(&track, cfg),
                        album_id: track.album_id,
                        kind: MediaKind::Track,
                        streaming: false,
                        chapters: [].into(),
                        segments: [].into(),
                    });
                }
            }
            MediaKind::Episode => {
                let Some(episode_id) = row.episode_id else {
                    continue;
                };
                let Some(ep) = get_episode(&conn, episode_id).context("looking up episode")? else {
                    continue;
                };
                let (source, streaming) = match (ep.audio_path.as_deref(), ep.audio_url.as_deref())
                {
                    (Some(p), _) => (root.join(p), false),
                    (None, Some(url)) => (PathBuf::from(url), true),
                    (None, None) => continue,
                };
                // Resolve the show's per-show overrides (speed) for the profile.
                let settings = get_show_settings(&conn, ep.show_id).context("show settings")?;
                // Attach the episode's chapters so chapter-skip works headless too.
                let chapters: Vec<conservatory_core::ChapterMark> =
                    conservatory_core::db::list_chapters(&conn, episode_id)
                        .context("looking up chapters")?
                        .into_iter()
                        .map(|c| conservatory_core::ChapterMark {
                            start_time: c.start_time,
                            title: c.title,
                        })
                        .collect();
                items.push(PlayableItem {
                    track_id: episode_id, // the queue item's id field carries the episode id
                    source,
                    profile: resolve_episode_profile(settings.as_ref()),
                    album_id: None,
                    kind: MediaKind::Episode,
                    streaming,
                    chapters: chapters.into(),
                    segments: [].into(),
                });
            }
            MediaKind::Audiobook => continue, // Phase 7
        }
    }
    Ok(items)
}

/// Parse a `--sleep` spec into a [`SleepMode`]: a positive number of minutes, or
/// `episode`/`item`/`track` (end of the current item), or `queue` (end of queue).
fn parse_sleep_spec(spec: &str) -> Result<SleepMode> {
    let s = spec.trim().to_ascii_lowercase();
    match s.as_str() {
        "episode" | "item" | "track" | "book" => Ok(SleepMode::EndOfItem),
        "queue" => Ok(SleepMode::EndOfQueue),
        _ => {
            let mins: f64 = s.parse().with_context(|| {
                format!("invalid --sleep value {spec:?} (minutes, episode, or queue)")
            })?;
            if mins > 0.0 {
                Ok(SleepMode::After(mins * 60.0))
            } else {
                anyhow::bail!("--sleep minutes must be positive")
            }
        }
    }
}

fn play(db: PathBuf, root: PathBuf, track_id: Option<i64>, sleep: Option<String>) -> Result<()> {
    let sleep_mode = sleep.as_deref().map(parse_sleep_spec).transpose()?;
    // Multi-thread runtime: the worker runs on a blocking thread and the player
    // engine thread `block_on`s worker writes through this handle, so it must
    // outlive the engine. Tear down in order: player -> worker -> runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .context("building runtime")?;

    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).context("spawning worker")?
    };
    let pool = ReadPool::new(db, 3).context("opening read pool")?;

    // An explicit track id replaces the queue ("play this now").
    if let Some(id) = track_id {
        runtime
            .block_on(worker.replace_queue_with_tracks(vec![id]))
            .context("setting the queue")?;
    }

    // Resolve the queue and decide where to start (resume the saved cursor only
    // when no explicit track was given).
    let items = resolve_queue_items(&pool, &root, &PlaybackConfig::default())?;
    let saved = {
        let conn = pool.open().context("opening pool connection")?;
        read_playback_state(&conn).context("reading playback state")?
    };

    if items.is_empty() {
        println!("Queue is empty. Add tracks with `queue add <db> <id>...` or `play <db> <id>`.");
        let _ = runtime.block_on(worker.shutdown_ack());
        return Ok(());
    }

    let (start, start_pos) = match track_id {
        Some(_) => (0, 0.0),
        // Resume at the saved cursor, matched by kind + id: the cursor's id is
        // its track_id (track) or episode_id (episode), and a queue item's
        // `track_id` field carries whichever id its kind implies (6b-ii-c-2).
        None => saved
            .and_then(|s| {
                let id = match s.kind {
                    MediaKind::Track => s.track_id,
                    MediaKind::Episode => s.episode_id,
                    MediaKind::Audiobook => None,
                };
                id.map(|id| (s.kind, id, s.position))
            })
            .and_then(|(kind, id, pos)| {
                items
                    .iter()
                    .position(|i| i.kind == kind && i.track_id == id)
                    .map(|i| (i, pos))
            })
            .unwrap_or((0, 0.0)),
    };

    let player = conservatory_core::player::spawn(worker.clone(), runtime.handle().clone())
        .context("starting the player engine")?;
    // Apply the persisted equalizer (Phase 5.5b) and DSP modules (Phase 5.5c)
    // before playing, so headless playback honours the user's audio settings.
    if let Ok(conn) = pool.open() {
        if let Ok(eq) = conservatory_core::db::get_eq_state(&conn) {
            player.set_eq(eq);
        }
        if let Ok(audio) = conservatory_core::db::get_audio_state(&conn) {
            player.set_dsp(audio.dsp);
        }
    }
    println!("Playing {} item(s), starting at #{start}.", items.len());
    player.play_queue(items, start);
    if start_pos > 0.0 {
        player.seek(start_pos);
        println!("Resuming at {start_pos:.1}s.");
    }
    if let Some(mode) = sleep_mode {
        player.set_sleep_timer(Some(mode));
        println!("Sleep timer armed ({mode:?}).");
    }

    // Drive the engine by polling its snapshot; print each advance until the
    // queue ends. The engine itself persists position + play counts.
    let mut last: Option<usize> = None;
    loop {
        let snap = player.snapshot();
        if snap.current_index != last {
            if let Some(idx) = snap.current_index {
                println!(
                    "  > #{idx}  track {}  ({:.0}s)",
                    snap.track_id.unwrap_or(0),
                    snap.duration.unwrap_or(0.0),
                );
            }
            last = snap.current_index;
        }
        if snap.ended {
            break;
        }
        // A fired duration sleep timer paused playback: end the headless run (the
        // GUI would keep the tap-to-extend window open, Phase 6c-iii-d).
        if snap.sleep.is_some_and(|s| s.fired) {
            println!("Sleep timer elapsed; playback paused.");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }

    player.shutdown();
    let _ = runtime.block_on(worker.shutdown_ack());
    drop(worker);
    drop(runtime);
    println!("Done.");
    Ok(())
}

fn queue(action: QueueAction) -> Result<()> {
    match action {
        QueueAction::Add { db, track_ids } => block_on(run_queue_add(db, track_ids)),
        QueueAction::List { db } => queue_list(db),
        QueueAction::Remove { db, position } => block_on(run_queue_remove(db, position)),
        QueueAction::Clear { db } => block_on(run_queue_clear(db)),
    }
}

async fn run_queue_add(db: PathBuf, track_ids: Vec<i64>) -> Result<()> {
    let worker = spawn_worker(db).context("spawning worker")?;
    let n = track_ids.len();
    worker
        .enqueue_tracks(track_ids)
        .await
        .context("enqueuing tracks")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("enqueued {n} track(s)");
    Ok(())
}

async fn run_queue_remove(db: PathBuf, position: i64) -> Result<()> {
    let worker = spawn_worker(db).context("spawning worker")?;
    worker
        .remove_queue_item(position)
        .await
        .context("removing queue item")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("removed position {position}");
    Ok(())
}

async fn run_queue_clear(db: PathBuf) -> Result<()> {
    let worker = spawn_worker(db).context("spawning worker")?;
    worker.clear_queue().await.context("clearing queue")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("queue cleared");
    Ok(())
}

fn queue_list(db: PathBuf) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let rows = load_queue(&conn).context("loading queue")?;
    if rows.is_empty() {
        println!("(queue empty)");
        return Ok(());
    }
    for row in &rows {
        let title = row
            .track_id
            .and_then(|id| get_track(&conn, id).ok().flatten())
            .map(|t| t.title)
            .unwrap_or_else(|| "-".to_string());
        println!("{}\t{}\t{}", row.position, row.kind, title);
    }
    Ok(())
}

fn debug_facets(db: PathBuf) -> Result<()> {
    use conservatory_core::db::{FacetField, facet_rows, facet_tracks};
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;

    for (label, field) in [
        ("Genre", FacetField::Genre),
        ("Album Artist", FacetField::AlbumArtist),
        ("Album", FacetField::Album),
    ] {
        let rows = facet_rows(&conn, field, &[]).context("facet rows")?;
        let total: i64 = rows.iter().map(|r| r.count).sum();
        println!(
            "=== {label} [All ({} {})] ===",
            rows.len(),
            label.to_lowercase()
        );
        for row in &rows {
            println!("  {:>5}  {}", row.count, row.value);
        }
        let _ = total;
    }

    let leaf = facet_tracks(&conn, &[]).context("facet tracks")?;
    println!("\nleaf: {} track(s)", leaf.len());
    Ok(())
}

fn search(db: PathBuf, query: String, format: Format) -> Result<()> {
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let today = Utc::now().date_naive();

    let parsed = parse(&query);
    for warning in &parsed.warnings {
        eprintln!("warning: {warning}");
    }

    // SQL fast path when the whole expression translates; else in-memory eval.
    let rows = search_rows(&conn).context("loading search rows")?;
    let mut matched: Vec<SearchRow> = match try_translate(&parsed.expr, today) {
        Some(clause) => {
            let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
            let ids: std::collections::HashSet<i64> = search_track_ids(&conn, &clause.sql, &params)
                .context("running search SQL")?
                .into_iter()
                .collect();
            rows.into_iter()
                .filter(|r| ids.contains(&r.track_id))
                .collect()
        }
        None => rows
            .into_iter()
            .filter(|r| conservatory_search::evaluate(&parsed.expr, &to_item(r), today))
            .collect(),
    };

    // Precompute FTS bm25 for bare-text terms (ranking input).
    let terms = collect_text_terms(&parsed.expr);
    let bm = if terms.is_empty() {
        Default::default()
    } else {
        let match_query = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        fts_rank(&conn, &match_query).unwrap_or_default()
    };
    order_results(&mut matched, &parsed, &bm);

    match format {
        Format::Json => println!("{{\"matches\":{}}}", matched.len()),
        Format::Tsv => {
            println!("id\ttitle\tartist\talbum");
            for r in &matched {
                println!(
                    "{}\t{}\t{}\t{}",
                    r.track_id,
                    r.title,
                    r.artist.as_deref().unwrap_or(""),
                    r.album.as_deref().unwrap_or("")
                );
            }
        }
        Format::Human => {
            for r in &matched {
                println!(
                    "{}  —  {} · {}",
                    r.title,
                    r.artist.as_deref().unwrap_or("?"),
                    r.album.as_deref().unwrap_or("?")
                );
            }
            println!("\n{} match(es)", matched.len());
        }
    }
    Ok(())
}

/// Order results: explicit `sort:` specs win; else bare-text hits rank by FTS
/// bm25 (in `bm`) blended with recency; else by title.
fn order_results(
    rows: &mut [SearchRow],
    parsed: &conservatory_search::ParseResult,
    bm: &std::collections::HashMap<i64, f64>,
) {
    use conservatory_search::SortKey;
    if let Some(spec) = parsed.sorts.first() {
        rows.sort_by(|a, b| {
            let ord = match spec.key {
                SortKey::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                SortKey::Artist => artist_key(a).cmp(&artist_key(b)),
                SortKey::Album => album_key(a).cmp(&album_key(b)),
                SortKey::Year => a.year.cmp(&b.year),
                SortKey::Added => a.added.cmp(&b.added),
                SortKey::Rating => a.rating.cmp(&b.rating),
                SortKey::Duration => a
                    .duration
                    .partial_cmp(&b.duration)
                    .unwrap_or(std::cmp::Ordering::Equal),
            };
            if spec.descending { ord.reverse() } else { ord }
        });
        return;
    }

    if !bm.is_empty() {
        let now = Utc::now().timestamp();
        let score = |r: &SearchRow| {
            let bm25 = bm.get(&r.track_id).copied().unwrap_or(0.0);
            let days = r.added.map(|a| (now - a).max(0) / 86_400).unwrap_or(3650);
            blend_relevance(bm25, days, 30.0)
        };
        rows.sort_by(|a, b| {
            score(b)
                .partial_cmp(&score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return;
    }

    rows.sort_by_key(|r| r.title.to_lowercase());
}

fn artist_key(r: &SearchRow) -> String {
    r.artist.clone().unwrap_or_default().to_lowercase()
}

fn album_key(r: &SearchRow) -> String {
    r.album.clone().unwrap_or_default().to_lowercase()
}

fn to_param(value: &SqlValue) -> SqlParam {
    match value {
        SqlValue::Text(s) => SqlParam::Text(s.clone()),
        SqlValue::Int(n) => SqlParam::Int(*n),
        SqlValue::Real(x) => SqlParam::Real(*x),
    }
}

fn to_item(r: &SearchRow) -> SearchItem {
    SearchItem {
        title: r.title.clone(),
        artist: r.artist.clone(),
        album_artist: r.album_artist.clone(),
        album: r.album.clone(),
        shelf_genre: r.shelf_genre.clone(),
        genres: r.genres.clone(),
        year: r.year,
        added: r.added,
        rating: r.rating,
        bitrate: r.bitrate,
        duration: r.duration,
        format: r.format.clone(),
        played: r.played,
        starred: r.starred,
        queued: r.queued,
        // Music rows carry no audiobook projection (the shelf is matched in memory).
        ..SearchItem::default()
    }
}

/// Run a future on a fresh current-thread runtime (the CLI's worker pattern).
#[cfg(feature = "podcasts")]
fn podcast(action: PodcastAction) -> Result<()> {
    match action {
        PodcastAction::Add { db, url, format } => block_on(run_podcast_add(db, url, format)),
        PodcastAction::Remove { db, show_id } => block_on(run_podcast_remove(db, show_id)),
        PodcastAction::Refresh {
            db,
            show_id,
            format,
        } => block_on(run_podcast_refresh(db, show_id, format)),
        PodcastAction::Download {
            db,
            episode_id,
            root,
        } => block_on(run_podcast_download(db, episode_id, root)),
        PodcastAction::Prune {
            db,
            show_id,
            root,
            apply,
        } => block_on(run_podcast_prune(db, show_id, root, apply)),
        PodcastAction::Episodes {
            db,
            show,
            bucket,
            format,
        } => run_podcast_episodes(db, show, bucket, format),
        PodcastAction::Mark {
            db,
            episode_id,
            state,
        } => block_on(run_podcast_mark(db, episode_id, state)),
        PodcastAction::Star {
            db,
            episode_id,
            off,
        } => block_on(run_podcast_star(db, episode_id, off)),
        PodcastAction::Settings { db, show_id, speed } => {
            block_on(run_podcast_settings(db, show_id, speed))
        }
        PodcastAction::DebugChain { db, episode_id } => podcast_debug_chain(db, episode_id),
        PodcastAction::Stats { db } => podcast_stats(db),
        PodcastAction::Chapters { db, episode_id } => podcast_chapters(db, episode_id),
    }
}

/// List an episode's stored chapters (Phase 6c-iii): index, start (mm:ss), and
/// title, read-only via the read pool. An episode with no chapters prints a note.
#[cfg(feature = "podcasts")]
fn podcast_chapters(db: PathBuf, episode_id: i64) -> Result<()> {
    use conservatory_core::db::list_chapters;

    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let chapters = list_chapters(&conn, episode_id).context("reading chapters")?;

    if chapters.is_empty() {
        println!("episode {episode_id}: no chapters");
        return Ok(());
    }
    println!("episode {episode_id}: {} chapters", chapters.len());
    for (i, ch) in chapters.iter().enumerate() {
        let title = ch.title.as_deref().unwrap_or("(untitled)");
        // `fmt_duration` (6c-ii) is the same M:SS / H:MM:SS clock format.
        println!(
            "  {:>3}  {:>8}  {title}",
            i + 1,
            fmt_duration(ch.start_time)
        );
    }
    Ok(())
}

/// Print the listening totals (Phase 6c-ii): the append-only `listening_sessions`
/// ledger summed into session count, wall-clock listen time, audio covered, and
/// the time Smart Speed saved. Read-only.
#[cfg(feature = "podcasts")]
fn podcast_stats(db: PathBuf) -> Result<()> {
    use conservatory_core::db::listening_totals;

    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let totals = listening_totals(&conn).context("reading listening totals")?;

    println!("sessions:         {}", totals.sessions);
    println!("listened:         {}", fmt_duration(totals.real_seconds));
    println!("audio covered:    {}", fmt_duration(totals.audio_seconds));
    println!(
        "smart speed saved: {}",
        fmt_duration(totals.smart_speed_saved)
    );
    Ok(())
}

/// Format a duration in seconds as `H:MM:SS` (or `M:SS` under an hour), for the
/// stats readout.
#[cfg(feature = "podcasts")]
fn fmt_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Print an episode's resolved `af` chain (Phase 6c): the spoken-word profile
/// (Smart Speed / Voice Boost from the show settings) composed with the persisted
/// EQ + DSP, exactly as `MpvHost::load` would build it. Read-only.
#[cfg(feature = "podcasts")]
fn podcast_debug_chain(db: PathBuf, episode_id: i64) -> Result<()> {
    use conservatory_core::db::{get_audio_state, get_eq_state, get_show_settings};
    use conservatory_core::resolve_episode_profile;

    let pool = ReadPool::new(db, 1).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let episode = get_episode(&conn, episode_id)
        .context("looking up episode")?
        .ok_or_else(|| anyhow::anyhow!("no episode with id {episode_id}"))?;
    let settings = get_show_settings(&conn, episode.show_id).context("reading show settings")?;
    let profile = resolve_episode_profile(settings.as_ref());
    let eq = get_eq_state(&conn).context("reading EQ state")?;
    let dsp = get_audio_state(&conn).context("reading audio state")?.dsp;
    let chain = build_af_chain(&profile, &eq, &dsp);

    println!("episode:     {} {}", episode.id, episode.title);
    println!(
        "speed:       {}  pitch_correction={}",
        profile.speed, profile.pitch_correction
    );
    println!("smart_speed: {}", profile.smart_speed);
    println!("voice_boost: {}", profile.voice_boost);
    println!(
        "af chain:    {}",
        if chain.is_empty() { "(empty)" } else { &chain }
    );
    Ok(())
}

/// A minimal JSON string literal (quote + escape) for the hand-rolled `--json`
/// output (serde is not a CLI dependency).
#[cfg(feature = "podcasts")]
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(feature = "podcasts")]
async fn run_podcast_mark(db: PathBuf, episode_id: i64, state: String) -> Result<()> {
    use conservatory_core::db::PlayedState;

    let played = match state.to_ascii_lowercase().as_str() {
        "played" => PlayedState::PlayedFully,
        "unplayed" => PlayedState::Unplayed,
        "archived" => PlayedState::ArchivedUnlistened,
        other => anyhow::bail!("unknown state '{other}' (played | unplayed | archived)"),
    };
    // Stamp last_played only when actually played.
    let when = (played == PlayedState::PlayedFully).then(now_secs);

    let worker = spawn_worker(db).context("spawning worker")?;
    worker
        .set_episode_played(episode_id, played, when)
        .await
        .context("setting played state")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Episode {episode_id} marked {state}.");
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_star(db: PathBuf, episode_id: i64, off: bool) -> Result<()> {
    let worker = spawn_worker(db).context("spawning worker")?;
    worker
        .set_episode_starred(episode_id, !off)
        .await
        .context("setting starred")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!(
        "Episode {episode_id} {}.",
        if off { "unstarred" } else { "starred" }
    );
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_settings(db: PathBuf, show_id: i64, speed: Option<f64>) -> Result<()> {
    use conservatory_core::db::{InboxPolicy, ShowSettings};

    // Read current settings, or the schema defaults if the show has none, so a
    // `--speed` set preserves the other fields (the partial-edit discipline).
    let pool = ReadPool::new(db.clone(), 1).context("opening read pool")?;
    let current = {
        let conn = pool.open().context("opening pool connection")?;
        get_show_settings(&conn, show_id).context("reading show settings")?
    };
    let mut settings = current.unwrap_or(ShowSettings {
        show_id,
        playback_speed: 1.0,
        smart_speed: true,
        voice_boost: false,
        skip_intro: 0,
        skip_outro: 0,
        skip_forward: None,
        skip_back: None,
        inbox_policy: InboxPolicy::Inbox,
    });

    match speed {
        Some(s) => {
            anyhow::ensure!(s > 0.0, "speed must be positive (e.g. 1.5)");
            settings.playback_speed = s;
            let worker = spawn_worker(db).context("spawning worker")?;
            worker
                .upsert_show_settings(settings)
                .await
                .context("saving show settings")?;
            worker.shutdown_ack().await.context("shutdown ack")?;
            println!("Show {show_id} playback speed set to {s}x.");
        }
        None => {
            println!(
                "Show {show_id}: speed {}x, smart_speed {}, voice_boost {}, \
                 skip_intro {}s, skip_outro {}s, inbox_policy {}",
                settings.playback_speed,
                settings.smart_speed,
                settings.voice_boost,
                settings.skip_intro,
                settings.skip_outro,
                settings.inbox_policy.as_str(),
            );
        }
    }
    Ok(())
}

/// List episodes with triage state. Read-only: no worker, just the pool.
#[cfg(feature = "podcasts")]
fn run_podcast_episodes(
    db: PathBuf,
    show: Option<i64>,
    bucket: Option<String>,
    format: Format,
) -> Result<()> {
    use conservatory_core::db::{TriageBucket, episodes_for_show, episodes_in_bucket};

    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let conn = pool.open().context("opening pool connection")?;
    let rows = if let Some(show_id) = show {
        episodes_for_show(&conn, show_id).context("reading show episodes")?
    } else {
        let bucket = match bucket.as_deref() {
            Some(s) => TriageBucket::parse(s)
                .ok_or_else(|| anyhow::anyhow!("unknown bucket '{s}' (inbox | queue | played)"))?,
            None => TriageBucket::Inbox,
        };
        episodes_in_bucket(&conn, bucket).context("reading triage bucket")?
    };
    print_episode_rows(&rows, format);
    Ok(())
}

#[cfg(feature = "podcasts")]
fn print_episode_rows(rows: &[conservatory_core::db::EpisodeListRow], format: Format) {
    use conservatory_core::db::PlayedState;

    let state = |p: PlayedState| match p {
        PlayedState::Unplayed => "unplayed",
        PlayedState::InProgress => "in-progress",
        PlayedState::PlayedFully => "played",
        PlayedState::ArchivedUnlistened => "archived",
    };
    let date = |r: &conservatory_core::db::EpisodeListRow| {
        r.pub_date
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "-".to_string())
    };
    let dur = |r: &conservatory_core::db::EpisodeListRow| {
        r.duration
            .map(|s| format!("{}:{:02}", s / 60, s % 60))
            .unwrap_or_else(|| "-".to_string())
    };

    match format {
        Format::Tsv => {
            println!("id\tshow\ttitle\tdate\tduration\tstate\tstarred\tqueued");
            for r in rows {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    r.id,
                    r.show_title,
                    r.title,
                    date(r),
                    dur(r),
                    state(r.played),
                    r.starred,
                    r.in_queue,
                );
            }
        }
        Format::Json => {
            print!("[");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    print!(",");
                }
                print!(
                    "{{\"id\":{},\"show\":{},\"title\":{},\"date\":\"{}\",\"state\":\"{}\",\"starred\":{},\"queued\":{}}}",
                    r.id,
                    json_str(&r.show_title),
                    json_str(&r.title),
                    date(r),
                    state(r.played),
                    r.starred,
                    r.in_queue,
                );
            }
            println!("]");
        }
        Format::Human => {
            if rows.is_empty() {
                println!("(no episodes)");
            }
            for r in rows {
                let flags = match (r.starred, r.in_queue) {
                    (true, true) => " ★ queued",
                    (true, false) => " ★",
                    (false, true) => " queued",
                    (false, false) => "",
                };
                println!(
                    "[{}] {} — {} ({}, {}){}",
                    state(r.played),
                    r.show_title,
                    r.title,
                    date(r),
                    dur(r),
                    flags,
                );
            }
        }
    }
}

#[cfg(feature = "podcasts")]
async fn run_podcast_add(db: PathBuf, url: String, format: Format) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let fetcher = conservatory_podcasts::Fetcher::new().context("building feed fetcher")?;
    let (show_id, new, total) = conservatory_podcasts::add_show(&worker, &pool, &fetcher, &url)
        .await
        .context("subscribing to feed")?;
    worker.shutdown_ack().await.context("shutdown ack")?;

    match format {
        Format::Json => println!("{{\"show_id\":{show_id},\"new\":{new},\"total\":{total}}}"),
        Format::Tsv => {
            println!("show_id\tnew\ttotal");
            println!("{show_id}\t{new}\t{total}");
        }
        Format::Human => {
            println!("Subscribed (show {show_id}): {new} new of {total} episode(s).")
        }
    }
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_remove(db: PathBuf, show_id: i64) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    {
        let pool = ReadPool::new(db, 3).context("opening read pool")?;
        let conn = pool.open().context("opening pool connection")?;
        if conservatory_core::db::get_show(&conn, show_id)
            .context("looking up show")?
            .is_none()
        {
            worker.shutdown_ack().await.ok();
            anyhow::bail!("no show with id {show_id}");
        }
    }
    worker.delete_show(show_id).await.context("deleting show")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!("Removed show {show_id} (its episodes and state cascade).");
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_refresh(db: PathBuf, show_id: Option<i64>, format: Format) -> Result<()> {
    use conservatory_podcasts::RefreshStatus;

    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let fetcher = conservatory_podcasts::Fetcher::new().context("building feed fetcher")?;
    // Best-effort: a missing secret service just means private feeds stay
    // anonymous (and a 401 surfaces as a per-show Failed outcome).
    let creds = conservatory_podcasts::CredentialStore::secret_service()
        .await
        .ok();

    let outcomes = if let Some(id) = show_id {
        let show = {
            let conn = pool.open().context("opening pool connection")?;
            conservatory_core::db::get_show(&conn, id)
                .context("looking up show")?
                .ok_or_else(|| anyhow::anyhow!("no show with id {id}"))?
        };
        vec![
            conservatory_podcasts::refresh_show(&worker, &pool, &fetcher, show, creds.as_ref())
                .await
                .context("refreshing show")?,
        ]
    } else {
        conservatory_podcasts::refresh_all(&worker, &pool, &fetcher, creds)
            .await
            .context("refreshing subscriptions")?
    };
    worker.shutdown_ack().await.context("shutdown ack")?;

    let status_str = |s: &RefreshStatus| match s {
        RefreshStatus::Updated { new, total } => format!("updated\t{new}\t{total}"),
        RefreshStatus::NotModified => "not-modified\t0\t0".to_string(),
        RefreshStatus::Failed(_) => "failed\t0\t0".to_string(),
    };

    match format {
        Format::Tsv => {
            println!("show_id\ttitle\tstatus\tnew\ttotal");
            for o in &outcomes {
                println!("{}\t{}\t{}", o.show_id, o.show_title, status_str(&o.status));
            }
        }
        Format::Json => {
            print!("[");
            for (i, o) in outcomes.iter().enumerate() {
                if i > 0 {
                    print!(",");
                }
                let (status, new, total) = match &o.status {
                    RefreshStatus::Updated { new, total } => ("updated", *new, *total),
                    RefreshStatus::NotModified => ("not_modified", 0, 0),
                    RefreshStatus::Failed(_) => ("failed", 0, 0),
                };
                print!(
                    "{{\"show_id\":{},\"status\":\"{status}\",\"new\":{new},\"total\":{total}}}",
                    o.show_id
                );
            }
            println!("]");
        }
        Format::Human => {
            for o in &outcomes {
                let line = match &o.status {
                    RefreshStatus::Updated { new, total } => {
                        format!("{new} new of {total} episode(s)")
                    }
                    RefreshStatus::NotModified => "not modified".to_string(),
                    RefreshStatus::Failed(e) => format!("FAILED: {e}"),
                };
                println!("{} — {}", o.show_title, line);
            }
            if outcomes.is_empty() {
                println!("No subscriptions.");
            }
        }
    }
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_download(db: PathBuf, episode_id: i64, root: PathBuf) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let fetcher = conservatory_podcasts::Fetcher::new().context("building fetcher")?;

    let (episode, show) = {
        let conn = pool.open().context("opening pool connection")?;
        let episode = conservatory_core::db::get_episode(&conn, episode_id)
            .context("looking up episode")?
            .ok_or_else(|| anyhow::anyhow!("no episode with id {episode_id}"))?;
        let show =
            conservatory_core::db::get_show(&conn, episode.show_id).context("looking up show")?;
        (episode, show)
    };

    // Resolve the show's Basic-auth credentials, if any (best-effort).
    let creds = conservatory_podcasts::CredentialStore::secret_service()
        .await
        .ok();
    let auth = match (&creds, &show) {
        (Some(store), Some(s)) => store
            .resolve(s.auth_user.as_deref(), s.auth_pass_ref.as_deref())
            .await
            .ok()
            .flatten(),
        _ => None,
    };

    let dst = conservatory_podcasts::download_episode(
        &fetcher.client(),
        &worker,
        &root,
        &episode,
        auth.as_ref(),
    )
    .await
    .context("downloading episode")?;
    worker.shutdown_ack().await.context("shutdown ack")?;

    println!("Downloaded episode {episode_id} to {}.", dst.display());
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_podcast_prune(
    db: PathBuf,
    show_id: Option<i64>,
    root: PathBuf,
    apply: bool,
) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;

    let plan = conservatory_podcasts::retention::plan(&pool, show_id).context("planning prune")?;
    if plan.is_empty() {
        println!("Nothing to prune (no downloads beyond keep_count).");
        worker.shutdown_ack().await.context("shutdown ack")?;
        return Ok(());
    }

    if apply {
        let pruned = conservatory_podcasts::retention::apply(&worker, &root, &plan)
            .await
            .context("applying prune")?;
        println!("Pruned {pruned} of {} downloaded episode(s):", plan.len());
    } else {
        println!(
            "Would prune {} downloaded episode(s) (dry-run; pass --apply to delete):",
            plan.len()
        );
    }
    for p in &plan {
        println!("  {}\t{}\t{}", p.show_title, p.episode_title, p.audio_path);
    }
    worker.shutdown_ack().await.context("shutdown ack")?;
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_import_opml(db: PathBuf, file: PathBuf) -> Result<()> {
    let body = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let summary = conservatory_podcasts::import_opml(&worker, &pool, &body)
        .await
        .context("importing OPML")?;
    worker.shutdown_ack().await.context("shutdown ack")?;
    println!(
        "Imported {} subscription(s) ({} new). Run `podcast refresh` to pull episodes.",
        summary.total, summary.created
    );
    Ok(())
}

#[cfg(feature = "podcasts")]
async fn run_export_opml(db: PathBuf, out: Option<PathBuf>) -> Result<()> {
    // Export is read-only: no worker, just the pool.
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let xml = conservatory_podcasts::export_opml(&pool)
        .await
        .context("exporting OPML")?;
    match out {
        Some(path) => {
            std::fs::write(&path, &xml).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("Wrote OPML to {}.", path.display());
        }
        None => print!("{xml}"),
    }
    Ok(())
}

fn block_on<F: std::future::Future<Output = Result<()>>>(fut: F) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building current-thread runtime")?
        .block_on(fut)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Install the tracing subscriber (v0.0.38). The worker / fetch / refresh code
/// emits tracing events; without a subscriber they are silent. Headless control
/// is `RUST_LOG` (e.g. `RUST_LOG=conservatory_podcasts=debug`); the default
/// `warn` keeps a scriptable run quiet (only warnings/errors), so routine info
/// never clutters stderr or interferes with the `--tsv` / `--json` stdout. Logs
/// go to stderr.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .compact()
        .init();
}

fn opt<T: std::fmt::Display>(value: &Option<T>) -> String {
    value.as_ref().map_or_else(|| "-".to_string(), T::to_string)
}

fn join(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(", ")
    }
}

fn num_of(n: Option<u32>, total: Option<u32>) -> String {
    match (n, total) {
        (Some(n), Some(t)) => format!("{n}/{t}"),
        (Some(n), None) => n.to_string(),
        _ => "-".to_string(),
    }
}

fn num_of_f(track: Option<f64>, album: Option<f64>) -> String {
    match (track, album) {
        (None, None) => "-".to_string(),
        _ => format!("track {} / album {}", opt(&track), opt(&album)),
    }
}

async fn run_fixture(db: PathBuf, scale: FixtureScale) -> Result<()> {
    let worker = spawn_worker(db.clone()).context("spawning worker")?;
    fixtures::generate(&worker, scale)
        .await
        .context("generating fixture")?;

    // Counts come back through the read pool, proving the write -> read split.
    let pool = ReadPool::new(db, 3).context("opening read pool")?;
    let counts = library_counts(&pool.open().context("opening pool connection")?)
        .context("counting library")?;

    worker.shutdown_ack().await.context("shutdown ack")?;

    println!(
        "OK: artists={} albums={} tracks={}",
        counts.artists, counts.albums, counts.tracks
    );
    Ok(())
}

#[cfg(all(test, feature = "audiobooks"))]
mod audiobook_filter_tests {
    use super::book_search_item;
    use chrono::NaiveDate;
    use conservatory_core::db::BookListRow;
    use conservatory_search::{evaluate, parse};

    fn row() -> BookListRow {
        BookListRow {
            id: 1,
            title: "The Way of Kings".into(),
            subtitle: None,
            author_display: Some("Brandon Sanderson".into()),
            narrator_display: Some("Kate Reading, Michael Kramer".into()),
            series_name: Some("The Stormlight Archive".into()),
            series_sequence: Some(1.0),
            year: Some(2010),
            cover_path: None,
            accent_rgb: None,
            rating: 5,
            starred: true,
            position: 0.0,
            finished: true,
            last_played: None,
            total_duration: 3600.0,
        }
    }

    fn matches(expr: &str) -> bool {
        let today = NaiveDate::from_ymd_opt(2026, 6, 28).unwrap();
        evaluate(&parse(expr).expr, &book_search_item(&row()), today)
    }

    #[test]
    fn cli_mapping_filters_audiobook_fields() {
        assert!(matches("author:sanderson"));
        assert!(matches("narrator:kramer")); // second narrator, split from the join
        assert!(matches("series:stormlight"));
        assert!(matches("is:finished AND is:starred"));
        assert!(matches("rating:>=4 AND year:2010"));
        assert!(!matches("author:tolkien"));
        assert!(!matches("NOT is:finished"));
    }
}
