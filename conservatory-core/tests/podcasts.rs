//! Phase 6a-i integration tests: the podcast schema (migration 0006) and the
//! core worker CRUD that backs the absorbed Belfry subsystem (spec §4.2, §8).
//! No network here; that is Phase 6a-ii. These exercise the worker write path
//! and the read pool round-trip, plus the structural change to the unified
//! queue (the deferred `episode_id` foreign key).

use chrono::{DateTime, TimeZone, Utc};
use conservatory_core::db::{Chapter, MediaKind, PlaybackCursor, WorkerHandle};
use conservatory_core::db::{
    Episode, InboxPolicy, Playback, PlayedState, ReadPool, Show, ShowSettings, get_episode_by_guid,
    get_playback, get_show, get_show_settings, list_chapters, list_episodes_for_show, list_shows,
    list_tags_for_show, read_playback_state, spawn_worker,
};
use tempfile::tempdir;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().unwrap()
}

fn sample_show(slug: &str, feed_url: &str) -> Show {
    Show {
        id: 0,
        slug: slug.to_string(),
        feed_url: feed_url.to_string(),
        title: "Reply All".to_string(),
        author: Some("Gimlet".to_string()),
        description: Some("A podcast about the internet".to_string()),
        homepage_url: Some("https://gimletmedia.com/reply-all".to_string()),
        cover_path: None,
        accent_rgb: None,
        apple_podcasts_id: Some("941907967".to_string()),
        last_fetched: None,
        last_modified: None,
        etag: None,
        fetch_interval: 3600,
        auth_user: None,
        auth_pass_ref: None,
        auto_download: true,
        keep_count: 0,
        priority: 0,
        folder_path: format!("Podcasts/{slug}"),
    }
}

fn sample_episode(show_id: i64, guid: &str, title: &str, pub_date: i64) -> Episode {
    Episode {
        id: 0,
        show_id,
        guid: guid.to_string(),
        title: title.to_string(),
        description: Some("Show notes go here".to_string()),
        pub_date: Some(ts(pub_date)),
        duration: Some(1800),
        file_size: Some(28_800_000),
        audio_url: Some(format!("https://cdn.example.com/{guid}.mp3")),
        audio_path: None,
        folder_path: format!("Podcasts/replyall/{guid}"),
        mime_type: Some("audio/mpeg".to_string()),
        season: Some(1),
        episode_number: Some(1),
        episode_type: Some("full".to_string()),
    }
}

async fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    (dir, worker, pool)
}

#[tokio::test]
async fn episode_metadata_resolves_show_title_and_cover() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    let episode_id = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "The Web", 1_000))
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let np = conservatory_core::db::episode_metadata(&conn, episode_id)
        .unwrap()
        .expect("episode metadata present");
    // The episode title is the title; the show stands in for the artist (so the
    // Now-bar shows the episode + show, not a stale music track). v0.0.38.
    assert_eq!(np.title, "The Web");
    assert_eq!(np.artist.as_deref(), Some("Reply All"));
    assert_eq!(np.length, Some(1800.0));
    // sample_show has no cover, so the Now-bar falls back to its placeholder
    // rather than the previous track's cover.
    assert_eq!(np.album_cover_path, None);
    // A missing episode reads as None, not an error.
    assert!(
        conservatory_core::db::episode_metadata(&conn, 999_999)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn show_round_trip_and_idempotent_add() {
    let (_dir, worker, pool) = fresh().await;

    let id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();

    // Adding the same feed again returns the same row, not a duplicate.
    let again = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    assert_eq!(id, again);

    let conn = pool.open().unwrap();
    assert_eq!(list_shows(&conn).unwrap().len(), 1);
    let show = get_show(&conn, id).unwrap().unwrap();
    assert_eq!(show.title, "Reply All");
    assert_eq!(show.apple_podcasts_id.as_deref(), Some("941907967"));
    assert_eq!(show.fetch_interval, 3600);
    assert!(show.auto_download);
}

#[tokio::test]
async fn update_show_persists_conditional_get_state() {
    let (_dir, worker, pool) = fresh().await;
    let id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();

    // The fetch loop's after-poll write: stamp etag / last_modified / last_fetched.
    let mut show = {
        let conn = pool.open().unwrap();
        get_show(&conn, id).unwrap().unwrap()
    };
    show.etag = Some("\"abc123\"".to_string());
    show.last_modified = Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string());
    show.last_fetched = Some(ts(1_700_000_000));
    worker.update_show(show).await.unwrap();

    let conn = pool.open().unwrap();
    let reread = get_show(&conn, id).unwrap().unwrap();
    assert_eq!(reread.etag.as_deref(), Some("\"abc123\""));
    assert_eq!(
        reread.last_modified.as_deref(),
        Some("Wed, 21 Oct 2026 07:28:00 GMT")
    );
    assert_eq!(reread.last_fetched, Some(ts(1_700_000_000)));
}

