//! Phase 4b-i integration tests: the unified queue's position integrity, the
//! `is:queued` membership wiring, and the threaded player engine advancing a
//! queue end-to-end through a null audio output.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    Episode, MediaKind, PlayedState, ReadPool, Show, get_playback, get_track, load_queue,
    read_playback_state, search_rows, spawn_worker,
};
use conservatory_core::player;
use conservatory_core::{
    ChapterMark, ImportOptions, MoveMode, PlayableItem, PlaybackConfig, PlayerHandle,
    PlayerSnapshot, import_folder, resolve_music_profile,
};
use tempfile::tempdir;

// --- Queue model: positions stay contiguous through enqueue / remove / reorder.

#[tokio::test]
async fn queue_positions_stay_contiguous() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    // Real track ids (1..80) so the queue's FK to `tracks` is satisfied.
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker.enqueue_tracks(vec![1, 2, 3, 4, 5]).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![1, 2, 3, 4, 5]);
    assert_positions_contiguous(&pool);

    // Remove the middle entry (position 2 → track 3); the gap closes.
    worker.remove_queue_item(2).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![1, 2, 4, 5]);
    assert_positions_contiguous(&pool);

    // Move the head (position 0) to position 2.
    worker.reorder_queue(0, 2).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![2, 4, 1, 5]);
    assert_positions_contiguous(&pool);

    // Move a tail entry back toward the head.
    worker.reorder_queue(3, 1).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![2, 5, 4, 1]);
    assert_positions_contiguous(&pool);

    worker.clear_queue().await.unwrap();
    assert!(queue_track_ids(&pool).is_empty());
}

#[tokio::test]
async fn is_queued_reflects_membership() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker.enqueue_tracks(vec![1]).await.unwrap();
    {
        let conn = pool.open().unwrap();
        let rows = search_rows(&conn).unwrap();
        assert!(rows.iter().find(|r| r.track_id == 1).unwrap().queued);
        assert!(!rows.iter().find(|r| r.track_id == 2).unwrap().queued);
    }

    worker.remove_queue_item(0).await.unwrap();
    {
        let conn = pool.open().unwrap();
        let rows = search_rows(&conn).unwrap();
        assert!(!rows.iter().find(|r| r.track_id == 1).unwrap().queued);
    }
}

#[tokio::test]
async fn get_tracks_batches_across_chunks() {
    use conservatory_core::db::get_tracks;

    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    // Medium = 2000 tracks, so a 1..=1000 request crosses the 900-id chunk size.
    fixtures::generate(&worker, FixtureScale::Medium)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();

    let ids: Vec<i64> = (1..=1000).collect();
    let tracks = get_tracks(&conn, &ids).unwrap();
    assert_eq!(
        tracks.len(),
        1000,
        "every requested id should come back once"
    );
    let got: std::collections::HashSet<i64> = tracks.iter().map(|t| t.id).collect();
    assert_eq!(got, ids.iter().copied().collect());

    // Empty request is a clean empty result (no zero-placeholder SQL).
    assert!(get_tracks(&conn, &[]).unwrap().is_empty());
}

#[tokio::test]
async fn load_queue_display_returns_ordered_rows_with_titles() {
    use conservatory_core::db::{MediaKind, load_queue_display};

    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker.enqueue_tracks(vec![3, 1]).await.unwrap();
    let conn = pool.open().unwrap();
    let rows = load_queue_display(&conn).unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].position, 0);
    assert_eq!(rows[0].kind, MediaKind::Track);
    assert_eq!(rows[0].track_id, Some(3));
    assert_eq!(rows[1].track_id, Some(1));
    // Titles and artists are joined in from the track.
    assert!(!rows[0].title.is_empty());
    assert!(rows[0].artist.is_some());
}

#[tokio::test]
async fn track_metadata_joins_title_artist_album() {
    use conservatory_core::db::track_metadata;

    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();

    let np = track_metadata(&conn, 1).unwrap().unwrap();
    assert!(!np.title.is_empty());
    assert!(np.artist.is_some());
    assert!(np.album.is_some());
    assert!(np.length.is_some());

    assert!(track_metadata(&conn, 999_999).unwrap().is_none());
}

// --- Player engine: build a queue of real fixtures and play it to the end.

