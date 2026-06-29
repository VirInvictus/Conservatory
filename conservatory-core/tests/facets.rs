//! Phase 3b: faceted-browse queries against a fixture library. Verifies counts,
//! the cascade (an upstream selection narrows the next pane), multi-value genre
//! faceting, and the leaf track set.

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    Album, Artist, FacetField, FacetFilter, ReadPool, Track, facet_rows, facet_tracks, spawn_worker,
};
use tempfile::tempdir;

fn rows(pool: &ReadPool, target: FacetField, filters: &[FacetFilter]) -> Vec<(String, i64)> {
    let conn = pool.open().unwrap();
    facet_rows(&conn, target, filters)
        .unwrap()
        .into_iter()
        .map(|r| (r.value, r.count))
        .collect()
}

#[tokio::test]
async fn fixture_cascade_and_counts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    // The small fixture: 5 genres across 10 albums × 8 tracks = 80 tracks; each
    // album's 8 tracks all carry that album's single genre.
    let genres = rows(&pool, FacetField::Genre, &[]);
    let total: i64 = genres.iter().map(|(_, c)| c).sum();
    assert_eq!(total, 80, "all tracks counted across the genre facet");
    assert!(genres.iter().any(|(v, _)| v == "Electronic"));

    // Selecting one genre narrows the AlbumArtist pane to that genre's tracks.
    let electronic = FacetFilter {
        field: FacetField::Genre,
        values: vec!["Electronic".into()],
    };
    let artists_all = rows(&pool, FacetField::AlbumArtist, &[]);
    let artists_elec = rows(
        &pool,
        FacetField::AlbumArtist,
        std::slice::from_ref(&electronic),
    );
    let sum_elec: i64 = artists_elec.iter().map(|(_, c)| c).sum();
    assert!(sum_elec < total, "the cascade narrows the downstream pane");
    assert!(artists_elec.len() <= artists_all.len());

    // The leaf track set under that genre matches the genre's own count.
    let conn = pool.open().unwrap();
    let leaf = facet_tracks(&conn, std::slice::from_ref(&electronic)).unwrap();
    let elec_count = genres.iter().find(|(v, _)| v == "Electronic").unwrap().1;
    assert_eq!(leaf.len() as i64, elec_count);

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn new_single_valued_field_facets_and_cascade() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    // Each new single-valued facet partitions the 80 tracks exactly (every track
    // maps to one bucket, unlike multi-value Genre), so the per-bucket counts sum
    // back to 80.
    for field in [
        FacetField::ShelfGenre,
        FacetField::Artist,
        FacetField::Year,
        FacetField::Format,
    ] {
        let buckets = rows(&pool, field, &[]);
        assert!(!buckets.is_empty(), "{field:?} produced no rows");
        let total: i64 = buckets.iter().map(|(_, c)| c).sum();
        assert_eq!(total, 80, "{field:?} buckets must partition all 80 tracks");
    }

    // The cascade narrows a new-field pane (Format) by an upstream Genre pick.
    let electronic = FacetFilter {
        field: FacetField::Genre,
        values: vec!["Electronic".into()],
    };
    let format_elec = rows(&pool, FacetField::Format, std::slice::from_ref(&electronic));
    let sum_elec: i64 = format_elec.iter().map(|(_, c)| c).sum();
    assert!(sum_elec > 0 && sum_elec < 80, "the cascade narrows Format");

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn multi_value_genre_counts_under_each() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();

    // One album, one track, tagged with two genres.
    let artist = worker
        .insert_artist(Artist {
            id: 0,
            name: "BoC".into(),
            sort_name: "BoC".into(),
            musicbrainz_id: None,
        })
        .await
        .unwrap();
    let album = worker
        .insert_album(Album {
            id: 0,
            title: "Geogaddi".into(),
            album_artist_id: Some(artist),
            shelf_genre: Some("Electronic".into()),
            year: Some(2002),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "x".into(),
            added_at: None,
        })
        .await
        .unwrap();
    let track = worker
        .insert_track(Track {
            id: 0,
            album_id: Some(album),
            artist_id: Some(artist),
            title: "Music Is Math".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: None,
            file_path: "x/1.flac".into(),
            format: Some("flac".into()),
            bitrate: None,
            sample_rate: None,
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: None,
        })
        .await
        .unwrap();
    for g in ["Electronic", "Ambient"] {
        let gid = worker.get_or_create_genre(g).await.unwrap();
        worker.link_track_genre(track, gid).await.unwrap();
    }

    let pool = ReadPool::new(path, 3).unwrap();
    let genres = rows(&pool, FacetField::Genre, &[]);
    // The single track appears under BOTH genre rows.
    assert_eq!(genres.len(), 2);
    assert!(genres.iter().all(|(_, c)| *c == 1));

    worker.shutdown_ack().await.unwrap();
}
