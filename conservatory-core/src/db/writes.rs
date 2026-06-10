//! Write helpers run on the single writer connection (via the worker).
//!
//! Phase 1b ships the inserts the import pipeline and the fixture builder need;
//! update/delete land with the editor and mover in later phases. Reads never
//! come through here: they use the read pool (`reads.rs`).

use rusqlite::{Connection, params};

use crate::db::models::{Album, Artist, Track};
use crate::errors::Result;

pub(crate) fn insert_artist(conn: &Connection, artist: &Artist) -> Result<i64> {
    conn.execute(
        "INSERT INTO artists (name, sort_name, musicbrainz_id) VALUES (?1, ?2, ?3)",
        params![artist.name, artist.sort_name, artist.musicbrainz_id],
    )?;
    Ok(conn.last_insert_rowid())
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