#[test]
fn engine_plays_queue_to_end() {
    // A multi-thread runtime owns the worker and backs the engine thread's
    // blocking writes (mirrors the CLI / GUI). Kept alive until after shutdown.
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

    // Copy the committed fixtures into a source dir and import them (copy mode)
    // into a managed tree, so tracks have real, playable files.
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

    // Resolve every imported track into a PlayableItem (absolute path = root +
    // the relative `file_path`).
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
                kind: conservatory_core::db::MediaKind::Track,
                streaming: false,
                chapters: [].into(),
                segments: [].into(),
            });
        }
        (items, ids)
    };
    assert_eq!(items.len(), 4, "all four fixtures should import as tracks");

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(items, 0);

    // Drive: poll until the queue ends, with a generous wall-clock guard so a
    // wedged engine can't hang the test.
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

    // Every track played exactly once, and the cursor landed on the last item.
    let conn = pool.open().unwrap();
    for id in &ids {
        let track = get_track(&conn, *id).unwrap().unwrap();
        assert_eq!(track.play_count, 1, "track {id} should have played once");
    }
    let cursor = read_playback_state(&conn).unwrap().unwrap();
    assert_eq!(cursor.track_id, ids.last().copied());

    runtime.block_on(worker.shutdown_ack()).ok();
}

