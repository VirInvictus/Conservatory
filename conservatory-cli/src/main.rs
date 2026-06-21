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
    MediaKind, ReadPool, SearchRow, SqlParam, fts_rank, get_track, library_counts, load_queue,
    probe_read, read_playback_state, search_rows, search_track_ids, spawn_worker,
    track_render_rows,
};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp};
use conservatory_core::{
    AlbumEdit, Assignment, Field, GenreVocab, ImportOptions, ImportReport, PathTemplate,
    PlayableItem, PlaybackConfig, TrackEdit, TrackFields, any_path_affecting, build_album_edit,
    build_track_edit, compute_accent, find_collisions, find_cover_bytes, genres_assignment,
    import_folder, parse_assignment, read_track, replace_in, resolve_album, resolve_music_profile,
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

    /// Phase 3b smoke test: dump the faceted-browse panes (Genre → Album Artist
    /// → Album) with counts and the leaf track total. Read-only.
    DebugFacets {
        /// Path to the SQLite database.
        db: PathBuf,
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
        Some(Command::Search { db, query, format }) => search(db, query, format),
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

    // Build the operations: src = current managed path, dst = re-rendered target.
    let rows = {
        let conn = pool.open().context("opening pool connection")?;
        track_render_rows(&conn).context("reading track render rows")?
    };
    let template = PathTemplate::default_music();
    let mut ops = Vec::with_capacity(rows.len());
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
        let rel = template.render(&fields);
        ops.push(MoveOp {
            track_id: Some(row.track_id),
            album_id: row.album_id,
            src: root.join(&row.file_path),
            dst: root.join(&rel),
            db_old: Some(row.file_path.clone()),
            db_new: Some(rel.to_string_lossy().into_owned()),
        });
    }

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
        match format {
            Format::Json => println!("{{\"job_id\":{job_id},\"tracks\":{count}}}"),
            _ => println!(
                "applied job {job_id}: {count} track(s) organized under {}",
                root.display()
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
    let rows = {
        let conn = pool.open().context("opening pool connection")?;
        track_render_rows(&conn).context("reading render rows")?
    };
    let template = PathTemplate::default_music();
    let mut ops = Vec::new();
    for row in &rows {
        if !row.album_id.map(|a| albums.contains(&a)).unwrap_or(false) {
            continue;
        }
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
        let rel = template.render(&fields);
        ops.push(MoveOp {
            track_id: Some(row.track_id),
            album_id: row.album_id,
            src: root.join(&row.file_path),
            dst: root.join(&rel),
            db_old: Some(row.file_path.clone()),
            db_new: Some(rel.to_string_lossy().into_owned()),
        });
    }

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
        println!("applied move job {job_id}: {count} file(s) re-shelved");
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

/// Resolve the queue rows into `PlayableItem`s the engine can play. Phase 4b-i
/// is music-only, so non-track rows (none yet) are skipped. `tracks.file_path`
/// is stored relative to the library root, so it is joined with `root` to get
/// the absolute path libmpv loads.
fn resolve_queue_items(
    pool: &ReadPool,
    root: &Path,
    cfg: &PlaybackConfig,
) -> Result<Vec<PlayableItem>> {
    let conn = pool.open().context("opening pool connection")?;
    let mut items = Vec::new();
    for row in load_queue(&conn).context("loading queue")? {
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
        None => saved
            .and_then(|s| s.track_id.map(|t| (t, s.position)))
            .and_then(|(tid, pos)| {
                items
                    .iter()
                    .position(|i| i.track_id == tid)
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