#[tokio::test]
async fn episode_upsert_dedups_by_guid_and_orders_newest_first() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();

    let older = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "The Original", 1_000))
        .await
        .unwrap();
    let newer = worker
        .upsert_episode(sample_episode(show_id, "guid-2", "Later Episode", 2_000))
        .await
        .unwrap();
    assert_ne!(older, newer);

    // Re-fetch of guid-1 with a changed title updates the same row.
    let same = worker
        .upsert_episode(sample_episode(
            show_id,
            "guid-1",
            "The Original (rerun)",
            1_000,
        ))
        .await
        .unwrap();
    assert_eq!(older, same);

    let conn = pool.open().unwrap();
    let episodes = list_episodes_for_show(&conn, show_id).unwrap();
    assert_eq!(episodes.len(), 2, "upsert must not duplicate by guid");
    // Newest pub_date first.
    assert_eq!(episodes[0].guid, "guid-2");
    assert_eq!(episodes[1].guid, "guid-1");
    assert_eq!(episodes[1].title, "The Original (rerun)");

    let by_guid = get_episode_by_guid(&conn, show_id, "guid-2")
        .unwrap()
        .unwrap();
    assert_eq!(by_guid.title, "Later Episode");
    assert!(
        get_episode_by_guid(&conn, show_id, "nope")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn upsert_episode_keeps_a_downloaded_path_across_refetch() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();

    let mut ep = sample_episode(show_id, "guid-1", "Episode", 1_000);
    ep.audio_path = Some("Podcasts/replyall/guid-1/episode.mp3".to_string());
    worker.upsert_episode(ep).await.unwrap();

    // A later feed refresh carries no local path; it must not erase the download.
    worker
        .upsert_episode(sample_episode(
            show_id,
            "guid-1",
            "Episode (updated)",
            1_000,
        ))
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let ep = get_episode_by_guid(&conn, show_id, "guid-1")
        .unwrap()
        .unwrap();
    assert_eq!(
        ep.audio_path.as_deref(),
        Some("Podcasts/replyall/guid-1/episode.mp3")
    );
    assert_eq!(ep.title, "Episode (updated)");
}

#[tokio::test]
async fn fts_tracks_show_and_episode_through_edits() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    let ep_id = worker
        .upsert_episode(sample_episode(
            show_id,
            "guid-1",
            "The Snapchat Thief",
            1_000,
        ))
        .await
        .unwrap();

    let fts_count = |table: &str, q: &str| -> i64 {
        let conn = pool.open().unwrap();
        let sql = format!("SELECT count(*) FROM {table} WHERE {table} MATCH ?1");
        conn.query_row(&sql, [q], |r| r.get(0)).unwrap()
    };

    assert_eq!(fts_count("show_fts", "Gimlet"), 1);
    assert_eq!(fts_count("episode_fts", "Snapchat"), 1);

    // An episode title edit re-syncs the FTS index.
    let mut ep = sample_episode(show_id, "guid-1", "The Snapchat Thief", 1_000);
    ep.id = ep_id;
    ep.title = "The Instagram Thief".to_string();
    worker.upsert_episode(ep).await.unwrap();
    assert_eq!(fts_count("episode_fts", "Snapchat"), 0);
    assert_eq!(fts_count("episode_fts", "Instagram"), 1);

    // Deleting the show cascades the episode out of both FTS indexes.
    worker.delete_show(show_id).await.unwrap();
    assert_eq!(fts_count("episode_fts", "Instagram"), 0);
    assert_eq!(fts_count("show_fts", "Gimlet"), 0);
}

