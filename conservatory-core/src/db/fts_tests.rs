//! FTS5 trigger-sync tests (spec §4.4).
//!
//! These exercise the `track_fts` / `album_fts` triggers directly on a writer
//! connection (crate-internal): insert/update/delete plus the rename-propagation
//! triggers that keep the denormalized `artist` / `album` / `album_artist`
//! columns correct when an artist or album is renamed.

use rusqlite::{Connection, params};

use crate::db::models::{Album, Artist, Track};
use crate::db::{connection, migrations, writes};

fn fresh_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = connection::open_writer(&dir.path().join("test.db")).unwrap();
    migrations::run(&mut conn).unwrap();
    (dir, conn)
}

fn artist(name: &str, sort: &str) -> Artist {
    Artist {
        id: 0,
        name: name.to_string(),
        sort_name: sort.to_string(),
        musicbrainz_id: None,
    }
}

fn album(title: &str, album_artist_id: i64) -> Album {
    Album {
        id: 0,
        title: title.to_string(),
        album_artist_id: Some(album_artist_id),
        shelf_genre: Some("Electronic".to_string()),
        year: Some(2001),
        release_date: None,
        musicbrainz_release_id: None,
        cover_path: None,
        accent_rgb: None,
        folder_path: format!("Electronic/{title}"),
        added_at: None,
    }
}

fn track(title: &str, album_id: i64, artist_id: i64) -> Track {
    Track {
        id: 0,
        album_id: Some(album_id),
        artist_id: Some(artist_id),
        title: title.to_string(),
        track_no: Some(1),
        disc_no: Some(1),
        duration: Some(120.0),
        file_path: format!("Electronic/{title}.flac"),
        format: Some("flac".to_string()),
        bitrate: Some(1024),
        sample_rate: Some(44100),
        replaygain_track: None,
        replaygain_album: None,
        rating: 0,
        play_count: 0,
        last_played: None,
        starred: false,
        musicbrainz_recording_id: None,
        added_at: None,
    }
}

fn fts_col(conn: &Connection, table: &str, col: &str, rowid: i64) -> Option<String> {
    let sql = format!("SELECT {col} FROM {table} WHERE rowid = ?1");
    conn.query_row(&sql, params![rowid], |r| r.get(0)).ok()
}

fn match_tracks(conn: &Connection, query: &str) -> Vec<i64> {
    let mut stmt = conn
        .prepare("SELECT rowid FROM track_fts WHERE track_fts MATCH ?1")
        .unwrap();
    let rows = stmt.query_map(params![query], |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

#[test]
fn insert_populates_denormalized_columns() {
    let (_dir, conn) = fresh_db();
    let aid = writes::insert_artist(&conn, &artist("Aphex Twin", "Aphex Twin")).unwrap();
    let alid = writes::insert_album(&conn, &album("Drukqs", aid)).unwrap();
    let tid = writes::insert_track(&conn, &track("Avril 14th", alid, aid)).unwrap();

    assert_eq!(
        fts_col(&conn, "track_fts", "artist", tid).as_deref(),
        Some("Aphex Twin")
    );
    assert_eq!(
        fts_col(&conn, "track_fts", "album", tid).as_deref(),
        Some("Drukqs")
    );
    assert_eq!(match_tracks(&conn, "avril"), vec![tid]);
    assert_eq!(match_tracks(&conn, "artist:aphex"), vec![tid]);

    assert_eq!(
        fts_col(&conn, "album_fts", "album_artist", alid).as_deref(),
        Some("Aphex Twin")
    );
}

#[test]
fn update_track_title_resyncs_fts() {
    let (_dir, conn) = fresh_db();
    let aid =
        writes::insert_artist(&conn, &artist("Boards of Canada", "Boards of Canada")).unwrap();
    let alid = writes::insert_album(&conn, &album("Geogaddi", aid)).unwrap();
    let tid = writes::insert_track(&conn, &track("Sunshine Recorder", alid, aid)).unwrap();

    conn.execute(
        "UPDATE tracks SET title = ?1 WHERE id = ?2",
        params!["Dawn Chorus", tid],
    )
    .unwrap();

    assert!(match_tracks(&conn, "dawn").contains(&tid));
    assert!(match_tracks(&conn, "sunshine").is_empty());
}

#[test]
fn rename_artist_propagates_into_track_and_album_fts() {
    let (_dir, conn) = fresh_db();
    let aid = writes::insert_artist(&conn, &artist("Aphex Twin", "Aphex Twin")).unwrap();
    let alid = writes::insert_album(&conn, &album("Drukqs", aid)).unwrap();
    let tid = writes::insert_track(&conn, &track("Avril 14th", alid, aid)).unwrap();

    conn.execute(
        "UPDATE artists SET name = ?1 WHERE id = ?2",
        params!["AFX", aid],
    )
    .unwrap();

    assert_eq!(
        fts_col(&conn, "track_fts", "artist", tid).as_deref(),
        Some("AFX")
    );
    assert_eq!(
        fts_col(&conn, "album_fts", "album_artist", alid).as_deref(),
        Some("AFX")
    );
}

#[test]
fn rename_album_propagates_into_track_fts() {
    let (_dir, conn) = fresh_db();
    let aid = writes::insert_artist(&conn, &artist("Aphex Twin", "Aphex Twin")).unwrap();
    let alid = writes::insert_album(&conn, &album("Drukqs", aid)).unwrap();
    let tid = writes::insert_track(&conn, &track("Avril 14th", alid, aid)).unwrap();

    conn.execute(
        "UPDATE albums SET title = ?1 WHERE id = ?2",
        params!["Syro", alid],
    )
    .unwrap();

    assert_eq!(
        fts_col(&conn, "track_fts", "album", tid).as_deref(),
        Some("Syro")
    );
    assert_eq!(
        fts_col(&conn, "album_fts", "title", alid).as_deref(),
        Some("Syro")
    );
}

#[test]
fn delete_track_removes_fts_row() {
    let (_dir, conn) = fresh_db();
    let aid = writes::insert_artist(&conn, &artist("Aphex Twin", "Aphex Twin")).unwrap();
    let alid = writes::insert_album(&conn, &album("Drukqs", aid)).unwrap();
    let tid = writes::insert_track(&conn, &track("Avril 14th", alid, aid)).unwrap();

    conn.execute("DELETE FROM tracks WHERE id = ?1", params![tid])
        .unwrap();

    assert!(fts_col(&conn, "track_fts", "title", tid).is_none());
    assert!(match_tracks(&conn, "avril").is_empty());
}
