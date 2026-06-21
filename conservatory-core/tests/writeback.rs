//! Phase 5b integration tests: embedded-tag write-back round-trips, the §5.6
//! re-import contract (the managed tree + embedded tags rebuild the descriptive
//! layer), and the APEv2 strip. Committed fixtures keep CI hermetic.

use std::path::{Path, PathBuf};

use conservatory_core::db::WritebackRow;
use conservatory_core::db::{ReadPool, WorkerHandle, get_album, spawn_worker, writeback_rows};
use conservatory_core::mover::MoveMode;
use conservatory_core::{
    AlbumEdit, ImportOptions, TagWrite, import_folder, read_track, write_track_tags,
};
use tempfile::tempdir;

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// Import the four committed fixtures (copy) into `<dir>/lib`, returning the
/// pool + worker. The library root is `<dir>/lib`.
async fn managed_lib(dir: &Path) -> (ReadPool, WorkerHandle) {
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
            library_root: lib,
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();
    (pool, worker)
}

fn target_from(r: &WritebackRow) -> TagWrite {
    TagWrite {
        title: r.title.clone(),
        track_artist: r.track_artist.clone(),
        track_artist_sort: r.track_artist_sort.clone(),
        album: r.album.clone(),
        album_artist: r.album_artist.clone(),
        album_artist_sort: r.album_artist_sort.clone(),
        year: r.year,
        track_no: r.track_no,
        disc_no: r.disc_no,
        genres: r.genres.clone(),
    }
}

#[tokio::test]
async fn embed_round_trips_descriptive_fields() {
    let dir = tempdir().unwrap();
    let (pool, worker) = managed_lib(dir.path()).await;
    let lib = dir.path().join("lib");

    worker
        .update_album(
            1,
            AlbumEdit {
                title: Some("RT Album".into()),
                year: Some(1995),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let ids: Vec<i64> = (1..=4).collect();
    let rows = {
        let conn = pool.open().unwrap();
        writeback_rows(&conn, &ids).unwrap()
    };
    for r in &rows {
        write_track_tags(&lib.join(&r.file_path), &target_from(r)).unwrap();
    }

    for r in &rows {
        let d = read_track(&lib.join(&r.file_path)).unwrap();
        assert_eq!(d.album.as_deref(), Some("RT Album"), "{}", r.file_path);
        assert_eq!(d.year, Some(1995), "{}", r.file_path);
        // Multi-value genre survives on Vorbis formats; ID3v2 (mp3) collapses it
        // on read, so only assert the set there is non-empty.
        let mut g = d.genres.clone();
        g.sort();
        if r.file_path.ends_with(".flac") || r.file_path.ends_with(".opus") {
            assert_eq!(g, vec!["Ambient".to_string(), "Electronic".to_string()]);
        } else {
            assert!(!g.is_empty(), "{} kept a genre", r.file_path);
        }
    }
    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn reimport_recovers_edited_descriptive_metadata() {
    let dir = tempdir().unwrap();
    let (pool, worker) = managed_lib(dir.path()).await;
    let lib = dir.path().join("lib");

    worker
        .update_album(
            1,
            AlbumEdit {
                title: Some("Self Describing".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let ids: Vec<i64> = (1..=4).collect();
    let rows = {
        let conn = pool.open().unwrap();
        writeback_rows(&conn, &ids).unwrap()
    };
    for r in &rows {
        write_track_tags(&lib.join(&r.file_path), &target_from(r)).unwrap();
    }
    worker.shutdown_ack().await.unwrap();
    drop(pool);

    // Fresh DB: re-import the managed tree. The edit must survive because it is
    // in the files now (§5.6).
    let db2 = dir.path().join("fresh.db");
    let worker2 = spawn_worker(db2.clone()).unwrap();
    let pool2 = ReadPool::new(db2, 3).unwrap();
    import_folder(
        &worker2,
        &pool2,
        &lib,
        &ImportOptions {
            library_root: dir.path().join("lib2"),
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();

    let conn = pool2.open().unwrap();
    let alb = get_album(&conn, 1).unwrap().unwrap();
    assert_eq!(alb.title, "Self Describing");
    worker2.shutdown_ack().await.unwrap();
}