#[tokio::test]
async fn playback_and_show_settings_upsert() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    let ep_id = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "Episode", 1_000))
        .await
        .unwrap();

    // No row yet: an untouched episode is Inbox by default.
    {
        let conn = pool.open().unwrap();
        assert!(get_playback(&conn, ep_id).unwrap().is_none());
    }

    worker
        .upsert_playback(Playback {
            episode_id: ep_id,
            position: 123.5,
            played: PlayedState::InProgress,
            last_played: Some(ts(5_000)),
            play_count: 0,
            starred: true,
        })
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep_id).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::InProgress);
        assert!((pb.position - 123.5).abs() < 1e-9);
        assert!(pb.starred);
    }

    // A second upsert overwrites (marks fully played).
    worker
        .upsert_playback(Playback {
            episode_id: ep_id,
            position: 1800.0,
            played: PlayedState::PlayedFully,
            last_played: Some(ts(6_000)),
            play_count: 1,
            starred: false,
        })
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep_id).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::PlayedFully);
        assert_eq!(pb.play_count, 1);
        assert!(!pb.starred);
    }

    // Per-show overrides round-trip, including the inbox policy enum.
    worker
        .upsert_show_settings(ShowSettings {
            show_id,
            playback_speed: 1.5,
            smart_speed: true,
            voice_boost: true,
            skip_intro: 30,
            skip_outro: 0,
            skip_forward: Some(45),
            skip_back: None,
            inbox_policy: InboxPolicy::AlwaysQueue,
        })
        .await
        .unwrap();
    let conn = pool.open().unwrap();
    let settings = get_show_settings(&conn, show_id).unwrap().unwrap();
    assert!((settings.playback_speed - 1.5).abs() < 1e-9);
    assert!(settings.voice_boost);
    assert_eq!(settings.skip_intro, 30);
    assert_eq!(settings.skip_forward, Some(45));
    assert_eq!(settings.skip_back, None);
    assert_eq!(settings.inbox_policy, InboxPolicy::AlwaysQueue);
}

#[tokio::test]
async fn chapters_replace_is_clean() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    let ep_id = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "Episode", 1_000))
        .await
        .unwrap();

    let chapter = |start: f64, title: &str| Chapter {
        id: 0,
        episode_id: ep_id,
        start_time: start,
        end_time: None,
        title: Some(title.to_string()),
        url: None,
        image_path: None,
    };

    worker
        .replace_chapters(ep_id, vec![chapter(0.0, "Intro"), chapter(120.0, "Main")])
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let chapters = list_chapters(&conn, ep_id).unwrap();
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title.as_deref(), Some("Intro"));
        assert_eq!(chapters[1].title.as_deref(), Some("Main"));
    }

    // Replace, not append.
    worker
        .replace_chapters(ep_id, vec![chapter(0.0, "Only one now")])
        .await
        .unwrap();
    let conn = pool.open().unwrap();
    let chapters = list_chapters(&conn, ep_id).unwrap();
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title.as_deref(), Some("Only one now"));
}

