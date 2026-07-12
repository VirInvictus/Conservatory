//! Phase 9b integration tests: the engine's play-completion hook enqueues a
//! listen into `scrobble_outbox` on a natural EOF when scrobbling is on, and is
//! a true no-op when it is off. Drives the real threaded engine through a null
//! audio output (the `tests/queue.rs` precedent). Audiobooks are excluded at the
//! data layer (`scrobble_source` returns `None`), covered by a unit check.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conservatory_core::db::{
    MediaKind, ReadPool, count_pending_scrobbles, get_track, pending_scrobbles, scrobble_source,
    search_rows, spawn_worker,
};
use conservatory_core::player;
use conservatory_core::{
    ImportOptions, MoveMode, PlayableItem, PlaybackConfig, ScrobbleService, import_folder,
    resolve_music_profile,
};
use tempfile::tempdir;

/// Import the four committed audio fixtures into a managed tree and return the
/// worker, pool, and the resolved playable items (the `tests/queue.rs` setup).
fn import_fixtures(
    runtime: &tokio::runtime::Runtime,
    db: &std::path::Path,
    root: &std::path::Path,
) -> (
    conservatory_core::db::WorkerHandle,
    ReadPool,
    Vec<PlayableItem>,
) {
    let srcdir = tempdir().unwrap();
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    for name in ["sample.flac", "sample.mp3", "sample.opus", "sample.m4a"] {
        std::fs::copy(fixtures_dir.join(name), srcdir.path().join(name)).unwrap();
    }

    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.to_path_buf()).unwrap()
    };
    let pool = ReadPool::new(db.to_path_buf(), 3).unwrap();
    runtime.block_on(async {
        let opts = ImportOptions {
            library_root: root.to_path_buf(),
            mode: MoveMode::Copy,
        };
        import_folder(&worker, &pool, srcdir.path(), &opts)
            .await
            .unwrap();
    });

    let cfg = PlaybackConfig::default();
    let items: Vec<PlayableItem> = {
        let conn = pool.open().unwrap();
        search_rows(&conn)
            .unwrap()
            .into_iter()
            .map(|row| {
                let track = get_track(&conn, row.track_id).unwrap().unwrap();
                PlayableItem {
                    track_id: track.id,
                    source: root.join(&track.file_path),
                    profile: resolve_music_profile(&track, &cfg),
                    album_id: track.album_id,
                    kind: MediaKind::Track,
                    streaming: false,
                    chapters: [].into(),
                    segments: [].into(),
                }
            })
            .collect()
    };
    assert_eq!(items.len(), 4, "all four fixtures should import as tracks");
    (worker, pool, items)
}

/// Play `items` to the end of the queue, polling the snapshot with a wall-clock
/// guard, then shut the engine down.
fn play_to_end(player: &conservatory_core::PlayerHandle, items: Vec<PlayableItem>) {
    player.play_queue(items, 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "engine did not finish the queue in time"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();
}

#[test]
fn scrobble_enabled_enqueues_completed_tracks() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let libdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let (worker, pool, items) = import_fixtures(&runtime, &db, libdir.path());

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.set_scrobble(Some(ScrobbleService::ListenBrainz));
    play_to_end(&player, items);

    // Every completed track queued exactly one listen, bound for the configured
    // service, with the descriptive metadata snapshotted.
    let conn = pool.open().unwrap();
    assert_eq!(
        count_pending_scrobbles(&conn).unwrap(),
        4,
        "each of the four completed tracks should enqueue a listen"
    );
    let rows = pending_scrobbles(&conn, i64::MAX, 50).unwrap();
    assert_eq!(rows.len(), 4);
    for row in &rows {
        assert_eq!(row.service, "listenbrainz");
        assert_eq!(row.kind, "track");
        assert!(!row.artist.is_empty(), "artist snapshotted");
        assert!(!row.track.is_empty(), "title snapshotted");
    }

    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn scrobble_disabled_enqueues_nothing() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let libdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let (worker, pool, items) = import_fixtures(&runtime, &db, libdir.path());

    // No set_scrobble call: scrobbling defaults off. Playing the whole queue must
    // leave the outbox empty (the subsystem is inert until enabled).
    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    play_to_end(&player, items);

    let conn = pool.open().unwrap();
    assert_eq!(
        count_pending_scrobbles(&conn).unwrap(),
        0,
        "scrobbling off must enqueue nothing"
    );

    runtime.block_on(worker.shutdown_ack()).ok();
}

#[test]
fn scrobble_source_never_scrobbles_audiobooks() {
    // A book is not a "listen": the resolver returns None for any book id without
    // even reading, so the engine hook can never queue one (spec §9 scope). The
    // id is irrelevant; the Audiobook arm short-circuits.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    let conn = pool.open().unwrap();
    assert!(
        scrobble_source(&conn, MediaKind::Audiobook, 1)
            .unwrap()
            .is_none()
    );

    runtime.block_on(worker.shutdown_ack()).ok();
}