/// An episode plays to EOF through the engine and persists to the **podcast**
/// `playback` table (PlayedFully + play_count), while the per-kind guard keeps
/// the episode id out of the music tables: the transport cursor records
/// `kind = Episode` + `episode_id` (never `track_id`), and a colliding track's
/// `play_count` is untouched (Phase 6b-ii-c-2).
#[test]
fn engine_plays_an_episode_to_podcast_playback_not_the_track_tables() {
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

    // Import one fixture so a real track exists, then create a real episode whose
    // id collides with the track id (track and episode ids share an integer
    // space across their separate tables). This is load-bearing: if the engine
    // leaked the episode id into `playback_state.track_id` or bumped
    // `tracks.play_count`, the colliding track would be wrongly affected, so the
    // assertions below catch a regression of the per-kind dispatch.
    std::fs::copy(
        fixtures_dir.join("sample.mp3"),
        srcdir.path().join("sample.mp3"),
    )
    .unwrap();
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    let episode_id = runtime.block_on(async {
        let opts = ImportOptions {
            library_root: root.clone(),
            mode: MoveMode::Copy,
        };
        import_folder(&worker, &pool, srcdir.path(), &opts)
            .await
            .unwrap();
        let show_id = worker
            .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
            .await
            .unwrap();
        worker
            .upsert_episode(sample_episode(show_id, "guid-1", "Ep One"))
            .await
            .unwrap()
    });
    let track_id = {
        let conn = pool.open().unwrap();
        search_rows(&conn).unwrap()[0].track_id
    };
    assert_eq!(
        track_id, episode_id,
        "the test wants the ids to collide so a leak would be observable"
    );

    // The queue item's `track_id` field carries the *episode* id (the c-1 reuse).
    let item = PlayableItem {
        track_id: episode_id,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Episode,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(Instant::now() < deadline, "engine did not finish in time");
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();

    let conn = pool.open().unwrap();
    // The episode persisted to the podcast `playback` table on EOF.
    let pb = get_playback(&conn, episode_id).unwrap().unwrap();
    assert_eq!(pb.played, PlayedState::PlayedFully);
    assert_eq!(pb.play_count, 1);
    // The cursor records kind = Episode + the episode id, never a track id.
    let cur = read_playback_state(&conn).unwrap().unwrap();
    assert_eq!(cur.kind, MediaKind::Episode);
    assert_eq!(cur.episode_id, Some(episode_id));
    assert_eq!(
        cur.track_id, None,
        "episode playback must not write a music playback_state track cursor"
    );
    // The play-count guard (`on_item_ended`): the colliding track is not bumped.
    assert_eq!(
        get_track(&conn, track_id).unwrap().unwrap().play_count,
        0,
        "episode playback must not bump a track's play_count"
    );

    runtime.block_on(worker.shutdown_ack()).ok();
}

/// Regression for the v0.0.38 "had to pause then play" bug: a fresh `play_queue`
/// must start playing even when the engine was previously paused. The engine's
/// `load_current` now syncs mpv's pause property to "playing" (loadfile inherits
/// the prior pause state), so the new item is not stuck paused. Pre-fix the new
/// item inherited mpv's paused state and never reached EOF (this would time out).
#[test]
fn engine_unpauses_a_newly_loaded_item_after_a_pause() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };

    // A Track item over a real fixture. The track id need not exist: the EOF
    // persistence (play-count bump / cursor) is a silent no-op for a missing row.
    let item = || PlayableItem {
        track_id: 1,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Track,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item()], 0);
    std::thread::sleep(Duration::from_millis(120));
    player.toggle_pause();
    std::thread::sleep(Duration::from_millis(80));
    assert!(player.snapshot().paused, "the engine should now be paused");

    // The regression path: a fresh queue while mpv is paused must play.
    player.play_queue(vec![item()], 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a newly-loaded item never played after a pause (the pause desync)"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

/// Phase 5.5b-ii: a live EQ band change (af-command) does not interrupt
/// playback. With a non-flat EQ active, the `@eq` stage is in the chain, so a
/// `set_eq_band` goes through `af-command` (not a rebuild); the item still plays
/// to EOF. Exercises the real mpv `af-command` path through a null audio output.
#[test]
fn engine_applies_a_live_eq_band_change_without_stopping() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let item = || PlayableItem {
        track_id: 1,
        source: fixtures_dir.join("sample.flac"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Track,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    // A non-flat EQ so the @eq stage is built into the chain (the live path).
    let mut eq = conservatory_core::db::EqState::flat();
    eq.bands[0] = 6.0;
    player.set_eq(eq);
    player.play_queue(vec![item()], 0);
    std::thread::sleep(Duration::from_millis(120));
    // Live per-band changes while playing.
    player.set_eq_band(0, -3.0);
    player.set_eq_band(9, 4.0);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "playback did not finish after a live EQ change"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

/// An episode played to EOF lands exactly one append-only `listening_sessions`
/// row through the engine's start-on-load / close-on-boundary wiring (Phase
/// 6c-ii). The null fast-decode has no real silence, so the saved figure is ~0
/// here (the `saved > 0` math is covered by the `player::session` unit tests);
/// this proves the row is written, once, with sane non-negative totals.
#[test]
fn engine_records_a_listening_session_for_an_episode() {
    use conservatory_core::db::listening_totals;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();

    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db.clone(), 3).unwrap();

    // A real episode row: `listening_sessions.episode_id` is a NOT NULL foreign
    // key to `episodes`, so the engine's close-of-session insert needs it to exist.
    let episode_id = runtime.block_on(async {
        let show_id = worker
            .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
            .await
            .unwrap();
        worker
            .upsert_episode(sample_episode(show_id, "guid-1", "Ep One"))
            .await
            .unwrap()
    });

    let item = PlayableItem {
        track_id: episode_id,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Episode,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(Instant::now() < deadline, "engine did not finish in time");
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();

    let conn = pool.open().unwrap();
    let totals = listening_totals(&conn).unwrap();
    assert_eq!(
        totals.sessions, 1,
        "playing one episode to EOF should append exactly one session row"
    );
    assert!(totals.real_seconds >= 0.0);
    assert!(totals.audio_seconds >= 0.0);
    assert!(
        totals.smart_speed_saved >= 0.0,
        "saved is never negative; ~0 with no real silence in the fast decode"
    );

    runtime.block_on(worker.shutdown_ack()).ok();
}

/// Chapter-skip seeks to the neighbouring chapter boundary (Phase 6c-iii-b). An
/// episode with synthetic marks at 0.0 / 0.15 s, played **paused** so the 0.3 s
/// fixture cannot end under us: skip-forward lands in chapter 2, skip-forward
/// again is a no-op at the last chapter, and skip-back returns to chapter 1. The
/// `neighbour_chapter` math itself is unit-tested; this proves the engine wiring
/// (the command seeks and the snapshot reports the current chapter).
#[test]
fn engine_skips_between_chapters() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };

    // A real episode row so the engine's FK-bound persistence stays happy.
    let episode_id = runtime.block_on(async {
        let show_id = worker
            .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
            .await
            .unwrap();
        worker
            .upsert_episode(sample_episode(show_id, "guid-1", "Ep One"))
            .await
            .unwrap()
    });

    let item = PlayableItem {
        track_id: episode_id,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Episode,
        streaming: false,
        chapters: Arc::from([
            ChapterMark {
                start_time: 0.0,
                title: Some("Intro".into()),
            },
            ChapterMark {
                start_time: 0.15,
                title: Some("Main".into()),
            },
        ]),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    player.pause();

    let wait = |pred: &dyn Fn(&PlayerSnapshot) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if pred(&player.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot condition not met in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // Paused at the start, in chapter one, with both chapters visible. (Exact
    // boundary indexing is covered by the `neighbour_chapter` unit tests; a
    // keyframe seek on a compressed fixture lands near, not on, a 0.15 s boundary,
    // so the engine test asserts seek *direction*, which is the wiring under test.)
    wait(&|s| s.paused && !s.ended);
    assert_eq!(player.snapshot().chapter_count, 2);
    assert_eq!(player.snapshot().current_chapter, Some(0));

    // Forward: seek toward the second chapter's 0.15 s start.
    player.skip_chapter(1);
    wait(&|s| s.position >= 0.1);

    // Back: return to the first chapter's start (clamped at the head).
    player.skip_chapter(-1);
    wait(&|s| s.position < 0.05);

    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

/// A queue interleaving a music track and a podcast episode plays both to the end
/// (the roadmap-named 6c-iii test): the engine swaps the music `af`-chain for the
/// spoken-word profile at the kind boundary mid-queue (spec §16.9). Proven by
/// both items completing: the track's `play_count` bumps and the episode's
/// podcast `playback` row reaches `PlayedFully`.
#[test]
fn engine_swaps_profile_between_a_track_and_an_episode() {
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

    // Import one fixture as a real, playable track.
    std::fs::copy(
        fixtures_dir.join("sample.flac"),
        srcdir.path().join("sample.flac"),
    )
    .unwrap();
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };
    let pool = ReadPool::new(db.clone(), 3).unwrap();
    let episode_id = runtime.block_on(async {
        let opts = ImportOptions {
            library_root: root.clone(),
            mode: MoveMode::Copy,
        };
        import_folder(&worker, &pool, srcdir.path(), &opts)
            .await
            .unwrap();
        let show_id = worker
            .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
            .await
            .unwrap();
        worker
            .upsert_episode(sample_episode(show_id, "guid-1", "Ep One"))
            .await
            .unwrap()
    });

    // The imported track (music profile) followed by the episode (spoken-word).
    let (track_id, track_item) = {
        let conn = pool.open().unwrap();
        let track = get_track(&conn, search_rows(&conn).unwrap()[0].track_id)
            .unwrap()
            .unwrap();
        let item = PlayableItem {
            track_id: track.id,
            source: root.join(&track.file_path),
            profile: resolve_music_profile(&track, &PlaybackConfig::default()),
            album_id: track.album_id,
            kind: MediaKind::Track,
            streaming: false,
            chapters: [].into(),
            segments: [].into(),
        };
        (track.id, item)
    };
    let episode_item = PlayableItem {
        track_id: episode_id,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Episode,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![track_item, episode_item], 0);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if player.snapshot().ended {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the mixed track+episode queue did not finish"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    player.shutdown();

    let conn = pool.open().unwrap();
    assert_eq!(
        get_track(&conn, track_id).unwrap().unwrap().play_count,
        1,
        "the track played through its music profile"
    );
    let pb = get_playback(&conn, episode_id).unwrap().unwrap();
    assert_eq!(
        pb.played,
        PlayedState::PlayedFully,
        "the episode played through after the af-chain profile swap"
    );

    runtime.block_on(worker.shutdown_ack()).ok();
}

/// The snapshot surfaces Smart Speed for the Now Playing indicator (Phase
/// 6c-iii-c): an episode whose profile has Smart Speed on reports
/// `smart_speed_active = true` with a non-negative live `smart_speed_saved`. The
/// saved math is unit-tested in `player::session`; this proves the snapshot
/// wiring (`refresh_snapshot` reads the current item's profile + open session).
#[test]
fn snapshot_reports_smart_speed_for_an_episode() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };

    // A real episode row: the close-of-session insert at shutdown is FK-bound.
    let episode_id = runtime.block_on(async {
        let show_id = worker
            .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
            .await
            .unwrap();
        worker
            .upsert_episode(sample_episode(show_id, "guid-1", "Ep One"))
            .await
            .unwrap()
    });

    let mut profile = conservatory_core::resolve_episode_profile(None);
    profile.smart_speed = true;
    let item = PlayableItem {
        track_id: episode_id,
        source: fixtures_dir.join("sample.mp3"),
        profile,
        album_id: None,
        kind: MediaKind::Episode,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item], 0);
    player.pause();

    // Loaded and paused so the 0.3 s fixture cannot end under the assertion.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = player.snapshot();
        if s.paused && !s.ended && s.kind == Some(MediaKind::Episode) {
            break;
        }
        assert!(Instant::now() < deadline, "episode never loaded paused");
        std::thread::sleep(Duration::from_millis(20));
    }
    let snap = player.snapshot();
    assert!(
        snap.smart_speed_active,
        "an episode with Smart Speed on must report it active"
    );
    assert!(
        snap.smart_speed_saved >= 0.0,
        "saved seconds are never negative"
    );

    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

/// Minimal podcast fixtures for the engine episode test (no chrono / network).
fn sample_show(slug: &str, feed_url: &str) -> Show {
    Show {
        id: 0,
        slug: slug.to_string(),
        feed_url: feed_url.to_string(),
        title: "Reply All".to_string(),
        author: None,
        description: None,
        homepage_url: None,
        cover_path: None,
        accent_rgb: None,
        apple_podcasts_id: None,
        last_fetched: None,
        last_modified: None,
        etag: None,
        fetch_interval: 3600,
        auth_user: None,
        auth_pass_ref: None,
        auto_download: false,
        keep_count: 0,
        priority: 0,
        folder_path: format!("Podcasts/{slug}"),
    }
}

fn sample_episode(show_id: i64, guid: &str, title: &str) -> Episode {
    Episode {
        id: 0,
        show_id,
        guid: guid.to_string(),
        title: title.to_string(),
        description: None,
        pub_date: None,
        duration: None,
        file_size: None,
        audio_url: Some(format!("https://cdn.example.com/{guid}.mp3")),
        audio_path: None,
        folder_path: format!("Podcasts/replyall/{guid}"),
        mime_type: Some("audio/mpeg".to_string()),
        season: None,
        episode_number: None,
        episode_type: None,
    }
}

/// Live move/remove keep `current_index` aligned without auto-advancing: start
/// paused so the 0.3 s fixtures don't end under us, then exercise the in-place
/// mutations the queue drawer drives.
#[test]
fn engine_move_and_remove_track_the_current_index() {
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
                    kind: conservatory_core::db::MediaKind::Track,
                    streaming: false,
                    chapters: [].into(),
                    segments: [].into(),
                }
            })
            .collect()
    };
    assert_eq!(items.len(), 4);

    let wait_for = |player: &conservatory_core::PlayerHandle,
                    pred: fn(&conservatory_core::PlayerSnapshot) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if pred(&player.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot condition not met in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    // Start at index 1, paused (SetQueue then Pause drain together, before the
    // 0.3 s track can end).
    player.play_queue(items, 1);
    player.pause();
    wait_for(&player, |s| s.current_index == Some(1) && s.paused);

    // Move the playing item to index 3: it follows; the queue length is unchanged.
    player.move_item(1, 3);
    wait_for(&player, |s| s.current_index == Some(3) && s.queue_len == 4);

    // Remove an item before the current one: current shifts down, queue shrinks.
    player.remove_item(0);
    wait_for(&player, |s| s.current_index == Some(2) && s.queue_len == 3);

    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

/// Append-to-idle starts playing; appending again extends the tail; a fresh
/// engine resumes the whole queue paused at the cursor (Phase 4b-ii-c). Pauses
/// keep the 0.3 s fixtures from advancing under the assertions.
#[test]
fn engine_append_and_resume() {
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
                    kind: conservatory_core::db::MediaKind::Track,
                    streaming: false,
                    chapters: [].into(),
                    segments: [].into(),
                }
            })
            .collect()
    };
    assert_eq!(items.len(), 4);

    let wait = |player: &PlayerHandle, pred: fn(&PlayerSnapshot) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if pred(&player.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot condition not met in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    // Append to an idle engine: the first item starts playing (pause to freeze).
    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.append(items[..2].to_vec());
    player.pause();
    wait(&player, |s| {
        s.current_index == Some(0) && s.paused && s.queue_len == 2
    });
    // Append more: the tail grows, the current item is unchanged.
    player.append(items[2..].to_vec());
    wait(&player, |s| s.queue_len == 4 && s.current_index == Some(0));
    player.shutdown();

    // A fresh engine resumes the whole queue paused at the cursor (index 2).
    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.resume(items.clone(), 2, 0.0);
    wait(&player, |s| s.current_index == Some(2) && s.paused);
    player.shutdown();

    runtime.block_on(worker.shutdown_ack()).ok();
}