#[tokio::test]
async fn show_tags_round_trip_and_replace() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();

    worker
        .set_show_tags(show_id, vec!["Tech".to_string(), "Stories".to_string()])
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let tags: Vec<String> = list_tags_for_show(&conn, show_id)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(tags, vec!["Stories".to_string(), "Tech".to_string()]);
    }

    // get_or_create_tag is idempotent: re-using "Tech" does not duplicate.
    let tech = worker.get_or_create_tag("Tech").await.unwrap();
    let tech_again = worker.get_or_create_tag("Tech").await.unwrap();
    assert_eq!(tech, tech_again);

    // Replacing the set drops the old links.
    worker
        .set_show_tags(show_id, vec!["Interview".to_string()])
        .await
        .unwrap();
    let conn = pool.open().unwrap();
    let tags: Vec<String> = list_tags_for_show(&conn, show_id)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(tags, vec!["Interview".to_string()]);
}

#[tokio::test]
async fn queue_gained_the_episode_foreign_key() {
    // Migration 0006 rebuilt `queue` to add the deferred `episode_id` FK now that
    // `episodes` exists (the 4b-i note). The episode-enqueue path and its cascade
    // land with triage at Phase 6b; here we verify the structural change directly.
    let (_dir, _worker, pool) = fresh().await;
    let conn = pool.open().unwrap();

    let mut stmt = conn.prepare("PRAGMA foreign_key_list(queue)").unwrap();
    let referenced: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>("table"))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        referenced.iter().any(|t| t == "episodes"),
        "queue should reference episodes after 0006: {referenced:?}"
    );
    assert!(
        referenced.iter().any(|t| t == "tracks"),
        "queue should still reference tracks: {referenced:?}"
    );
    // `books` does not exist until Phase 7, so book_id stays plain (no FK yet).
    assert!(
        !referenced.iter().any(|t| t == "books"),
        "book_id must not have an FK until Phase 7: {referenced:?}"
    );
}

