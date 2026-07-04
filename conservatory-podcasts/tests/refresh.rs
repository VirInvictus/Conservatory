//! Phase 6a-ii-b: the refresh pipeline end to end, against a local wiremock
//! server and a real core single-writer worker (temp DB). Hermetic, no real
//! network. Covers: `add` lands episodes; a second `refresh` dedups by
//! `(show_id, guid)` and only counts the genuinely-new episode; and the
//! conditional-GET round-trip (an ETag stored on `add` is replayed on
//! `refresh`, and a 304 leaves the episode set untouched).
//!
//! Phase 6b-ii-c-3-b adds inbox-policy routing: a genuinely-new episode is
//! routed through the show's `inbox_policy` (AlwaysQueue / AlwaysArchive /
//! Inbox), and only new episodes route (a re-refresh never re-routes one the
//! user has since moved).

use conservatory_core::db::{
    InboxPolicy, PlayedState, ReadPool, ShowSettings, TriageBucket, WorkerHandle,
    episodes_in_bucket, get_episode_by_guid, get_playback, list_chapters, list_episodes_for_show,
    list_shows, spawn_worker,
};
use conservatory_podcasts::{Fetcher, RefreshStatus, add_show, refresh_all, refresh_show};
use tempfile::tempdir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FEED_2EP: &str = include_str!("fixtures/feeds/feed_2ep.xml");
const FEED_3EP: &str = include_str!("fixtures/feeds/feed_3ep.xml");

fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let db = dir.path().join("t.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    (dir, worker, pool)
}

#[tokio::test]
async fn add_then_refresh_dedups_by_guid() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;

    // First poll serves the 2-episode feed (once), every later poll the
    // 3-episode feed. Explicit priorities keep this independent of mount order;
    // `up_to_n_times(1)` retires the 2-ep mock after the `add`. No ETag is set,
    // so refresh sends no conditional request and always gets a 200.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FEED_2EP))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FEED_3EP))
        .with_priority(2)
        .mount(&server)
        .await;

    let url = format!("{}/feed.xml", server.uri());
    let fetcher = Fetcher::new().unwrap();

    let (show_id, new, total) = add_show(&worker, &pool, &fetcher, &url).await.unwrap();
    assert_eq!((new, total), (2, 2), "add pulls both episodes as new");

    // The show was created with a slug derived from the title.
    {
        let conn = pool.open().unwrap();
        let shows = list_shows(&conn).unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].slug, "sample-cast");
        assert_eq!(shows[0].folder_path, "Podcasts/sample-cast");
        assert_eq!(list_episodes_for_show(&conn, show_id).unwrap().len(), 2);
    }

    // Refresh: the 3-episode feed shares ep-1/ep-2, adds ep-3.
    let outcomes = refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(
        outcomes[0].status,
        RefreshStatus::Updated { new: 1, total: 3 },
        "only ep-3 is new; ep-1/ep-2 dedup by guid"
    );

    let conn = pool.open().unwrap();
    let episodes = list_episodes_for_show(&conn, show_id).unwrap();
    assert_eq!(episodes.len(), 3, "no duplicate rows for the shared guids");
    let mut guids: Vec<_> = episodes.iter().map(|e| e.guid.as_str()).collect();
    guids.sort_unstable();
    assert_eq!(guids, ["ep-1", "ep-2", "ep-3"]);

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn conditional_get_304_leaves_episodes_untouched() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;

    // The first (unconditional) request gets a 200 with an ETag and the 2-ep
    // body; it retires after one hit. The conditional request that replays the
    // ETag gets a 304. Priorities make the 200 win the first, unconditional
    // call even though its matcher would also accept the conditional one.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(FEED_2EP)
                .insert_header("ETag", "\"etag-v1\""),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("If-None-Match", "\"etag-v1\""))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(2)
        .mount(&server)
        .await;

    let url = format!("{}/feed.xml", server.uri());
    let fetcher = Fetcher::new().unwrap();

    let (show_id, new, total) = add_show(&worker, &pool, &fetcher, &url).await.unwrap();
    assert_eq!((new, total), (2, 2));

    // Re-read the stored show (it now carries the ETag) and refresh it.
    let show = {
        let conn = pool.open().unwrap();
        let shows = list_shows(&conn).unwrap();
        assert_eq!(shows[0].etag.as_deref(), Some("\"etag-v1\""));
        shows.into_iter().next().unwrap()
    };

    let outcome = refresh_show(&worker, &pool, &fetcher, show, None)
        .await
        .unwrap();
    assert_eq!(
        outcome.status,
        RefreshStatus::NotModified,
        "the replayed ETag yields a 304"
    );

    let conn = pool.open().unwrap();
    assert_eq!(
        list_episodes_for_show(&conn, show_id).unwrap().len(),
        2,
        "a 304 changes no episodes"
    );

    worker.shutdown_ack().await.unwrap();
}

