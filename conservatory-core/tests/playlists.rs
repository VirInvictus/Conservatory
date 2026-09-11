//! Phase 16d: playlist storage + static materialisation + the smart-playlist SQL
//! order/limit primitive. Smart *query evaluation* is the CLI/GUI's job (it needs
//! the search grammar, which core is free of at runtime), so it is not tested here;
//! `ordered_track_ids` is exercised with a literal `where_sql`.

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    MediaKind, Playlist, PlaylistKind, PlaylistOrder, QueueDisplayRow, ReadPool, Show,
    get_playlist, get_tracks, list_playlists, load_playlist_display, load_queue_display,
    ordered_track_ids, spawn_worker, static_playlist_track_ids,
};
use conservatory_core::{build_track_edit, parse_assignment};
use tempfile::tempdir;

/// A minimal subscribed show + one episode, for the mixed-entry tests.
fn sample_show() -> Show {
    Show {
        id: 0,
        slug: "mixed-test".into(),
        feed_url: "https://example.invalid/feed.xml".into(),
        title: "Mixed Test Show".into(),
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
        folder_path: "Podcasts/mixed-test".into(),
    }
}

fn setup() -> (
    tempfile::TempDir,
    conservatory_core::db::WorkerHandle,
    ReadPool,
) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    (dir, worker, ReadPool::new(path, 3).unwrap())
}

#[tokio::test]
async fn static_playlist_crud_and_ordering() {
    let (_dir, worker, pool) = setup();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    let id = worker
        .create_playlist("Faves".into(), PlaylistKind::Static, None, None, None, 100)
        .await
        .unwrap();

    // Append preserves order; a second append extends the tail.
    worker
        .append_playlist_tracks(id, vec![5, 3, 9])
        .await
        .unwrap();
    worker.append_playlist_tracks(id, vec![1]).await.unwrap();
    assert_eq!(track_ids(&pool, id), vec![5, 3, 9, 1]);

    // Reorder: move the head (position 0) to position 2.
    worker.reorder_playlist_entry(id, 0, 2).await.unwrap();
    assert_eq!(track_ids(&pool, id), vec![3, 9, 5, 1]);

    // Remove the entry at position 1 (track 9); the gap closes.
    worker.remove_playlist_entry(id, 1).await.unwrap();
    assert_eq!(track_ids(&pool, id), vec![3, 5, 1]);

    // The playlist row round-trips.
    let conn = pool.open().unwrap();
    let pl = get_playlist(&conn, id).unwrap().unwrap();
    assert_eq!(pl.kind, PlaylistKind::Static);
    assert_eq!(pl.name, "Faves");
    assert!(pl.query.is_none() && pl.order_by.is_none());
    drop(conn);

    // Deleting the playlist cascades its entries away.
    worker.delete_playlist(id).await.unwrap();
    let conn = pool.open().unwrap();
    assert!(get_playlist(&conn, id).unwrap().is_none());
    assert!(static_playlist_track_ids(&conn, id).unwrap().is_empty());
}

#[tokio::test]
async fn smart_row_persists_query_limit_order() {
    let (_dir, worker, pool) = setup();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let id = worker
        .create_playlist(
            "Top rated".into(),
            PlaylistKind::Smart,
            Some("rating:>=4".into()),
            Some(50),
            Some(PlaylistOrder::Rating),
            200,
        )
        .await
        .unwrap();

    let conn = pool.open().unwrap();
    let pl: Playlist = get_playlist(&conn, id).unwrap().unwrap();
    assert_eq!(pl.kind, PlaylistKind::Smart);
    assert_eq!(pl.query.as_deref(), Some("rating:>=4"));
    assert_eq!(pl.limit_n, Some(50));
    assert_eq!(pl.order_by, Some(PlaylistOrder::Rating));

    // list_playlists surfaces it.
    assert!(list_playlists(&conn).unwrap().iter().any(|p| p.id == id));
}

#[tokio::test]
async fn ordered_track_ids_sorts_and_limits() {
    let (_dir, worker, pool) = setup();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    // Give three tracks distinct high ratings, the rest stay 0.
    for (track, stars) in [(5, "5"), (3, "4"), (9, "3")] {
        let edit = build_track_edit(&[parse_assignment(&format!("rating={stars}")).unwrap()]);
        worker.update_track(track, edit).await.unwrap();
    }
    let conn = pool.open().unwrap();

    // Highest-rated first, capped at three: exactly our three, in rating order.
    let top = ordered_track_ids(&conn, "1=1", &[], PlaylistOrder::Rating, Some(3)).unwrap();
    assert_eq!(top, vec![5, 3, 9]);

    // The limit is honoured.
    let two = ordered_track_ids(&conn, "1=1", &[], PlaylistOrder::Rating, Some(2)).unwrap();
    assert_eq!(two.len(), 2);

    // Title order returns every track, non-decreasing by title.
    let by_title = ordered_track_ids(&conn, "1=1", &[], PlaylistOrder::Title, None).unwrap();
    assert_eq!(by_title.len(), 80);
    let name: std::collections::HashMap<i64, String> = get_tracks(&conn, &by_title)
        .unwrap()
        .into_iter()
        .map(|t| (t.id, t.title.to_lowercase()))
        .collect();
    let ordered_titles: Vec<&String> = by_title.iter().map(|id| &name[id]).collect();
    let mut sorted = ordered_titles.clone();
    sorted.sort();
    assert_eq!(ordered_titles, sorted, "title order is non-decreasing");
}

