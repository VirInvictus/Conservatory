//! Write helpers run on the single writer connection (via the worker).
//!
//! Phase 1b ships the inserts the import pipeline and the fixture builder need;
//! update/delete land with the editor and mover in later phases. Reads never
//! come through here: they use the read pool (`reads.rs`).

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{Album, Artist, Track};
use crate::errors::Result;

pub(crate) fn insert_artist(conn: &Connection, artist: &Artist) -> Result<i64> {
    conn.execute(
        "INSERT INTO artists (name, sort_name, musicbrainz_id) VALUES (?1, ?2, ?3)",
        params![artist.name, artist.sort_name, artist.musicbrainz_id],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Resolve an artist by its `sort_name` (the unique key, the Calibre author_sort
/// trick), creating it on first sight (Phase 2d import). The display `name` of an
/// existing artist is left as-is.
pub(crate) fn get_or_create_artist(
    conn: &Connection,
    name: &str,
    sort_name: &str,
    musicbrainz_id: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO artists (name, sort_name, musicbrainz_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(sort_name) DO NOTHING",
        params![name, sort_name, musicbrainz_id],
    )?;
    let id = conn.query_row(
        "SELECT id FROM artists WHERE sort_name = ?1",
        params![sort_name],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Resolve an album by `(album_artist_id, title)`, creating it (with the supplied
/// derived fields) on first sight (Phase 2d). Album identity is artist + title;
/// a compilation has `album_artist_id = NULL` (the NULL-aware match handles it).
pub(crate) fn get_or_create_album(conn: &Connection, album: &Album) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM albums
             WHERE title = ?1
               AND ((album_artist_id IS NULL AND ?2 IS NULL) OR album_artist_id = ?2)",
            params![album.title, album.album_artist_id],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(id) => Ok(id),
        None => insert_album(conn, album),
    }
}

/// Set an album's shelf genre (the §5.2 filed-under value). A path-affecting edit;
/// the caller re-renders the tree (`organize`) to move the album (Phase 2d).
pub(crate) fn set_album_shelf_genre(
    conn: &Connection,
    album_id: i64,
    shelf_genre: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE albums SET shelf_genre = ?2 WHERE id = ?1",
        params![album_id, shelf_genre],
    )?;
    Ok(())
}

pub(crate) fn insert_album(conn: &Connection, album: &Album) -> Result<i64> {
    conn.execute(
        "INSERT INTO albums (
            title, album_artist_id, shelf_genre, year, release_date,
            musicbrainz_release_id, cover_path, accent_rgb, folder_path, added_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            album.title,
            album.album_artist_id,
            album.shelf_genre,
            album.year,
            album.release_date,
            album.musicbrainz_release_id,
            album.cover_path,
            album.accent_rgb,
            album.folder_path,
            album.added_at.map(|t| t.timestamp()),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn insert_track(conn: &Connection, track: &Track) -> Result<i64> {
    conn.execute(
        "INSERT INTO tracks (
            album_id, artist_id, title, track_no, disc_no, duration, file_path,
            format, bitrate, sample_rate, replaygain_track, replaygain_album,
            rating, play_count, last_played, starred, musicbrainz_recording_id, added_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
        )",
        params![
            track.album_id,
            track.artist_id,
            track.title,
            track.track_no,
            track.disc_no,
            track.duration,
            track.file_path,
            track.format,
            track.bitrate,
            track.sample_rate,
            track.replaygain_track,
            track.replaygain_album,
            track.rating as i64,
            track.play_count as i64,
            track.last_played.map(|t| t.timestamp()),
            track.starred,
            track.musicbrainz_recording_id,
            track.added_at.map(|t| t.timestamp()),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Resolve a raw genre name to its id, creating the row on first sight. The
/// raw tag is stored verbatim (the §5.2 decoupling); normalization to a shelf
/// genre is a separate concern (Phase 2b).
pub(crate) fn get_or_create_genre(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO genres (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        params![name],
    )?;
    let id = conn.query_row(
        "SELECT id FROM genres WHERE name = ?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub(crate) fn link_track_genre(conn: &Connection, track_id: i64, genre_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, ?2)
         ON CONFLICT(track_id, genre_id) DO NOTHING",
        params![track_id, genre_id],
    )?;
    Ok(())
}

/// Save a Perspective (Phase 3c, spec §3.4), returning its id. Names are unique:
/// saving an existing name overwrites its expression (the obvious "update my
/// saved search" behavior), leaving the original `created_at` in place.
pub(crate) fn save_perspective(
    conn: &Connection,
    name: &str,
    expression: &str,
    scope: &str,
    created_at: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO perspectives (name, expression, scope, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET expression = excluded.expression, scope = excluded.scope",
        params![name, expression, scope, created_at],
    )?;
    let id = conn.query_row(
        "SELECT id FROM perspectives WHERE name = ?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Delete a Perspective by id (idempotent: deleting a gone row is fine).
pub(crate) fn delete_perspective(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM perspectives WHERE id = ?1", params![id])?;
    Ok(())
}