/// Mount a two-then-three episode feed (the dedup harness) and `add` the show.
/// Returns the show id; ep-1/ep-2 land at add (default Inbox), ep-3 arrives on
/// the next `refresh_all` and is the genuinely-new one that routes.
async fn add_two_then_serve_three(
    worker: &WorkerHandle,
    pool: &ReadPool,
    fetcher: &Fetcher,
    server: &MockServer,
) -> i64 {
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FEED_2EP))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(FEED_3EP))
        .with_priority(2)
        .mount(server)
        .await;

    let url = format!("{}/feed.xml", server.uri());
    let (show_id, new, _) = add_show(worker, pool, fetcher, &url).await.unwrap();
    assert_eq!(new, 2, "add pulls ep-1/ep-2 as new (default Inbox)");
    show_id
}

/// Store a show's inbox policy (the other fields are schema defaults).
async fn set_policy(worker: &WorkerHandle, show_id: i64, policy: InboxPolicy) {
    worker
        .upsert_show_settings(ShowSettings {
            show_id,
            playback_speed: 1.0,
            smart_speed: true,
            voice_boost: false,
            skip_intro: 0,
            skip_outro: 0,
            skip_forward: None,
            skip_back: None,
            inbox_policy: policy,
        })
        .await
        .unwrap();
}

fn episode_id(pool: &ReadPool, show_id: i64, guid: &str) -> i64 {
    let conn = pool.open().unwrap();
    get_episode_by_guid(&conn, show_id, guid)
        .unwrap()
        .unwrap()
        .id
}

#[tokio::test]
async fn always_queue_routes_only_the_new_episode() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    let fetcher = Fetcher::new().unwrap();

    let show_id = add_two_then_serve_three(&worker, &pool, &fetcher, &server).await;
    set_policy(&worker, show_id, InboxPolicy::AlwaysQueue).await;

    // ep-3 is the only new episode on this refresh, so it is the only one queued
    // (ep-1/ep-2 predate the policy and stay in Inbox).
    refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    let ep3 = episode_id(&pool, show_id, "ep-3");
    let queued: Vec<i64> = episodes_in_bucket(&pool.open().unwrap(), TriageBucket::Queue)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(queued, vec![ep3], "only the new episode is auto-queued");

    // Remove it by hand, then refresh again: ep-3 is no longer new, so the
    // policy must not re-queue it (the only-new-episodes-route invariant).
    worker.clear_queue().await.unwrap();
    refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    assert!(
        episodes_in_bucket(&pool.open().unwrap(), TriageBucket::Queue)
            .unwrap()
            .is_empty(),
        "a re-refresh does not re-queue an already-seen episode"
    );

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn always_archive_routes_the_new_episode_out_of_inbox() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    let fetcher = Fetcher::new().unwrap();

    let show_id = add_two_then_serve_three(&worker, &pool, &fetcher, &server).await;
    set_policy(&worker, show_id, InboxPolicy::AlwaysArchive).await;

    refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    let ep3 = episode_id(&pool, show_id, "ep-3");

    let conn = pool.open().unwrap();
    assert_eq!(
        get_playback(&conn, ep3).unwrap().unwrap().played,
        PlayedState::ArchivedUnlistened,
        "the new episode is archived"
    );
    let inbox: Vec<i64> = episodes_in_bucket(&conn, TriageBucket::Inbox)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert!(!inbox.contains(&ep3), "an archived episode is out of Inbox");
    // Only ep-2 (newest at add) is in Inbox: ep-1 was archived as back
    // catalog by the first-fetch convention, and the policy archived ep-3.
    assert_eq!(
        inbox.len(),
        1,
        "only the subscribe-time newest stays in Inbox"
    );

    worker.shutdown_ack().await.unwrap();
}