#[tokio::test]
async fn triage_buckets_partition_episodes() {
    use conservatory_core::db::{TriageBucket, episodes_for_show, episodes_in_bucket};

    let (dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show("cast", "https://feeds.example/cast.xml"))
        .await
        .unwrap();

    // Four episodes, one per triage state.
    let played = worker
        .upsert_episode(sample_episode(show_id, "ep-played", "Played", 4000))
        .await
        .unwrap();
    let queued = worker
        .upsert_episode(sample_episode(show_id, "ep-queued", "Queued", 3000))
        .await
        .unwrap();
    let in_prog = worker
        .upsert_episode(sample_episode(show_id, "ep-prog", "In progress", 2000))
        .await
        .unwrap();
    let untouched = worker
        .upsert_episode(sample_episode(show_id, "ep-fresh", "Fresh", 1000))
        .await
        .unwrap();

    worker
        .upsert_playback(Playback {
            episode_id: played,
            position: 1800.0,
            played: PlayedState::PlayedFully,
            last_played: Some(ts(5_000)),
            play_count: 1,
            starred: false,
        })
        .await
        .unwrap();
    worker
        .upsert_playback(Playback {
            episode_id: in_prog,
            position: 60.0,
            played: PlayedState::InProgress,
            last_played: Some(ts(5_000)),
            play_count: 0,
            starred: true,
        })
        .await
        .unwrap();

    // Put `queued` in the unified queue. The episode-enqueue command lands at
    // 6b-ii-b; here we insert the row directly to exercise the Queue derivation.
    {
        let conn = rusqlite::Connection::open(dir.path().join("t.db")).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "INSERT INTO queue (position, kind, episode_id) VALUES (0, 'episode', ?1)",
            [queued],
        )
        .unwrap();
    }

    let conn = pool.open().unwrap();
    let bucket_ids = |b| {
        episodes_in_bucket(&conn, b)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(bucket_ids(TriageBucket::Played), vec![played]);
    assert_eq!(bucket_ids(TriageBucket::Queue), vec![queued]);
    let mut inbox = bucket_ids(TriageBucket::Inbox);
    inbox.sort_unstable();
    let mut expect = vec![in_prog, untouched];
    expect.sort_unstable();
    assert_eq!(inbox, expect, "in-progress + untouched are Inbox");

    // episodes_for_show carries the joined triage state for every episode.
    let rows = episodes_for_show(&conn, show_id).unwrap();
    assert_eq!(rows.len(), 4);
    let by_id = |id| rows.iter().find(|r| r.id == id).unwrap().clone();
    assert_eq!(by_id(played).played, PlayedState::PlayedFully);
    assert!(by_id(queued).in_queue);
    assert!(!by_id(played).in_queue);
    assert!(by_id(in_prog).starred);
    assert_eq!(by_id(untouched).played, PlayedState::Unplayed);
    assert_eq!(by_id(played).show_title, "Reply All");

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn triage_actions_are_partial_and_tag_filter_works() {
    use conservatory_core::db::{
        TriageBucket, episodes_for_tag, episodes_in_bucket, list_all_tags,
    };

    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show("cast", "https://feeds.example/cast.xml"))
        .await
        .unwrap();
    let ep = worker
        .upsert_episode(sample_episode(show_id, "g1", "Ep", 1000))
        .await
        .unwrap();

    // A resume position, to prove mark-unplayed rewinds it.
    worker
        .upsert_playback(Playback {
            episode_id: ep,
            position: 500.0,
            played: PlayedState::InProgress,
            last_played: None,
            play_count: 0,
            starred: false,
        })
        .await
        .unwrap();

    // Star: played/position untouched.
    worker.set_episode_starred(ep, true).await.unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep).unwrap().unwrap();
        assert!(pb.starred);
        assert_eq!(pb.played, PlayedState::InProgress);
        assert!((pb.position - 500.0).abs() < 1e-9);
    }

    // Mark played: starred preserved, last_played set, moves to the Played bucket.
    worker
        .set_episode_played(ep, PlayedState::PlayedFully, Some(9_999))
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::PlayedFully);
        assert!(pb.starred, "mark-played must not clobber starred");
        assert_eq!(pb.last_played, Some(ts(9_999)));
        let played: Vec<_> = episodes_in_bucket(&conn, TriageBucket::Played)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(played, vec![ep]);
    }

    // Mark unplayed: rewinds position, keeps starred, returns to Inbox.
    worker
        .set_episode_played(ep, PlayedState::Unplayed, None)
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::Unplayed);
        assert!(pb.position.abs() < 1e-9, "mark-unplayed rewinds position");
        assert!(pb.starred, "still starred");
        let inbox: Vec<_> = episodes_in_bucket(&conn, TriageBucket::Inbox)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(inbox, vec![ep]);
    }

    // Tag filter.
    let tag_id = worker.get_or_create_tag("news").await.unwrap();
    worker
        .set_show_tags(show_id, vec!["news".to_string()])
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        assert!(
            list_all_tags(&conn)
                .unwrap()
                .iter()
                .any(|t| t.name == "news")
        );
        let tagged: Vec<_> = episodes_for_tag(&conn, tag_id)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(tagged, vec![ep]);
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn episode_enqueue_shows_in_the_queue_display() {
    use conservatory_core::db::{MediaKind, load_queue_display};

    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show("cast", "https://feeds.example/cast.xml"))
        .await
        .unwrap();
    let ep = worker
        .upsert_episode(sample_episode(show_id, "g1", "Ep One", 1000))
        .await
        .unwrap();

    worker.replace_queue_with_episodes(vec![ep]).await.unwrap();

    let conn = pool.open().unwrap();
    let rows = load_queue_display(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, MediaKind::Episode);
    assert_eq!(rows[0].episode_id, Some(ep));
    assert_eq!(rows[0].track_id, None);
    // The drawer renders an episode's title + its show as the "artist".
    assert_eq!(rows[0].title, "Ep One");
    assert_eq!(rows[0].artist.as_deref(), Some("Reply All"));

    worker.shutdown_ack().await.unwrap();
}

