//! Phase 2c integration suite for the file mover. Release-blocking (spec §5.4,
//! CLAUDE.md): moving the user's files is the headline risk, so the move/undo
//! round-trips, crash replay, conflict refusal, and tree↔DB consistency are
//! verified end to end against real files on disk.

use std::fs;
use std::path::Path;

use conservatory_core::db::{Album, ReadPool, Track, WorkerHandle, get_track, spawn_worker};
use conservatory_core::mover::journal::{self, JobState};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp, fsops};
use tempfile::{TempDir, tempdir};

/// A library root and a database, kept alive together for a test.
struct Fixture {
    _libdir: TempDir,
    _dbdir: TempDir,
    root: std::path::PathBuf,
    worker: WorkerHandle,
    pool: ReadPool,
}

async fn fixture() -> Fixture {
    let libdir = tempdir().unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    Fixture {
        root: libdir.path().to_path_buf(),
        _libdir: libdir,
        _dbdir: dbdir,
        worker,
        pool,
    }
}

fn track_row(album_id: i64, title: &str, file_path: &str) -> Track {
    Track {
        id: 0,
        album_id: Some(album_id),
        artist_id: None,
        title: title.to_string(),
        track_no: Some(1),
        disc_no: Some(1),
        duration: None,
        file_path: file_path.to_string(),
        format: Some("flac".to_string()),
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
    }
}

