//! Phase 6c-iii-d integration tests: the sleep timer driving the threaded engine
//! through a null audio output. The pure clock logic is unit-tested in
//! `player::sleep`; here we prove the engine enforces it — a duration timer pauses
//! playback mid-queue (and tap-to-extend re-arms it), and an "end of item" timer
//! pauses at the item boundary without advancing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conservatory_core::db::{MediaKind, ReadPool, get_track, search_rows, spawn_worker};
use conservatory_core::player::{self, SleepMode};
use conservatory_core::{
    ImportOptions, MoveMode, PlayableItem, PlaybackConfig, PlayerHandle, import_folder,
    resolve_music_profile,
};
use tempfile::tempdir;

/// Import the four committed fixtures into a managed tree and resolve them to a
/// playable queue (the `engine_plays_queue_to_end` setup). Returns the player, the
/// pool, the imported track ids, and keeps the runtime/worker alive via the caller.
struct Harness {
    worker: conservatory_core::db::WorkerHandle,
    pool: ReadPool,
    items: Vec<PlayableItem>,
    ids: Vec<i64>,
    _dbdir: tempfile::TempDir,
    _libdir: tempfile::TempDir,
    _srcdir: tempfile::TempDir,
    // Declared last so it drops last: the worker (and any runtime-bound state) must
    // tear down before the runtime, the queue.rs engine-test ordering. Dropping the
    // runtime first wedges the process at the end of the test.
    runtime: tokio::runtime::Runtime,
}

fn harness() -> Harness {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();

    let dbdir = tempdir().unwrap();
    let libdir = tempdir().unwrap();
    let srcdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let root = libdir.path().to_path_buf();

    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    for name in ["sample.flac", "sample.mp3", "sample.opus", "sample.m4a"] {
        std::fs::copy(fixtures_dir.join(name), srcdir.path().join(name)).unwrap();
    }

    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    runtime.block_on(async {
        let opts = ImportOptions {
            library_root: root.clone(),
            mode: MoveMode::Copy,
        };
        import_folder(&worker, &pool, srcdir.path(), &opts)
            .await
            .unwrap();
    });

    let cfg = PlaybackConfig::default();
    let (items, ids): (Vec<PlayableItem>, Vec<i64>) = {
        let conn = pool.open().unwrap();
        let mut items = Vec::new();
        let mut ids = Vec::new();
        for row in search_rows(&conn).unwrap() {
            let track = get_track(&conn, row.track_id).unwrap().unwrap();
            ids.push(track.id);
            items.push(PlayableItem {
                track_id: track.id,
                source: root.join(&track.file_path),
                profile: resolve_music_profile(&track, &cfg),
                album_id: track.album_id,
                kind: MediaKind::Track,
                streaming: false,
                chapters: [].into(),
                segments: [].into(),
            });
        }
        (items, ids)
    };
    assert_eq!(items.len(), 4, "all four fixtures import as tracks");

    Harness {
        runtime,
        worker,
        pool,
        items,
        ids,
        _dbdir: dbdir,
        _libdir: libdir,
        _srcdir: srcdir,
    }
}

/// Poll the snapshot until `pred` holds, failing on a wall-clock deadline so a
/// wedged engine cannot hang the test.
fn wait_until(player: &PlayerHandle, secs: u64, pred: impl Fn(&player::PlayerSnapshot) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if pred(&player.snapshot()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "condition not met before deadline"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A duration sleep timer pauses playback when it elapses (the queue is *not*
/// ended, so it stopped mid-stream), and pressing play within the tap-to-extend
/// window re-arms the same interval.
#[test]
fn after_timer_fires_pauses_and_tap_extends() {
    let h = harness();
    let player = player::spawn_null(h.worker.clone(), h.runtime.handle().clone()).unwrap();

    // The four 0.3 s fixtures play ~1.2 s end to end; a 0.5 s timer fires partway.
    player.play_queue(h.items.clone(), 0);
    player.set_sleep_timer(Some(SleepMode::After(0.5)));

    // It fires: playback pauses, the queue has not ended, and the snapshot reports
    // the timer fired (the tap-to-extend window is open).
    wait_until(&player, 30, |s| s.sleep.is_some_and(|sl| sl.fired));
    let snap = player.snapshot();
    assert!(snap.paused, "a fired timer pauses playback");
    assert!(!snap.ended, "the timer stopped mid-queue, not at its end");

    // Tap-to-extend: pressing play re-arms the full interval and clears `fired`.
    // (A `Some` + `!fired` snapshot can only come from a re-arm: firing leaves
    // remaining at 0, and the 30 s window cannot lapse inside this 5 s wait.) The
    // observed remaining is the re-armed 0.5 s minus the command + poll latency, so
    // assert it bounced well clear of zero rather than at a tight threshold.
    player.play();
    wait_until(&player, 5, |s| s.sleep.is_some_and(|sl| !sl.fired));
    let re = player.snapshot().sleep.expect("timer still armed");
    assert!(
        re.remaining.is_some_and(|r| r > 0.2),
        "play within the window re-armed the 0.5 s interval, got {:?}",
        re.remaining
    );

    player.shutdown();
    h.runtime.block_on(h.worker.shutdown_ack()).ok();
}

/// An "end of item" sleep timer pauses at the current item's boundary instead of
/// advancing: the first item plays to completion, the second is cued but paused
/// (never played), and the timer disarms.
#[test]
fn end_of_item_pauses_at_the_boundary() {
    let h = harness();
    let player = player::spawn_null(h.worker.clone(), h.runtime.handle().clone()).unwrap();

    // A two-item queue; stop after the first finishes.
    let two: Vec<PlayableItem> = h.items.iter().take(2).cloned().collect();
    player.play_queue(two, 0);
    player.set_sleep_timer(Some(SleepMode::EndOfItem));

    // The first item ends, the engine pauses cued on the second, and disarms.
    wait_until(&player, 30, |s| {
        s.current_index == Some(1) && s.paused && s.sleep.is_none()
    });
    let snap = player.snapshot();
    assert!(
        !snap.ended,
        "the queue did not end; it paused at the boundary"
    );

    player.shutdown();

    // The first item played once; the second never played (it was held paused).
    let conn = h.pool.open().unwrap();
    let first = get_track(&conn, h.ids[0]).unwrap().unwrap();
    let second = get_track(&conn, h.ids[1]).unwrap().unwrap();
    assert_eq!(first.play_count, 1, "the first item completed");
    assert_eq!(second.play_count, 0, "the second item was never played");

    h.runtime.block_on(h.worker.shutdown_ack()).ok();
}
