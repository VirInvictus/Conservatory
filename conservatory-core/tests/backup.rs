//! Backup / restore integration (spec §9, the data-safety contract): a backup
//! is a consistent snapshot taken through the single-writer worker, restore is
//! the documented replace path followed by a reopening that runs the
//! migrations, and every read-only attach refuses writes throughout.

use std::path::{Path, PathBuf};

use conservatory_core::backup;
use conservatory_core::db::{Album, ReadPool, Track, WorkerHandle, get_track, spawn_worker};
use tempfile::{TempDir, tempdir};

/// A library db + worker + pool, with the directories kept alive for the test.
struct Fixture {
    _dbdir: TempDir,
    _outdir: TempDir,
    db: PathBuf,
    worker: WorkerHandle,
    pool: ReadPool,
}

async fn fixture() -> Fixture {
    let dbdir = tempdir().unwrap();
    let outdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    Fixture {
        _dbdir: dbdir,
        _outdir: outdir,
        db,
        worker,
        pool,
    }
}

/// Insert artist + album + one track titled `title`, through `worker`. The
/// artist and album resolve idempotently, so repeated seeds only add tracks.
async fn seed(worker: &WorkerHandle, title: &str, file_path: &str) -> i64 {
    let artist = worker
        .get_or_create_artist("Artist".into(), "Artist".into(), None)
        .await
        .unwrap();
    let album = worker
        .get_or_create_album(Album {
            id: 0,
            title: "Album".into(),
            album_artist_id: Some(artist),
            shelf_genre: Some("Rock".into()),
            year: Some(2001),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "old".into(),
            added_at: None,
        })
        .await
        .unwrap();
    worker
        .insert_track(Track {
            id: 0,
            album_id: Some(album),
            artist_id: Some(artist),
            title: title.to_string(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: None,
            file_path: file_path.to_string(),
            format: Some("flac".into()),
            bitrate: None,
            sample_rate: None,
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: None,
        })
        .await
        .unwrap()
}

fn track_title(pool: &ReadPool, id: i64) -> String {
    let conn = pool.open().unwrap();
    get_track(&conn, id).unwrap().unwrap().title
}

fn track_count(pool: &ReadPool) -> i64 {
    let conn = pool.open().unwrap();
    conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap()
}

fn backup_path(fx: &Fixture) -> PathBuf {
    fx._outdir.path().join("library.db.backup")
}

#[tokio::test]
async fn backup_produces_a_restorable_snapshot() {
    let fx = fixture().await;
    let one = seed(&fx.worker, "one", "old/01.flac").await;
    let out = backup_path(&fx);

    backup::backup(&fx.worker, &out).await.unwrap();

    assert!(out.exists());
    // The snapshot is a working library: its read pool sees the seeded track.
    let snap = ReadPool::new(out.clone(), 1).unwrap();
    assert_eq!(track_count(&snap), 1);
    assert_eq!(track_title(&snap, one), "one");
    // A backup never overwrites: a second run to the same target refuses.
    assert!(backup::backup(&fx.worker, &out).await.is_err());
    // The live database is untouched by the snapshot and still writable.
    seed(&fx.worker, "still-live", "old/02.flac").await;
    assert_eq!(track_count(&fx.pool), 2);

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn restore_round_trips_through_the_replace_path() {
    let fx = fixture().await;
    let one = seed(&fx.worker, "one", "old/01.flac").await;
    let out = backup_path(&fx);
    backup::backup(&fx.worker, &out).await.unwrap();

    // A post-backup mutation (a second track) must not survive the restore.
    let two = seed(&fx.worker, "two", "old/02.flac").await;
    assert_eq!(track_count(&fx.pool), 2);

    // Nothing may hold the writer open across the file replace.
    fx.worker.shutdown_ack().await.unwrap();
    backup::restore(&fx.db, &out).unwrap();

    // The replaced file carries the backup's content: "one" back, "two" gone.
    let pool = ReadPool::new(fx.db.clone(), 3).unwrap();
    assert_eq!(track_count(&pool), 1);
    assert_eq!(track_title(&pool, one), "one");
    assert!(get_track(&pool.open().unwrap(), two).unwrap().is_none());

    // Reopening proves the restored file is a working library: the worker runs
    // the migrations on it and accepts writes again.
    let worker = spawn_worker(fx.db.clone()).unwrap();
    let three = seed(&worker, "three", "restored/01.flac").await;
    assert_eq!(track_title(&pool, three), "three");
    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn read_only_attaches_refuse_writes_through_backup_and_restore() {
    let fx = fixture().await;
    seed(&fx.worker, "one", "old/01.flac").await;
    let out = backup_path(&fx);

    // After the backup: the live database's read-only pool stays read-only.
    backup::backup(&fx.worker, &out).await.unwrap();
    refuses_writes(&fx.db);

    // The snapshot file is a database too, and its attach is read-only in the
    // same way: reading works, writing is refused at the engine level.
    refuses_writes(&out);

    // After a restore, the replaced database is read-only to readers as well.
    fx.worker.shutdown_ack().await.unwrap();
    backup::restore(&fx.db, &out).unwrap();
    refuses_writes(&fx.db);
}

/// Open a read-only attach on `db` and prove a write is refused at the engine
/// level (the connection.rs contract, held across the backup/replace path).
fn refuses_writes(db: &Path) {
    let pool = ReadPool::new(db.to_path_buf(), 1).unwrap();
    let conn = pool.open().unwrap();
    let result = conn.execute("CREATE TABLE smuggled (id INTEGER)", []);
    assert!(result.is_err(), "read-only attach must refuse writes");
}
