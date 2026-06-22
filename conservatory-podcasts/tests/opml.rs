//! Phase 6a-iii-a: OPML import/export through a real core worker (temp DB).
//! Network-free: import creates subscription rows + tags; export reads them
//! back. Covers the round-trip and the applePodcastsID / tag preservation.

use conservatory_core::db::{ReadPool, WorkerHandle, list_shows, list_tags_for_show, spawn_worker};
use conservatory_podcasts::opml::{OpmlSubscription, export_opml, import_opml, parse_opml};
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let db = dir.path().join("t.db");
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    (dir, worker, pool)
}

const OPML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test export</title></head>
  <body>
    <outline text="News" >
      <outline type="rss" text="The Daily" xmlUrl="https://feeds.example/daily.xml"
               category="news,politics" applePodcastsID="1200361736"/>
    </outline>
    <outline type="rss" text="Reply All" xmlUrl="https://feeds.example/replyall.xml"/>
  </body>
</opml>"#;

#[tokio::test]
async fn import_creates_shows_tags_and_apple_id() {
    let (_dir, worker, pool) = fresh();

    let summary = import_opml(&worker, &pool, OPML.as_bytes()).await.unwrap();
    assert_eq!(summary.total, 2);
    assert_eq!(summary.created, 2);

    let conn = pool.open().unwrap();
    let mut shows = list_shows(&conn).unwrap();
    shows.sort_by(|a, b| a.feed_url.cmp(&b.feed_url));
    assert_eq!(shows.len(), 2);

    let daily = shows.iter().find(|s| s.title == "The Daily").unwrap();
    assert_eq!(daily.feed_url, "https://feeds.example/daily.xml");
    assert_eq!(daily.apple_podcasts_id.as_deref(), Some("1200361736"));
    assert_eq!(daily.slug, "the-daily");
    assert_eq!(daily.folder_path, "Podcasts/the-daily");
    let mut tags: Vec<_> = list_tags_for_show(&conn, daily.id)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    tags.sort();
    assert_eq!(tags, ["news", "politics"]);

    // The untagged show has no tag links.
    let reply = shows.iter().find(|s| s.title == "Reply All").unwrap();
    assert!(list_tags_for_show(&conn, reply.id).unwrap().is_empty());

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn import_is_idempotent_and_export_round_trips() {
    let (_dir, worker, pool) = fresh();

    import_opml(&worker, &pool, OPML.as_bytes()).await.unwrap();
    // Re-import: no new shows, the second pass reports zero created.
    let again = import_opml(&worker, &pool, OPML.as_bytes()).await.unwrap();
    assert_eq!((again.total, again.created), (2, 0));
    assert_eq!(list_shows(&pool.open().unwrap()).unwrap().len(), 2);

    // Export and re-parse: the same subscription set comes back.
    let exported = export_opml(&pool).await.unwrap();
    let mut parsed = parse_opml(exported.as_bytes());
    parsed.sort_by(|a, b| a.feed_url.cmp(&b.feed_url));

    let expected = vec![
        OpmlSubscription {
            feed_url: "https://feeds.example/daily.xml".to_string(),
            title: "The Daily".to_string(),
            apple_podcasts_id: Some("1200361736".to_string()),
            tags: vec!["news".to_string(), "politics".to_string()],
        },
        OpmlSubscription {
            feed_url: "https://feeds.example/replyall.xml".to_string(),
            title: "Reply All".to_string(),
            apple_podcasts_id: None,
            tags: vec![],
        },
    ];
    assert_eq!(parsed, expected);

    worker.shutdown_ack().await.unwrap();
}