fn stage(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn op(root: &Path, track_id: i64, album_id: i64, old: &str, new: &str) -> MoveOp {
    MoveOp {
        track_id: Some(track_id),
        album_id: Some(album_id),
        src: root.join(old),
        dst: root.join(new),
        db_old: Some(old.to_string()),
        db_new: Some(new.to_string()),
    }
}

/// Insert an album with `n` tracks at `old/NN.flac`, stage the files, and return
/// (album_id, [(track_id, old_rel, new_rel)]).
async fn seed(fx: &Fixture, n: usize) -> (i64, Vec<(i64, String, String)>) {
    let album = fx
        .worker
        .insert_album(Album {
            id: 0,
            title: "Album".into(),
            album_artist_id: None,
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

    let mut out = Vec::new();
    for i in 0..n {
        let old = format!("old/{i}.flac");
        let new = format!("new/{i}.flac");
        let id = fx
            .worker
            .insert_track(track_row(album, &format!("t{i}"), &old))
            .await
            .unwrap();
        stage(&fx.root, &old, format!("audio-{i}").as_bytes());
        out.push((id, old, new));
    }
    (album, out)
}

fn job_state(fx: &Fixture, job_id: i64) -> JobState {
    let conn = fx.pool.open().unwrap();
    journal::get_job(&conn, job_id).unwrap().unwrap().state
}

fn db_path(fx: &Fixture, track_id: i64) -> String {
    let conn = fx.pool.open().unwrap();
    get_track(&conn, track_id).unwrap().unwrap().file_path
}

#[tokio::test]
async fn move_round_trip_updates_tree_and_db() {
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 3).await;
    let ops = tracks
        .iter()
        .map(|(id, old, new)| op(&fx.root, *id, album, old, new))
        .collect();

    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Organize,
        MoveMode::Move,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    for (i, (id, old, new)) in tracks.iter().enumerate() {
        assert!(!fx.root.join(old).exists(), "source {old} should be gone");
        // seed() staged each file as "audio-{i}".
        assert_eq!(
            fs::read(fx.root.join(new)).unwrap(),
            format!("audio-{i}").as_bytes()
        );
        assert_eq!(&db_path(&fx, *id), new, "track {id} file_path");
    }
    // Album folder_path resynced to the new parent.
    let conn = fx.pool.open().unwrap();
    let album_row = conservatory_core::db::get_album(&conn, album)
        .unwrap()
        .unwrap();
    assert_eq!(album_row.folder_path, "new");
    assert_eq!(job_state(&fx, job), JobState::Completed);

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn undo_restores_tree_and_db() {
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 3).await;
    let ops = tracks
        .iter()
        .map(|(id, old, new)| op(&fx.root, *id, album, old, new))
        .collect();

    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Organize,
        MoveMode::Move,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    mover::undo(&fx.worker, &fx.pool, job).await.unwrap();

    for (id, old, new) in &tracks {
        assert!(fx.root.join(old).exists(), "source {old} should be back");
        assert!(!fx.root.join(new).exists(), "target {new} should be gone");
        assert_eq!(&db_path(&fx, *id), old, "track {id} file_path reverted");
    }
    assert_eq!(job_state(&fx, job), JobState::Undone);

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn crash_mid_job_rolls_forward_on_recovery() {
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 4).await;
    let ops: Vec<MoveOp> = tracks
        .iter()
        .map(|(id, old, new)| op(&fx.root, *id, album, old, new))
        .collect();

    // Journal the job (the durable record), then simulate a crash: move the
    // first two files by hand (as the engine would) but never mark them done,
    // leaving the job in_progress with all ops still pending.
    let job = fx
        .worker
        .create_move_job(
            MoveKind::Organize,
            MoveMode::Move,
            fx.root.to_string_lossy().into_owned(),
            0,
            ops.clone(),
        )
        .await
        .unwrap();
    fsops::relocate(&ops[0].src, &ops[0].dst, MoveMode::Move).unwrap();
    fsops::relocate(&ops[1].src, &ops[1].dst, MoveMode::Move).unwrap();
    assert_eq!(job_state(&fx, job), JobState::InProgress);

    // Recovery rolls forward: the already-moved files are idempotent no-ops, the
    // rest move, and every op + the job is finalized.
    let recovered = mover::recover(&fx.worker, &fx.pool).await.unwrap();
    assert_eq!(recovered, 1);

    for (id, old, new) in &tracks {
        assert!(!fx.root.join(old).exists());
        assert!(fx.root.join(new).exists());
        assert_eq!(&db_path(&fx, *id), new);
    }
    assert_eq!(job_state(&fx, job), JobState::Completed);

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn apply_refuses_on_conflicts() {
    // Duplicate target: two tracks rendered to the same destination.
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 2).await;
    let ops = vec![
        op(&fx.root, tracks[0].0, album, &tracks[0].1, "new/dup.flac"),
        op(&fx.root, tracks[1].0, album, &tracks[1].1, "new/dup.flac"),
    ];
    assert!(
        mover::apply(
            &fx.worker,
            &fx.pool,
            MoveKind::Organize,
            MoveMode::Move,
            &fx.root,
            0,
            ops,
        )
        .await
        .is_err()
    );
    // Nothing moved: sources intact.
    for (_, old, _) in &tracks {
        assert!(fx.root.join(old).exists());
    }

    // Missing source.
    let missing = vec![op(&fx.root, tracks[0].0, album, "nope.flac", "new/x.flac")];
    assert!(
        mover::apply(
            &fx.worker,
            &fx.pool,
            MoveKind::Organize,
            MoveMode::Move,
            &fx.root,
            0,
            missing,
        )
        .await
        .is_err()
    );

    // Existing target.
    stage(&fx.root, "new/taken.flac", b"already here");
    let taken = vec![op(
        &fx.root,
        tracks[0].0,
        album,
        &tracks[0].1,
        "new/taken.flac",
    )];
    assert!(
        mover::apply(
            &fx.worker,
            &fx.pool,
            MoveKind::Organize,
            MoveMode::Move,
            &fx.root,
            0,
            taken,
        )
        .await
        .is_err()
    );

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn copy_mode_keeps_sources_and_undo_removes_targets() {
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 2).await;
    let ops = tracks
        .iter()
        .map(|(id, old, new)| op(&fx.root, *id, album, old, new))
        .collect();

    let job = mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Organize,
        MoveMode::Copy,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    for (_, old, new) in &tracks {
        assert!(fx.root.join(old).exists(), "copy keeps source {old}");
        assert!(fx.root.join(new).exists(), "copy creates target {new}");
    }

    mover::undo(&fx.worker, &fx.pool, job).await.unwrap();
    for (_, old, new) in &tracks {
        assert!(
            fx.root.join(old).exists(),
            "source still present after undo"
        );
        assert!(!fx.root.join(new).exists(), "copy target removed by undo");
    }

    fx.worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn tree_and_db_stay_consistent_after_move() {
    // The spec §5.6 rebuildable-subset invariant in miniature: every track's DB
    // file_path points to a file that actually exists under the root.
    let fx = fixture().await;
    let (album, tracks) = seed(&fx, 3).await;
    let ops = tracks
        .iter()
        .map(|(id, old, new)| op(&fx.root, *id, album, old, new))
        .collect();
    mover::apply(
        &fx.worker,
        &fx.pool,
        MoveKind::Organize,
        MoveMode::Move,
        &fx.root,
        0,
        ops,
    )
    .await
    .unwrap();

    for (id, _, _) in &tracks {
        let rel = db_path(&fx, *id);
        assert!(
            fx.root.join(&rel).exists(),
            "db path {rel} must exist on disk"
        );
    }

    fx.worker.shutdown_ack().await.unwrap();
}