// --- helpers

fn track_ids(pool: &ReadPool, playlist_id: i64) -> Vec<i64> {
    let conn = pool.open().unwrap();
    static_playlist_track_ids(&conn, playlist_id).unwrap()
}

fn display_rows(pool: &ReadPool, playlist_id: i64) -> Vec<QueueDisplayRow> {
    let conn = pool.open().unwrap();
    load_playlist_display(&conn, playlist_id).unwrap()
}

#[tokio::test]
async fn static_playlist_holds_mixed_kinds_in_order() {
    // The 1003 mixed entries: episode and book entries append, reorder, and
    // remove exactly like tracks, and the display read resolves their labels
    // and episode sources for the engine rebuild.
    let (_dir, worker, pool) = setup();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    let show_id = worker.get_or_create_show(sample_show()).await.unwrap();
    let episode_id = worker
        .upsert_episode(conservatory_core::db::Episode {
            id: 0,
            show_id,
            guid: "mixed-1".into(),
            title: "Episode One".into(),
            description: None,
            pub_date: None,
            duration: Some(1800),
            file_size: None,
            audio_url: Some("https://cdn.example.invalid/e1.mp3".into()),
            audio_path: None,
            folder_path: "Podcasts/mixed-test/e1".into(),
            mime_type: Some("audio/mpeg".into()),
            season: None,
            episode_number: None,
            episode_type: None,
        })
        .await
        .unwrap();
    let book_id = worker
        .insert_book(conservatory_core::db::Book {
            id: 0,
            title: "Mixed Book".into(),
            subtitle: None,
            series_id: None,
            series_sequence: None,
            year: Some(2020),
            publisher: None,
            isbn: None,
            asin: None,
            description: None,
            language: None,
            shelf_genre: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "Audiobooks/Standalone/Mixed Book (2020)".into(),
            rating: 0,
            starred: false,
            added_at: None,
        })
        .await
        .unwrap();

    let id = worker
        .create_playlist("Mixed".into(), PlaylistKind::Static, None, None, None, 100)
        .await
        .unwrap();
    // track, episode, book, track: interleaved kinds in position order.
    worker
        .append_playlist_entries(
            id,
            vec![
                (MediaKind::Track, 7),
                (MediaKind::Episode, episode_id),
                (MediaKind::Audiobook, book_id),
                (MediaKind::Track, 3),
            ],
        )
        .await
        .unwrap();

    // The track-only read still returns just the tracks, in position order.
    assert_eq!(track_ids(&pool, id), vec![7, 3]);
    // The display read carries every kind with labels resolved.
    let rows = display_rows(&pool, id);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].kind, MediaKind::Track);
    assert_eq!(rows[0].track_id, Some(7));
    assert!(!rows[0].title.is_empty(), "the label join resolved");
    assert_eq!(rows[1].kind, MediaKind::Episode);
    assert_eq!(rows[1].episode_id, Some(episode_id));
    assert_eq!(
        rows[1].audio_url.as_deref(),
        Some("https://cdn.example.invalid/e1.mp3")
    );
    assert_eq!(rows[2].kind, MediaKind::Audiobook);
    assert_eq!(rows[2].book_id, Some(book_id));
    assert_eq!(rows[2].title, "Mixed Book");

    // Reorder across kinds: the book (position 2) to the head.
    worker.reorder_playlist_entry(id, 2, 0).await.unwrap();
    let rows = display_rows(&pool, id);
    assert_eq!(rows[0].kind, MediaKind::Audiobook);
    assert_eq!(rows[1].kind, MediaKind::Track);
    assert_eq!(rows[1].track_id, Some(7));
    assert_eq!(rows[2].kind, MediaKind::Episode);

    // Removing an episode entry closes the gap; the CHECK shape survives a
    // kind-specific cascade (deleting the show cascades its episode entry).
    worker.remove_playlist_entry(id, 3).await.unwrap();
    assert_eq!(display_rows(&pool, id).len(), 3);
    worker.delete_show(show_id).await.unwrap();
    let rows = display_rows(&pool, id);
    assert_eq!(rows.len(), 2, "the episode entry cascaded with its show");
    assert!(rows.iter().all(|r| r.kind != MediaKind::Episode));

    // The queue takes the mixed order as one replace.
    let items: Vec<(MediaKind, i64)> = rows
        .iter()
        .filter_map(|r| {
            let kind = r.kind;
            r.track_id
                .map(|i| (kind, i))
                .or(r.episode_id.map(|i| (kind, i)))
                .or(r.book_id.map(|i| (kind, i)))
        })
        .collect();
    worker.replace_queue_mixed(items).await.unwrap();
    let conn = pool.open().unwrap();
    let queue = load_queue_display(&conn).unwrap();
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0].kind, MediaKind::Audiobook);
    assert_eq!(queue[1].kind, MediaKind::Track);
}