/// Phase 6b-ii-c-2: an episode's resume position + played state persist through
/// the worker (its per-episode `playback` row), the partial writes preserve
/// their siblings, and the transport cursor records `kind = Episode` so a
/// restart reopens the episode rather than a track.
#[tokio::test]
async fn episode_playback_persists_position_completion_and_cursor() {
    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show("replyall", "https://example.com/feed.xml"))
        .await
        .unwrap();
    let ep = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "Ep One", 1_000))
        .await
        .unwrap();

    // Star it first: the position write must preserve the star (partial upsert).
    worker.set_episode_starred(ep, true).await.unwrap();

    // A playback tick marks InProgress and records the resume position.
    worker
        .set_episode_position(ep, 123.0, Some(50))
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::InProgress);
        assert_eq!(pb.position, 123.0);
        assert_eq!(pb.play_count, 0);
        assert!(pb.starred, "the position write preserves starred");
    }

    // Playing to the end marks PlayedFully, bumps play_count, rewinds position.
    worker.complete_episode(ep, Some(99)).await.unwrap();
    {
        let conn = pool.open().unwrap();
        let pb = get_playback(&conn, ep).unwrap().unwrap();
        assert_eq!(pb.played, PlayedState::PlayedFully);
        assert_eq!(pb.position, 0.0);
        assert_eq!(pb.play_count, 1);
        assert!(pb.starred, "completion preserves starred");
    }

    // The transport cursor records kind = Episode + the episode id, never a
    // track id, so launch-resume reopens the episode (6b-ii-c-2).
    worker
        .save_playback_state(PlaybackCursor {
            kind: MediaKind::Episode,
            track_id: None,
            episode_id: Some(ep),
            position: 123.0,
            paused: true,
            volume: 90,
            updated_at: 1_500,
        })
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let cur = read_playback_state(&conn).unwrap().unwrap();
        assert_eq!(cur.kind, MediaKind::Episode);
        assert_eq!(cur.episode_id, Some(ep));
        assert_eq!(cur.track_id, None);
        assert_eq!(cur.position, 123.0);
    }

    worker.shutdown_ack().await.unwrap();
}

/// `listening_sessions` is append-only and `listening_totals` sums it (Phase
/// 6c-ii): an empty table reads zero, and three appended sessions sum the counts
/// and the real / audio / saved seconds.
#[tokio::test]
async fn listening_sessions_append_and_total() {
    use conservatory_core::db::listening_totals;

    let (_dir, worker, pool) = fresh().await;
    let show_id = worker
        .get_or_create_show(sample_show(
            "replyall",
            "https://feeds.example.com/replyall",
        ))
        .await
        .unwrap();
    let ep = worker
        .upsert_episode(sample_episode(show_id, "guid-1", "The Web", 1_000))
        .await
        .unwrap();

    // Empty ledger sums to zero (the COALESCE), not an error.
    {
        let conn = pool.open().unwrap();
        let t = listening_totals(&conn).unwrap();
        assert_eq!(t.sessions, 0);
        assert_eq!(t.real_seconds, 0.0);
        assert_eq!(t.audio_seconds, 0.0);
        assert_eq!(t.smart_speed_saved, 0.0);
    }

    // Three sessions: 60+120+30 real, 90+180+30 audio, 30+60+0 saved.
    for (start, end, real, audio, saved) in [
        (1_000, 1_060, 60.0, 90.0, 30.0),
        (2_000, 2_120, 120.0, 180.0, 60.0),
        (3_000, 3_030, 30.0, 30.0, 0.0),
    ] {
        worker
            .insert_listening_session(ep, start, end, real, audio, saved)
            .await
            .unwrap();
    }

    let conn = pool.open().unwrap();
    let t = listening_totals(&conn).unwrap();
    assert_eq!(t.sessions, 3);
    assert_eq!(t.real_seconds, 210.0);
    assert_eq!(t.audio_seconds, 300.0);
    assert_eq!(t.smart_speed_saved, 90.0);
}
