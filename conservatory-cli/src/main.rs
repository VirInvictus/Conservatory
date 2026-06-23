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
    MediaKind, ReadPool, SearchRow, SqlParam, fts_rank, get_album, get_episode, get_show_settings,
    get_track, library_counts, load_queue, probe_read, read_playback_state, search_rows,
    search_track_ids, spawn_worker, track_render_rows, writeback_rows,
};
use conservatory_core::mover::{self, MoveKind, MoveMode, organize_ops};
use conservatory_core::{
    AlbumEdit, Assignment, DEFAULT_TARGET_LUFS, Field, GenreVocab, ImportOptions, ImportReport,
    PathTemplate, PlayableItem, PlaybackConfig, TagWrite, TrackDraft, TrackEdit, TrackFields,
    any_path_affecting, build_album_edit, build_track_edit, compute_accent, find_collisions,
    find_cover_bytes, genres_assignment, import_folder, parse_assignment, read_track, replace_in,
    replaygain_from_file, resolve_album, resolve_episode_profile, resolve_music_profile,
    resync_album_covers, rsgain_available, scan_album_files, sync_album_cover, write_track_tags,
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

    /// Phase 3b smoke test: dump the faceted-browse panes (Genre → Album Artist
    /// → Album) with counts and the leaf track total. Read-only.
    DebugFacets {
        /// Path to the SQLite database.
        db: PathBuf,
    },
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
    match Cli::parse().command {
        Some(Command::DebugRoundtrip { db }) => debug_roundtrip(db),
        Some(Command::DebugFixture { db, scale }) => debug_fixture(db, scale),
        Some(Command::DebugTags { file }) => debug_tags(file),
        Some(Command::DebugPaths { db }) => debug_paths(db),
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
        Some(Command::Play { db, root, track_id }) => play(db, root, track_id),
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
        Some(Command::DebugFacets { db }) => debug_facets(db),
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
                let source = match (ep.audio_path.as_deref(), ep.audio_url.as_deref()) {
                    (Some(p), _) => root.join(p),
                    (None, Some(url)) => PathBuf::from(url),
                    (None, None) => continue,
                };
                // Resolve the show's per-show overrides (speed) for the profile.
                let settings = get_show_settings(&conn, ep.show_id).context("show settings")?;
                items.push(PlayableItem {
                    track_id: episode_id, // the queue item's id field carries the episode id
                    source,
                    profile: resolve_episode_profile(settings.as_ref()),
                    album_id: None,
                    kind: MediaKind::Episode,
                });
            }
            MediaKind::Audiobook => continue, // Phase 7
        }
    }
    Ok(items)
}

fn play(db: PathBuf, root: PathBuf, track_id: Option<i64>) -> Result<()> {
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
    println!("Playing {} item(s), starting at #{start}.", items.len());
    player.play_queue(items, start);
    if start_pos > 0.0 {
        player.seek(start_pos);
        println!("Resuming at {start_pos:.1}s.");
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
    }
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