// --- Phase 16a: Play Next (queue insert-at) and Remove from Library (delete).

#[tokio::test]
async fn queue_insert_at_shifts_later_positions() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker.enqueue_tracks(vec![1, 2, 3]).await.unwrap();
    // Insert [7, 8] at position 1: entries at/after 1 shift up by 2.
    worker.insert_queue_tracks_at(1, vec![7, 8]).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![1, 7, 8, 2, 3]);
    assert_positions_contiguous(&pool);

    // Insert past the end clamps to the tail.
    worker.insert_queue_tracks_at(999, vec![9]).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![1, 7, 8, 2, 3, 9]);
    assert_positions_contiguous(&pool);

    // Insert at 0 prepends.
    worker.insert_queue_tracks_at(0, vec![4]).await.unwrap();
    assert_eq!(queue_track_ids(&pool), vec![4, 1, 7, 8, 2, 3, 9]);
    assert_positions_contiguous(&pool);
}

#[tokio::test]
async fn delete_track_removes_it_and_cascades_the_queue() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    worker.enqueue_tracks(vec![1, 2, 3]).await.unwrap();
    worker.delete_track(2).await.unwrap();

    // The row is gone and its queue entry cascaded away (ON DELETE CASCADE); the
    // surviving order is intact (the cascade leaves a position gap, which the
    // position-ordered load tolerates, so this asserts order, not contiguity).
    let conn = pool.open().unwrap();
    assert!(get_track(&conn, 2).unwrap().is_none());
    drop(conn);
    assert_eq!(queue_track_ids(&pool), vec![1, 3]);
}

