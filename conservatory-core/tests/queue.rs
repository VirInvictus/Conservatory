//! Phase 4b-i integration tests: the unified queue's position integrity, the
//! `is:queued` membership wiring, and the threaded player engine advancing a
//! queue end-to-end through a null audio output.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    Episode, MediaKind, PlayedState, ReadPool, Show, get_playback, get_track, load_queue,
    read_playback_state, search_rows, spawn_worker,
};
use conservatory_core::player;
use conservatory_core::{
    ImportOptions, MoveMode, PlayableItem, PlaybackConfig, PlayerHandle, PlayerSnapshot,
    import_folder, resolve_music_profile,
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
        profile: conservatory_core::resolve_episode_profile(),
        album_id: None,
        kind: MediaKind::Episode,
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
