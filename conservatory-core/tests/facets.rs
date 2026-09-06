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
        FacetField::Rating,
        FacetField::Added,
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
async fn rating_and_added_facets_bucket_and_narrow() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    // Every fixture track starts unrated (the 0 default), so the Rating pane
    // is one worded bucket covering all 80; rating three of them splits it.
    let unrated = rows(&pool, FacetField::Rating, &[]);
    assert_eq!(unrated, vec![("Unrated".to_string(), 80)]);

    let conn = pool.open().unwrap();
    let ids: Vec<i64> = facet_tracks(&conn, &[])
        .unwrap()
        .iter()
        .take(3)
        .map(|t| t.id)
        .collect();
    drop(conn);
    for id in &ids {
        worker
            .update_track(
                *id,
                conservatory_core::edit::TrackEdit {
                    rating: Some(3),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let mut rated = rows(&pool, FacetField::Rating, &[]);
    rated.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        rated,
        vec![("3".to_string(), 3), ("Unrated".to_string(), 77)]
    );

    // The pane's value feeds back as a filter: the "3" bucket narrows the leaf
    // set to exactly the three rated tracks.
    let rating3 = FacetFilter {
        field: FacetField::Rating,
        values: vec!["3".into()],
    };
    let conn = pool.open().unwrap();
    let leaf3 = facet_tracks(&conn, std::slice::from_ref(&rating3)).unwrap();
    drop(conn);
    assert_eq!(leaf3.len(), 3);
    assert_eq!(
        leaf3
            .iter()
            .map(|t| t.id)
            .collect::<std::collections::HashSet<_>>(),
        ids.iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
    );

    // The Added pane buckets timestamps to their added month. The fixture all
    // landed "now", so two tracks backdated 45 days (always a prior month)
    // split it into two YYYY-MM buckets, and the backdated bucket's value
    // narrows the leaf set to exactly those two tracks.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let back = chrono::DateTime::from_timestamp(now - 45 * 86400, 0).unwrap();
    let artist = worker
        .insert_artist(Artist {
            id: 0,
            name: "Backdated".into(),
            sort_name: "Backdated".into(),
            musicbrainz_id: None,
        })
        .await
        .unwrap();
    let album = worker
        .insert_album(Album {
            id: 0,
            title: "Old Stock".into(),
            album_artist_id: Some(artist),
            shelf_genre: None,
            year: None,
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "Backdated/Old Stock".into(),
            added_at: Some(back),
        })
        .await
        .unwrap();
    let mut back_ids = Vec::new();
    for t in 0..2 {
        let track_id = worker
            .insert_track(Track {
                id: 0,
                album_id: Some(album),
                artist_id: Some(artist),
                title: format!("Old {t}"),
                track_no: Some(t + 1),
                disc_no: Some(1),
                duration: Some(100.0),
                file_path: format!("Backdated/Old Stock/{t}.flac"),
                format: Some("flac".into()),
                bitrate: Some(1024),
                sample_rate: Some(44100),
                replaygain_track: None,
                replaygain_album: None,
                rating: 0,
                play_count: 0,
                last_played: None,
                starred: false,
                musicbrainz_recording_id: None,
                added_at: Some(back),
            })
            .await
            .unwrap();
        back_ids.push(track_id);
    }

    let added = rows(&pool, FacetField::Added, &[]);
    assert_eq!(
        added.len(),
        2,
        "two added months: the fixture's and the backdated pair's"
    );
    let total: i64 = added.iter().map(|(_, c)| c).sum();
    assert_eq!(total, 82, "the month buckets partition all 82 tracks");
    let back_bucket = added
        .iter()
        .find(|(_, c)| *c == 2)
        .map(|(v, _)| v.clone())
        .expect("the backdated month holds exactly the 2 new tracks");
    let now_bucket = added
        .iter()
        .find(|(_, c)| *c == 80)
        .map(|(v, _)| v.as_str())
        .unwrap();
    assert!(back_bucket != now_bucket);
    // ISO months sort chronologically, and each bucket value is YYYY-MM shaped.
    for (v, _) in &added {
        assert_eq!(v.len(), 7, "{v:?} is a YYYY-MM bucket");
        assert_eq!(v.as_bytes()[4], b'-');
    }

    let back_filter = FacetFilter {
        field: FacetField::Added,
        values: vec![back_bucket],
    };
    let conn = pool.open().unwrap();
    let back_leaf = facet_tracks(&conn, std::slice::from_ref(&back_filter)).unwrap();
    drop(conn);
    assert_eq!(back_leaf.iter().map(|t| t.id).collect::<Vec<_>>(), back_ids);

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

#[tokio::test]
async fn leaf_carries_album_cover_and_accent() {
    // Phase 12b: facet_tracks projects the album's cover_path + accent_rgb onto
    // each leaf row, so the browse cover column can render art per track.
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();

    let artist = worker
        .insert_artist(Artist {
            id: 0,
            name: "Aphex Twin".into(),
            sort_name: "Aphex Twin".into(),
            musicbrainz_id: None,
        })
        .await
        .unwrap();
    let album = worker
        .insert_album(Album {
            id: 0,
            title: "Selected Ambient Works 85-92".into(),
            album_artist_id: Some(artist),
            shelf_genre: Some("Electronic".into()),
            year: Some(1992),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: Some("Electronic/Aphex Twin/SAW (1992)/cover.jpg".into()),
            accent_rgb: Some(0x00c4_746e),
            folder_path: "Electronic/Aphex Twin/SAW (1992)".into(),
            added_at: None,
        })
        .await
        .unwrap();
    worker
        .insert_track(Track {
            id: 0,
            album_id: Some(album),
            artist_id: Some(artist),
            title: "Xtal".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: None,
            file_path: "Electronic/Aphex Twin/SAW (1992)/01 Xtal.flac".into(),
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

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let leaf = facet_tracks(&conn, &[]).unwrap();
    assert_eq!(leaf.len(), 1);
    assert_eq!(
        leaf[0].cover_path.as_deref(),
        Some("Electronic/Aphex Twin/SAW (1992)/cover.jpg")
    );
    assert_eq!(leaf[0].accent_rgb, Some(0x00c4_746e));

    worker.shutdown_ack().await.unwrap();
}