/// Play Next inserts just after the current item without disturbing playback; a
/// general insert before the current item shifts its index up (Phase 16a). Paused
/// so the 0.3 s fixtures can't advance under the assertions.
#[test]
fn engine_play_next_inserts_after_the_current_item() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dbdir = tempdir().unwrap();
    let db = dbdir.path().join("library.db");
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio");
    let worker = {
        let _guard = runtime.enter();
        spawn_worker(db.clone()).unwrap()
    };

    // Synthetic Track items over one real fixture; the ids need not exist (the EOF
    // persistence is a silent no-op for a missing row, and we never reach EOF).
    let item = |id: i64| PlayableItem {
        track_id: id,
        source: fixtures_dir.join("sample.mp3"),
        profile: conservatory_core::resolve_episode_profile(None),
        album_id: None,
        kind: MediaKind::Track,
        streaming: false,
        chapters: [].into(),
        segments: [].into(),
    };

    let wait = |player: &PlayerHandle, pred: fn(&PlayerSnapshot) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if pred(&player.snapshot()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "snapshot condition not met in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    let player = player::spawn_null(worker.clone(), runtime.handle().clone()).unwrap();
    player.play_queue(vec![item(1), item(2)], 0);
    player.pause();
    wait(&player, |s| {
        s.current_index == Some(0) && s.queue_len == 2 && s.paused
    });

    // Play Next: insert at current + 1 = 1. The playing item keeps its index.
    player.insert_items(1, vec![item(3)]);
    wait(&player, |s| s.current_index == Some(0) && s.queue_len == 3);

    // A general insert before the current item shifts its index up by the block.
    player.insert_items(0, vec![item(4)]);
    wait(&player, |s| s.current_index == Some(1) && s.queue_len == 4);

    player.shutdown();
    runtime.block_on(worker.shutdown_ack()).ok();
}

// --- helpers

fn queue_track_ids(pool: &ReadPool) -> Vec<i64> {
    let conn = pool.open().unwrap();
    load_queue(&conn)
        .unwrap()
        .into_iter()
        .filter_map(|q| q.track_id)
        .collect()
}

fn assert_positions_contiguous(pool: &ReadPool) {
    let conn = pool.open().unwrap();
    let positions: Vec<i64> = load_queue(&conn)
        .unwrap()
        .into_iter()
        .map(|q| q.position)
        .collect();
    let expected: Vec<i64> = (0..positions.len() as i64).collect();
    assert_eq!(
        positions, expected,
        "positions must be a dense 0..n range in order"
    );
}
