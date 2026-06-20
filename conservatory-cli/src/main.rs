//! Conservatory headless CLI. The batch surface that pairs with the GUI (the
//! Hermitage / CalibreQuarry / Belfry pattern). Phase 1a ships a single debug
//! verb that exercises the worker + read-pool round-trip; the real verbs
//! (import, organize, search, tag, queue, podcast, stats) land at Phase 2+
//! (spec §9).

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    ReadPool, SearchRow, SqlParam, fts_rank, library_counts, probe_read, search_rows,
    search_track_ids, spawn_worker, track_render_rows,
};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp};
use conservatory_core::{
    GenreVocab, ImportOptions, ImportReport, PathTemplate, TrackFields, compute_accent,
    find_collisions, find_cover_bytes, import_folder, read_track, resolve_album,
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
