//! Phase 5d integration tests: import writes a cover to disk and records
//! `cover_path`, and a path-affecting edit + organize moves the cover with its
//! album. The committed fixtures carry an embedded PNG cover. CI-hermetic.

use std::path::{Path, PathBuf};

use conservatory_core::db::{ReadPool, WorkerHandle, get_album, spawn_worker, track_render_rows};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp};
use conservatory_core::{
    AlbumEdit, ImportOptions, PathTemplate, TrackFields, import_folder, resync_album_covers,
};
use tempfile::tempdir;

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

async fn managed_lib(dir: &Path) -> (ReadPool, WorkerHandle, PathBuf) {
    let db = dir.join("lib.db");
    let lib = dir.join("lib");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for f in ["sample.flac", "sample.mp3", "sample.m4a", "sample.opus"] {
        std::fs::copy(fixture_audio(f), src.join(f)).unwrap();
    }
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    import_folder(
        &worker,
        &pool,
        &src,
        &ImportOptions {
            library_root: lib.clone(),
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();
    (pool, worker, lib)
}

#[tokio::test]
async fn import_writes_cover_and_records_path() {
    let dir = tempdir().unwrap();
    let (pool, worker, lib) = managed_lib(dir.path()).await;

    let album = {
        let conn = pool.open().unwrap();
        get_album(&conn, 1).unwrap().unwrap()
    };
    let cover_path = album.cover_path.expect("cover_path set on import");
    assert!(
        lib.join(&cover_path).exists(),
        "cover written to disk: {cover_path}"
    );
    assert!(cover_path.ends_with("cover.png"), "fixtures embed a PNG");
    assert!(album.accent_rgb.is_some(), "accent computed from the cover");
    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn cover_follows_a_path_affecting_edit() {
    let dir = tempdir().unwrap();
    let (pool, worker, lib) = managed_lib(dir.path()).await;
    let before = {
        let conn = pool.open().unwrap();
        get_album(&conn, 1).unwrap().unwrap().cover_path.unwrap()
    };

    // A year edit moves the album folder; organize relocates the tracks; the
    // cover-resync then moves the cover to match.
    worker
        .update_album(
            1,
            AlbumEdit {
                year: Some(1990),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    organize_all(&worker, &pool, &lib).await;
    let moved = resync_album_covers(&worker, &pool, &lib).await.unwrap();
    assert!(moved >= 1, "a cover was moved");

    let after = {
        let conn = pool.open().unwrap();
        get_album(&conn, 1).unwrap().unwrap().cover_path.unwrap()
    };
    assert_ne!(before, after, "cover_path updated");
    assert!(after.contains("(1990)"), "cover under the new year");
    assert!(lib.join(&after).exists(), "cover at the new location");
    assert!(!lib.join(&before).exists(), "stale cover removed");
    worker.shutdown_ack().await.unwrap();
}

/// Re-render every track from the DB and move to match (the organize flow).
async fn organize_all(worker: &WorkerHandle, pool: &ReadPool, root: &Path) {
    let rows = {
        let conn = pool.open().unwrap();
        track_render_rows(&conn).unwrap()
    };
    let template = PathTemplate::default_music();
    let ops: Vec<MoveOp> = rows
        .iter()
        .map(|row| {
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
            MoveOp {
                track_id: Some(row.track_id),
                album_id: row.album_id,
                src: root.join(&row.file_path),
                dst: root.join(&rel),
                db_old: Some(row.file_path.clone()),
                db_new: Some(rel.to_string_lossy().into_owned()),
            }
        })
        .collect();
    mover::apply(
        worker,
        pool,
        MoveKind::Organize,
        MoveMode::Move,
        root,
        0,
        ops,
    )
    .await
    .unwrap();
}
