//! Phase 6a-ii-b: the refresh pipeline end to end, against a local wiremock
//! server and a real core single-writer worker (temp DB). Hermetic, no real
//! network. Covers: `add` lands episodes; a second `refresh` dedups by
//! `(show_id, guid)` and only counts the genuinely-new episode; and the
//! conditional-GET round-trip (an ETag stored on `add` is replayed on
//! `refresh`, and a 304 leaves the episode set untouched).

use conservatory_core::db::{
    ReadPool, WorkerHandle, list_episodes_for_show, list_shows, spawn_worker,
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
    let outcomes = refresh_all(&worker, &pool, &fetcher).await.unwrap();
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

    let outcome = refresh_show(&worker, &pool, &fetcher, show).await.unwrap();
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
