//! Phase 2d end-to-end import tests. Uses the committed per-format tag fixtures
//! (copied into a temp source so the originals are never touched), imports them
//! into a temp library, and checks the managed tree + DB, the conflict guarantee,
//! and the shelf-genre-set → organize move (the sub-phase's usable artifact).

use std::fs;
use std::path::{Path, PathBuf};

use conservatory_core::db::{
    ReadPool, library_counts, list_albums, spawn_worker, track_render_rows,
};
use conservatory_core::{ImportOptions, MoveMode, import_folder};
use tempfile::{TempDir, tempdir};

/// Copy the four committed fixture files into a fresh temp dir (the import source).
fn fixture_source() -> TempDir {
    let dir = tempdir().unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    for name in ["sample.flac", "sample.mp3", "sample.opus", "sample.m4a"] {
        fs::copy(src.join(name), dir.path().join(name)).unwrap();
    }
    dir
}

struct Lib {
    _dbdir: TempDir,
    _libdir: TempDir,
    db: PathBuf,
    root: PathBuf,
}

fn lib() -> Lib {
    let dbdir = tempdir().unwrap();
    let libdir = tempdir().unwrap();
    Lib {
        db: dbdir.path().join("library.db"),
        root: libdir.path().to_path_buf(),
        _dbdir: dbdir,
        _libdir: libdir,
    }
}

async fn import_into(lib: &Lib, source: &Path, mode: MoveMode) -> conservatory_core::ImportReport {
    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let opts = ImportOptions {
        library_root: lib.root.clone(),
        mode,
    };
    let report = import_folder(&worker, &pool, source, &opts).await.unwrap();
    worker.shutdown_ack().await.unwrap();
    report
}

#[tokio::test]
async fn imports_an_album_into_the_managed_tree() {
    let src = fixture_source();
    let lib = lib();

    let report = import_into(&lib, src.path(), MoveMode::Copy).await;

    assert_eq!(report.files_scanned, 4);
    assert_eq!(report.tracks, 4);
    assert_eq!(report.albums, 1);
    // Two distinct artists: the album artist and the (different) track artist.
    assert_eq!(report.artists, 2);
    assert!(report.job_id.is_some());
    assert!(report.conflicts.is_empty());

    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let conn = pool.open().unwrap();

    let counts = library_counts(&conn).unwrap();
    assert_eq!(counts.albums, 1);
    assert_eq!(counts.tracks, 4);

    let albums = list_albums(&conn).unwrap();
    // Genres across the four fixtures: Ambient on all four, Electronic on three
    // (ID3 collapsed the mp3 to Ambient), so the most-common shelf genre is Ambient.
    assert_eq!(albums[0].shelf_genre.as_deref(), Some("Ambient"));
    assert_eq!(albums[0].year, Some(2021));

    // Every track's DB path exists on disk under the managed tree, and the
    // originals are untouched (copy mode).
    for row in track_render_rows(&conn).unwrap() {
        assert!(
            lib.root.join(&row.file_path).exists(),
            "missing managed file {}",
            row.file_path
        );
        assert!(
            row.file_path
                .starts_with("Ambient/Test Album Artist/Test Album (2021)/")
        );
    }
    assert_eq!(fs::read_dir(src.path()).unwrap().count(), 4, "sources kept");
}

#[tokio::test]
async fn move_mode_consumes_the_sources() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Move).await;
    assert_eq!(
        fs::read_dir(src.path()).unwrap().count(),
        0,
        "move should consume sources"
    );
}

#[tokio::test]
async fn re_importing_the_same_album_is_refused_and_changes_nothing() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Copy).await;

    // A second import renders the same targets, which now exist: refused.
    let src2 = fixture_source();
    let report = import_into(&lib, src2.path(), MoveMode::Copy).await;
    assert!(!report.conflicts.is_empty());
    assert!(report.job_id.is_none());

    // The library still has exactly one album / four tracks (nothing added).
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();
    let counts = library_counts(&pool.open().unwrap()).unwrap();
    assert_eq!(counts.albums, 1);
    assert_eq!(counts.tracks, 4);
}

#[tokio::test]
async fn shelf_genre_set_then_organize_moves_the_album() {
    let src = fixture_source();
    let lib = lib();
    import_into(&lib, src.path(), MoveMode::Copy).await;

    let worker = spawn_worker(lib.db.clone()).unwrap();
    let pool = ReadPool::new(lib.db.clone(), 3).unwrap();

    let album_id = list_albums(&pool.open().unwrap()).unwrap()[0].id;
    worker
        .set_album_shelf_genre(album_id, "Jazz".to_string())
        .await
        .unwrap();

    // Re-render and move: build the ops from the DB and apply (the `organize` path).
    let rows = track_render_rows(&pool.open().unwrap()).unwrap();
    let template = conservatory_core::PathTemplate::default_music();
    let ops = rows
        .iter()
        .map(|row| {
            let fields = conservatory_core::TrackFields {
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
            conservatory_core::MoveOp {
                track_id: Some(row.track_id),
                album_id: row.album_id,
                src: lib.root.join(&row.file_path),
                dst: lib.root.join(&rel),
                db_old: Some(row.file_path.clone()),
                db_new: Some(rel.to_string_lossy().into_owned()),
            }
        })
        .collect();

    conservatory_core::mover::apply(
        &worker,
        &pool,
        conservatory_core::MoveKind::Organize,
        MoveMode::Move,
        &lib.root,
        0,
        ops,
    )
    .await
    .unwrap();
    worker.shutdown_ack().await.unwrap();

    // Every file now lives under Jazz/, and no files remain under Ambient/
    // (organize leaves empty directories behind; only files matter).
    for row in track_render_rows(&pool.open().unwrap()).unwrap() {
        assert!(row.file_path.starts_with("Jazz/"), "{}", row.file_path);
        assert!(lib.root.join(&row.file_path).exists());
    }
    assert_eq!(
        count_files(&lib.root.join("Ambient")),
        0,
        "no files left in Ambient/"
    );
    assert_eq!(
        count_files(&lib.root.join("Jazz")),
        4,
        "all four files under Jazz/"
    );
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() { count_files(&p) } else { 1 }
        })
        .sum()
}