/// Subscribing pulls the whole back catalog as rows, but only the newest
/// episode lands in Inbox: the rest arrive `ArchivedUnlistened` (the Castro /
/// Overcast convention), so a ten-year feed cannot flood the Inbox. A later
/// refresh is not a first fetch, so its genuinely-new episode routes normally.
#[tokio::test]
async fn first_fetch_archives_the_back_catalog() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    let fetcher = Fetcher::new().unwrap();

    let show_id = add_two_then_serve_three(&worker, &pool, &fetcher, &server).await;
    let ep1 = episode_id(&pool, show_id, "ep-1");
    let ep2 = episode_id(&pool, show_id, "ep-2");
    {
        let conn = pool.open().unwrap();
        // ep-2 is the newest by pubDate: routed per the default Inbox policy
        // (no playback row). ep-1 is back catalog: archived.
        assert!(get_playback(&conn, ep2).unwrap().is_none());
        assert_eq!(
            get_playback(&conn, ep1).unwrap().unwrap().played,
            PlayedState::ArchivedUnlistened,
            "back catalog arrives archived"
        );
        let inbox: Vec<i64> = episodes_in_bucket(&conn, TriageBucket::Inbox)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(inbox, vec![ep2], "only the newest lands in Inbox");
    }

    // The next refresh (ep-3 arrives) is not a first fetch: the new episode
    // routes normally into Inbox and the archived back catalog stays put.
    refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    let ep3 = episode_id(&pool, show_id, "ep-3");
    let conn = pool.open().unwrap();
    assert!(get_playback(&conn, ep3).unwrap().is_none());
    let inbox: Vec<i64> = episodes_in_bucket(&conn, TriageBucket::Inbox)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(inbox.len(), 2, "newest-at-add plus the new arrival");
    assert!(inbox.contains(&ep2) && inbox.contains(&ep3));
    assert_eq!(
        get_playback(&conn, ep1).unwrap().unwrap().played,
        PlayedState::ArchivedUnlistened,
        "a refresh never re-routes the archived back catalog"
    );

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn default_inbox_policy_leaves_new_episode_in_inbox() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;
    let fetcher = Fetcher::new().unwrap();

    // No settings stored: the show defaults to Inbox.
    let show_id = add_two_then_serve_three(&worker, &pool, &fetcher, &server).await;
    refresh_all(&worker, &pool, &fetcher, None).await.unwrap();
    let ep3 = episode_id(&pool, show_id, "ep-3");

    let conn = pool.open().unwrap();
    assert!(
        get_playback(&conn, ep3).unwrap().is_none(),
        "Inbox routing writes no playback row"
    );
    let inbox: Vec<i64> = episodes_in_bucket(&conn, TriageBucket::Inbox)
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert!(inbox.contains(&ep3), "the new episode lands in Inbox");

    worker.shutdown_ack().await.unwrap();
}

/// A new episode whose feed carries a `<podcast:chapters url=…>` has its chapter
/// JSON fetched and stored on `add` (Phase 6c-iii-a). The feed and the chapters
/// JSON are both served by the mock server, so the URL is rewritten to point at
/// it; `list_chapters` then returns the parsed set.
#[tokio::test]
async fn refresh_fetches_and_stores_podcast_chapters() {
    let (_dir, worker, pool) = fresh();
    let server = MockServer::start().await;

    let chapters_url = format!("{}/ep1-chapters.json", server.uri());
    // A minimal one-item feed pointing its chapters URL at the mock server.
    let feed = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
     xmlns:podcast="https://podcastindex.org/namespace/1.0">
  <channel>
    <title>Chaptered Cast</title>
    <description>Has chapters.</description>
    <item>
      <title>Episode One</title>
      <guid isPermaLink="false">ep-1</guid>
      <pubDate>Tue, 05 Mar 2024 09:00:00 GMT</pubDate>
      <enclosure url="https://cdn.example.com/ep1.mp3" length="1000000" type="audio/mpeg"/>
      <podcast:chapters url="{chapters_url}" type="application/json+chapters"/>
      <description>The first episode.</description>
    </item>
  </channel>
</rss>"#
    );
    let chapters_json = r#"{
        "version": "1.2.0",
        "chapters": [
            { "startTime": 0, "title": "Cold open" },
            { "startTime": 95.5, "title": "Interview", "endTime": 1800 }
        ]
    }"#;

    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ep1-chapters.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(chapters_json))
        .mount(&server)
        .await;

    let url = format!("{}/feed.xml", server.uri());
    let fetcher = Fetcher::new().unwrap();
    let (show_id, new, _total) = add_show(&worker, &pool, &fetcher, &url).await.unwrap();
    assert_eq!(new, 1);

    let conn = pool.open().unwrap();
    let ep = get_episode_by_guid(&conn, show_id, "ep-1")
        .unwrap()
        .unwrap();
    let chapters = list_chapters(&conn, ep.id).unwrap();
    assert_eq!(chapters.len(), 2, "both chapters were fetched and stored");
    assert_eq!(chapters[0].start_time, 0.0);
    assert_eq!(chapters[0].title.as_deref(), Some("Cold open"));
    assert_eq!(chapters[1].start_time, 95.5);
    assert_eq!(chapters[1].end_time, Some(1800.0));

    worker.shutdown_ack().await.unwrap();
}
