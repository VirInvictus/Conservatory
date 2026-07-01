//! Phase 16d: playlist storage + static materialisation + the smart-playlist SQL
//! order/limit primitive. Smart *query evaluation* is the CLI/GUI's job (it needs
//! the search grammar, which core is free of at runtime), so it is not tested here;
//! `ordered_track_ids` is exercised with a literal `where_sql`.

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    Playlist, PlaylistKind, PlaylistOrder, ReadPool, get_playlist, get_tracks, list_playlists,
    ordered_track_ids, spawn_worker, static_playlist_track_ids,
};
use conservatory_core::{build_track_edit, parse_assignment};
use tempfile::tempdir;

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

// --- helper

fn track_ids(pool: &ReadPool, playlist_id: i64) -> Vec<i64> {
    let conn = pool.open().unwrap();
    static_playlist_track_ids(&conn, playlist_id).unwrap()
}
