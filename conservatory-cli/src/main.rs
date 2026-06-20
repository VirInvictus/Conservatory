//! Conservatory headless CLI. The batch surface that pairs with the GUI (the
//! Hermitage / CalibreQuarry / Belfry pattern). Phase 1a ships a single debug
//! verb that exercises the worker + read-pool round-trip; the real verbs
//! (import, organize, search, tag, queue, podcast, stats) land at Phase 2+
//! (spec §9).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    ReadPool, library_counts, probe_read, spawn_worker, track_render_rows,
};
use conservatory_core::{
    PathTemplate, TrackFields, compute_accent, find_collisions, find_cover_bytes, read_track,
};

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
// All verbs are `Debug*` smoke tests for now; the shared prefix is intentional
// and goes away when the real verbs (import, search, ...) land at Phase 2 (§9).
#[allow(clippy::enum_variant_names)]
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
        None => {
            println!("conservatory-cli {}", conservatory_core::VERSION);
            println!("plugins: {}", plugin_list());
            println!("Phase 1a: try `debug-roundtrip <db>`. Real verbs land at Phase 2 (spec §9).");
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
