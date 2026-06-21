//! Write helpers run on the single writer connection (via the worker).
//!
//! Phase 1b ships the inserts the import pipeline and the fixture builder need;
//! update/delete land with the editor and mover in later phases. Reads never
//! come through here: they use the read pool (`reads.rs`).

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{Album, Artist, Track};
use crate::edit::{AlbumEdit, TrackEdit};
use crate::errors::Result;
use crate::import::resolve::derive_sort_name;

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

/// Apply a track-level field edit (Phase 5a, spec §3.5). Only the `Some` fields
/// change (`COALESCE(new, old)` leaves the rest untouched); setting `artist`
/// resolves it through `get_or_create_artist` (by derived sort name) and points
/// the track at that artist. The FTS triggers re-sync on the UPDATE.
pub(crate) fn update_track(conn: &Connection, track_id: i64, edit: &TrackEdit) -> Result<()> {
    let artist_id = match &edit.artist {
        Some(name) => Some(get_or_create_artist(
            conn,
            name,
            &derive_sort_name(name),
            None,
        )?),
        None => None,
    };
    conn.execute(
        "UPDATE tracks SET
            title = COALESCE(?2, title),
            rating = COALESCE(?3, rating),
            artist_id = COALESCE(?4, artist_id)
         WHERE id = ?1",
        params![
            track_id,
            edit.title,
            edit.rating.map(|r| r as i64),
            artist_id
        ],
    )?;
    Ok(())
}

/// Apply an album-level field edit (Phase 5a). Album-level edits change the whole
/// album (every track under it). `shelf_genre`, `album`, `album_artist`, and
/// `year` are path-affecting: the caller re-renders and moves (spec §5.4).
pub(crate) fn update_album(conn: &Connection, album_id: i64, edit: &AlbumEdit) -> Result<()> {
    let album_artist_id = match &edit.album_artist {
        Some(name) => Some(get_or_create_artist(
            conn,
            name,
            &derive_sort_name(name),
            None,
        )?),
        None => None,
    };
    conn.execute(
        "UPDATE albums SET
            title = COALESCE(?2, title),
            year = COALESCE(?3, year),
            shelf_genre = COALESCE(?4, shelf_genre),
            album_artist_id = COALESCE(?5, album_artist_id)
         WHERE id = ?1",
        params![
            album_id,
            edit.title,
            edit.year,
            edit.shelf_genre,
            album_artist_id
        ],
    )?;
    Ok(())
}

/// Replace a track's raw genre set (Phase 5a, the §5.2 multi-value side): clear
/// its `track_genres` links and re-link the given names (get-or-create each).
/// Never touches `shelf_genre`. One transaction so a reader never sees a partial
/// set. Genres are not in the FTS columns, so no FTS resync is needed; the genre
/// facet reads `track_genres` directly.
pub(crate) fn set_track_genres(
    conn: &mut Connection,
    track_id: i64,
    genres: &[String],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM track_genres WHERE track_id = ?1",
        params![track_id],
    )?;
    for name in genres {
        tx.execute(
            "INSERT INTO genres (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            params![name],
        )?;
        let genre_id: i64 = tx.query_row(
            "SELECT id FROM genres WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, ?2)
             ON CONFLICT(track_id, genre_id) DO NOTHING",
            params![track_id, genre_id],
        )?;
    }
    tx.commit()?;
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

/// Upsert the singleton playback cursor (Phase 4a, spec §6.4). One row, id = 1;
/// later saves overwrite it. A `None` track clears the cursor.
pub(crate) fn save_playback_state(
    conn: &Connection,
    track_id: Option<i64>,
    position: f64,
    paused: bool,
    volume: i64,
    updated_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO playback_state (id, track_id, position, paused, volume, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            track_id = excluded.track_id,
            position = excluded.position,
            paused = excluded.paused,
            volume = excluded.volume,
            updated_at = excluded.updated_at",
        params![track_id, position, paused, volume, updated_at],
    )?;
    Ok(())
}

/// Record a completed play (spec §6.4): bump `play_count` and stamp
/// `last_played`. Only natural end-of-file reaches here (`EndReason::Eof`); the
/// caller gates on that.
pub(crate) fn increment_play_count(conn: &Connection, track_id: i64, played_at: i64) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
        params![track_id, played_at],
    )?;
    Ok(())
}

// --- Unified queue (Phase 4b, spec §4.3). Positions stay contiguous 0..n-1;
// every mutation is one transaction on the single writer, so there is no
// concurrent writer to race the renumber. Phase 4b-i enqueues `track` rows only.

/// Append tracks at the tail, preserving order. Each new row takes the next
/// free position after the current maximum.
pub(crate) fn enqueue_tracks(conn: &mut Connection, track_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    let base: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |r| r.get(0),
    )?;
    for (offset, &track_id) in track_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, track_id) VALUES (?1, 'track', ?2)",
            params![base + offset as i64, track_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Remove the entry at `position` and close the gap (shift everything after it
/// down by one), keeping positions contiguous.
pub(crate) fn remove_queue_item(conn: &mut Connection, position: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let removed = tx.execute("DELETE FROM queue WHERE position = ?1", params![position])?;
    if removed > 0 {
        tx.execute(
            "UPDATE queue SET position = position - 1 WHERE position > ?1",
            params![position],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Move the entry at `from` to `to`, shifting the items between them to keep
/// positions contiguous. Both are clamped to the current range; a no-op `from`
/// (nothing there) leaves the queue untouched.
pub(crate) fn reorder_queue(conn: &mut Connection, from: i64, to: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM queue", [], |r| r.get(0))?;
    if count > 0 {
        let to = to.clamp(0, count - 1);
        let id: Option<i64> = tx
            .query_row(
                "SELECT id FROM queue WHERE position = ?1",
                params![from],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            if from < to {
                tx.execute(
                    "UPDATE queue SET position = position - 1 WHERE position > ?1 AND position <= ?2",
                    params![from, to],
                )?;
            } else if from > to {
                tx.execute(
                    "UPDATE queue SET position = position + 1 WHERE position >= ?1 AND position < ?2",
                    params![to, from],
                )?;
            }
            if from != to {
                tx.execute(
                    "UPDATE queue SET position = ?1 WHERE id = ?2",
                    params![to, id],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Empty the queue.
pub(crate) fn clear_queue(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM queue", [])?;
    Ok(())
}

/// Replace the whole queue with `track_ids` in order (the "play these now" path).
pub(crate) fn replace_queue_with_tracks(conn: &mut Connection, track_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM queue", [])?;
    for (pos, &track_id) in track_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, track_id) VALUES (?1, 'track', ?2)",
            params![pos as i64, track_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}
