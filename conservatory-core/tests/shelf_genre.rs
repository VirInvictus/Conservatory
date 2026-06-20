//! Phase 2b integration: derive shelf genres from a real fixture library. The
//! exhaustive chain/normalization coverage lives in the `shelf_genre` unit
//! tests; this checks the DB-driven `resolve_album` against the fixture's stored
//! values (the sub-phase's "stable shelf_genre for any album" usable artifact).

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{Album, Artist, ReadPool, Track, list_albums, spawn_worker};
use conservatory_core::{GenreVocab, resolve_album};
use tempfile::tempdir;

#[tokio::test]
async fn resolver_matches_fixture_shelf_genres() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();

    // Empty vocabulary (the v1 default, spec §16.4): the fixture tags each track
    // with its album's genre, so the chain's most-common step re-derives exactly
    // the value the fixture stored.
    let vocab = GenreVocab::load(&conn).unwrap();
    let albums = list_albums(&conn).unwrap();
    assert!(!albums.is_empty());

    for album in &albums {
        let derived = resolve_album(&conn, album.id, &vocab).unwrap();
        assert_eq!(
            Some(derived.as_str()),
            album.shelf_genre.as_deref(),
            "album {} shelf genre",
            album.id
        );
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn album_with_no_track_genres_resolves_to_unknown() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");

    let worker = spawn_worker(path.clone()).unwrap();
    // An album row with one track but no genre links.
    let artist = worker
        .insert_artist(Artist {
            id: 0,
            name: "Nobody".into(),
            sort_name: "Nobody".into(),
            musicbrainz_id: None,
        })
        .await
        .unwrap();
    let album = worker
        .insert_album(Album {
            id: 0,
            title: "Untagged".into(),
            album_artist_id: Some(artist),
            shelf_genre: None,
            year: None,
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "x".into(),
            added_at: None,
        })
        .await
        .unwrap();
    worker
        .insert_track(Track {
            id: 0,
            album_id: Some(album),
            artist_id: Some(artist),
            title: "T".into(),
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

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let derived = resolve_album(&conn, album, &GenreVocab::empty()).unwrap();
    assert_eq!(derived, "Unknown");

    worker.shutdown_ack().await.unwrap();
}
