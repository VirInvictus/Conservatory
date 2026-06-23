//! Phase 6b-ii-c-3-b: retention pruning of downloaded episodes beyond a show's
//! `keep_count`, against a real core worker (temp DB) and a temp library root.
//! Hermetic, no network. Covers: a `keep_count > 0` show prunes its oldest
//! downloads (deletes the files, clears `audio_path`) and keeps the newest;
//! `keep_count == 0` keeps everything; never-downloaded episodes are untouched.

use std::path::Path;

use chrono::{TimeZone, Utc};
use conservatory_core::db::{
    Episode, ReadPool, Show, WorkerHandle, get_episode, list_episodes_for_show, spawn_worker,
};
use conservatory_podcasts::retention;
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let db = dir.path().join("t.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    (dir, worker, pool)
}

async fn make_show(worker: &WorkerHandle, keep_count: u32) -> i64 {
    worker
        .get_or_create_show(Show {
            id: 0,
            slug: "cast".into(),
            feed_url: "https://example.test/feed.xml".into(),
            title: "Cast".into(),
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
            keep_count,
            priority: 0,
            folder_path: "Podcasts/cast".into(),
        })
        .await
        .unwrap()
}

/// Insert episode `n` (day `n` of 2024-01, so a higher `n` is newer) and, when
/// `download` is set, create its file under `root` and record `audio_path`.
async fn make_episode(
    worker: &WorkerHandle,
    root: &Path,
    show_id: i64,
    n: u32,
    download: bool,
) -> i64 {
    let folder = format!("Podcasts/cast/ep-{n}");
    let id = worker
        .upsert_episode(Episode {
            id: 0,
            show_id,
            guid: format!("ep-{n}"),
            title: format!("Episode {n}"),
            description: None,
            pub_date: Some(Utc.with_ymd_and_hms(2024, 1, n, 0, 0, 0).unwrap()),
            duration: None,
            file_size: None,
            audio_url: Some(format!("https://example.test/ep-{n}.mp3")),
            audio_path: None,
            folder_path: folder.clone(),
            mime_type: Some("audio/mpeg".into()),
            season: None,
            episode_number: Some(n),
            episode_type: None,
        })
        .await
        .unwrap();
    if download {
        let rel = format!("{folder}/ep-{n}.mp3");
        let dst = root.join(&rel);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&dst, b"audio").unwrap();
        worker.set_episode_audio_path(id, rel).await.unwrap();
    }
    id
}

#[tokio::test]
async fn prune_keeps_newest_and_deletes_oldest_downloads() {
    let (dir, worker, pool) = fresh();
    let root = dir.path();
    let show_id = make_show(&worker, 2).await;

    // Four downloaded episodes (ep-1 oldest .. ep-4 newest).
    let mut ids = Vec::new();
    for n in 1..=4 {
        ids.push(make_episode(&worker, root, show_id, n, true).await);
    }
    let (ep1, ep2, ep3, ep4) = (ids[0], ids[1], ids[2], ids[3]);

    // Plan: keep the 2 newest (ep-4, ep-3); prune the 2 oldest (ep-2, ep-1).
    let plan = retention::plan(&pool, Some(show_id)).unwrap();
    let mut planned: Vec<i64> = plan.iter().map(|p| p.episode_id).collect();
    planned.sort_unstable();
    assert_eq!(planned, {
        let mut v = vec![ep1, ep2];
        v.sort_unstable();
        v
    });

    let pruned = retention::apply(&worker, root, &plan).await.unwrap();
    assert_eq!(pruned, 2);

    // The oldest two lost their files + audio_path; the newest two are intact.
    let conn = pool.open().unwrap();
    for (id, n, kept) in [
        (ep1, 1, false),
        (ep2, 2, false),
        (ep3, 3, true),
        (ep4, 4, true),
    ] {
        let ep = get_episode(&conn, id).unwrap().unwrap();
        let file = root.join(format!("Podcasts/cast/ep-{n}/ep-{n}.mp3"));
        assert_eq!(ep.audio_path.is_some(), kept, "ep-{n} audio_path");
        assert_eq!(file.exists(), kept, "ep-{n} file on disk");
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn keep_count_zero_prunes_nothing() {
    let (dir, worker, pool) = fresh();
    let root = dir.path();
    let show_id = make_show(&worker, 0).await;
    for n in 1..=3 {
        make_episode(&worker, root, show_id, n, true).await;
    }

    let plan = retention::plan(&pool, Some(show_id)).unwrap();
    assert!(plan.is_empty(), "keep_count = 0 keeps everything");

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn undownloaded_episodes_are_never_pruned() {
    let (dir, worker, pool) = fresh();
    let root = dir.path();
    let show_id = make_show(&worker, 1).await;

    // Two downloaded + two stream-only. keep_count = 1 leaves 1 download to
    // prune; the stream-only episodes never count toward the cap.
    make_episode(&worker, root, show_id, 1, true).await;
    make_episode(&worker, root, show_id, 2, true).await;
    make_episode(&worker, root, show_id, 3, false).await;
    make_episode(&worker, root, show_id, 4, false).await;

    let plan = retention::plan(&pool, Some(show_id)).unwrap();
    assert_eq!(plan.len(), 1, "only one excess download is pruned");
    // The pruned one is the older download (ep-1), not any stream-only episode.
    assert_eq!(plan[0].episode_title, "Episode 1");
    assert!(plan[0].audio_path.contains("ep-1"));

    retention::apply(&worker, root, &plan).await.unwrap();
    let conn = pool.open().unwrap();
    let downloaded = list_episodes_for_show(&conn, show_id)
        .unwrap()
        .into_iter()
        .filter(|e| e.audio_path.is_some())
        .count();
    assert_eq!(downloaded, 1, "ep-2 (newest download) survives");

    worker.shutdown_ack().await.unwrap();
}
