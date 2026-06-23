//! Write helpers run on the single writer connection (via the worker).
//!
//! Phase 1b ships the inserts the import pipeline and the fixture builder need;
//! update/delete land with the editor and mover in later phases. Reads never
//! come through here: they use the read pool (`reads.rs`).

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{
    Album, Artist, Chapter, Episode, Playback, PlaybackCursor, PlayedState, Show, ShowSettings,
    Track,
};
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

/// Update a track's ReplayGain values after a scan (Phase 5c). `None` clears the
/// column (e.g. a single track scanned without album context has no album gain).
pub(crate) fn set_track_replaygain(
    conn: &Connection,
    track_id: i64,
    track_gain: Option<f64>,
    album_gain: Option<f64>,
) -> Result<()> {
    conn.execute(
        "UPDATE tracks SET replaygain_track = ?2, replaygain_album = ?3 WHERE id = ?1",
        params![track_id, track_gain, album_gain],
    )?;
    Ok(())
}

/// Set an album's `cover_path` (Phase 5d), optionally refreshing `accent_rgb` (on
/// a cover change; `None` keeps the existing accent). `cover_path` is relative to
/// the library root, like `file_path`.
pub(crate) fn set_album_cover_path(
    conn: &Connection,
    album_id: i64,
    cover_path: Option<&str>,
    accent_rgb: Option<u32>,
) -> Result<()> {
    conn.execute(
        "UPDATE albums SET cover_path = ?2, accent_rgb = COALESCE(?3, accent_rgb) WHERE id = ?1",
        params![album_id, cover_path, accent_rgb],
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

/// Upsert the singleton playback cursor (Phase 4a, spec §6.4). One row, id = 1;
/// later saves overwrite it. The cursor's `kind` (Phase 6b-ii-c-2) records what
/// was last playing so a restart reopens it: `track_id` is set for a track,
/// `episode_id` for an episode (the other is `None`). A cursor with neither id
/// is a cleared cursor.
pub(crate) fn save_playback_state(conn: &Connection, cursor: &PlaybackCursor) -> Result<()> {
    conn.execute(
        "INSERT INTO playback_state (id, kind, track_id, episode_id, position, paused, volume, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            track_id = excluded.track_id,
            episode_id = excluded.episode_id,
            position = excluded.position,
            paused = excluded.paused,
            volume = excluded.volume,
            updated_at = excluded.updated_at",
        params![
            cursor.kind.as_str(),
            cursor.track_id,
            cursor.episode_id,
            cursor.position,
            cursor.paused,
            cursor.volume,
            cursor.updated_at
        ],
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
// concurrent writer to race the renumber. Tracks land here at Phase 4b-i,
// episodes at 6b-ii-c-1 (the `kind` column distinguishes them); books at Phase 7.

/// Append episodes at the tail, preserving order (Phase 6b-ii-c-1). Each new row
/// takes the next free position after the current maximum.
pub(crate) fn enqueue_episodes(conn: &mut Connection, episode_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    let base: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |r| r.get(0),
    )?;
    for (offset, &episode_id) in episode_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, episode_id) VALUES (?1, 'episode', ?2)",
            params![base + offset as i64, episode_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Replace the whole queue with these episodes in order ("play these now",
/// Phase 6b-ii-c-1).
pub(crate) fn replace_queue_with_episodes(
    conn: &mut Connection,
    episode_ids: &[i64],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM queue", [])?;
    for (pos, &episode_id) in episode_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, episode_id) VALUES (?1, 'episode', ?2)",
            params![pos as i64, episode_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Append tracks at the tail, preserving order (Phase 4b). Each new row takes
/// the next free position after the current maximum.
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

// --- Podcast writes (Phase 6a-i, spec §4.2). The fetch loop (Phase 6a-ii) and
// the triage UI (Phase 6b) drive these through the worker; the schema is
// core-owned (the §2.2 boundary rule). Reads use the pool (`reads.rs`).

/// Resolve a show by its `feed_url` (the subscription identity), creating it on
/// first sight (`podcast add`). An existing subscription is left untouched and
/// its id returned, so adding the same feed twice is idempotent.
pub(crate) fn get_or_create_show(conn: &Connection, show: &Show) -> Result<i64> {
    conn.execute(
        "INSERT INTO shows (
            slug, feed_url, title, author, description, homepage_url, cover_path,
            accent_rgb, apple_podcasts_id, last_fetched, last_modified, etag,
            fetch_interval, auth_user, auth_pass_ref, auto_download, keep_count,
            priority, folder_path
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19
        )
        ON CONFLICT(feed_url) DO NOTHING",
        params![
            show.slug,
            show.feed_url,
            show.title,
            show.author,
            show.description,
            show.homepage_url,
            show.cover_path,
            show.accent_rgb,
            show.apple_podcasts_id,
            show.last_fetched.map(|t| t.timestamp()),
            show.last_modified,
            show.etag,
            show.fetch_interval as i64,
            show.auth_user,
            show.auth_pass_ref,
            show.auto_download,
            show.keep_count as i64,
            show.priority as i64,
            show.folder_path,
        ],
    )?;
    let id = conn.query_row(
        "SELECT id FROM shows WHERE feed_url = ?1",
        params![show.feed_url],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Update a subscription in full by id, including the conditional-GET
/// bookkeeping (`etag` / `last_modified` / `last_fetched`) the fetch loop
/// refreshes after a poll (Phase 6a-ii). The FTS triggers re-sync on the UPDATE.
pub(crate) fn update_show(conn: &Connection, show: &Show) -> Result<()> {
    conn.execute(
        "UPDATE shows SET
            slug = ?2, feed_url = ?3, title = ?4, author = ?5, description = ?6,
            homepage_url = ?7, cover_path = ?8, accent_rgb = ?9,
            apple_podcasts_id = ?10, last_fetched = ?11, last_modified = ?12,
            etag = ?13, fetch_interval = ?14, auth_user = ?15, auth_pass_ref = ?16,
            auto_download = ?17, keep_count = ?18, priority = ?19, folder_path = ?20
         WHERE id = ?1",
        params![
            show.id,
            show.slug,
            show.feed_url,
            show.title,
            show.author,
            show.description,
            show.homepage_url,
            show.cover_path,
            show.accent_rgb,
            show.apple_podcasts_id,
            show.last_fetched.map(|t| t.timestamp()),
            show.last_modified,
            show.etag,
            show.fetch_interval as i64,
            show.auth_user,
            show.auth_pass_ref,
            show.auto_download,
            show.keep_count as i64,
            show.priority as i64,
            show.folder_path,
        ],
    )?;
    Ok(())
}

/// Delete a subscription (`podcast remove`). The FK `ON DELETE CASCADE` chain
/// removes its episodes, playback, settings, sessions, chapters, tag links, and
/// any unified-queue entries (the episode FK added in migration 0006).
pub(crate) fn delete_show(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM shows WHERE id = ?1", params![id])?;
    Ok(())
}

/// Insert or update an episode by its `(show_id, guid)` identity (spec §8): a
/// re-fetch updates the descriptive fields rather than duplicating the row.
/// `audio_path` is deliberately not overwritten on update, so a re-fetch never
/// forgets a downloaded file. Returns the episode id.
pub(crate) fn upsert_episode(conn: &Connection, episode: &Episode) -> Result<i64> {
    conn.execute(
        "INSERT INTO episodes (
            show_id, guid, title, description, pub_date, duration, file_size,
            audio_url, audio_path, folder_path, mime_type, season, episode_number,
            episode_type
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
        )
        ON CONFLICT(show_id, guid) DO UPDATE SET
            title = excluded.title,
            description = excluded.description,
            pub_date = excluded.pub_date,
            duration = excluded.duration,
            file_size = excluded.file_size,
            audio_url = excluded.audio_url,
            folder_path = excluded.folder_path,
            mime_type = excluded.mime_type,
            season = excluded.season,
            episode_number = excluded.episode_number,
            episode_type = excluded.episode_type",
        params![
            episode.show_id,
            episode.guid,
            episode.title,
            episode.description,
            episode.pub_date.map(|t| t.timestamp()),
            episode.duration,
            episode.file_size,
            episode.audio_url,
            episode.audio_path,
            episode.folder_path,
            episode.mime_type,
            episode.season,
            episode.episode_number,
            episode.episode_type,
        ],
    )?;
    let id = conn.query_row(
        "SELECT id FROM episodes WHERE show_id = ?1 AND guid = ?2",
        params![episode.show_id, episode.guid],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Record an episode's downloaded `audio_path` (Phase 6a-iii-b). Set explicitly
/// because `upsert_episode` deliberately preserves `audio_path` across a
/// re-fetch; the FTS triggers re-sync on the UPDATE.
pub(crate) fn set_episode_audio_path(
    conn: &Connection,
    episode_id: i64,
    audio_path: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET audio_path = ?2 WHERE id = ?1",
        params![episode_id, audio_path],
    )?;
    Ok(())
}

/// Clear an episode's downloaded `audio_path` (retention prune, Phase
/// 6b-ii-c-3-b): the on-disk file has been removed, so the row reverts to
/// stream-only. The counterpart to `set_episode_audio_path`.
pub(crate) fn clear_episode_audio_path(conn: &Connection, episode_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE episodes SET audio_path = NULL WHERE id = ?1",
        params![episode_id],
    )?;
    Ok(())
}

/// Upsert an episode's triage/playback row by `episode_id` (the triage actions
/// and the resume cursor, spec §4.2).
pub(crate) fn upsert_playback(conn: &Connection, playback: &Playback) -> Result<()> {
    conn.execute(
        "INSERT INTO playback (
            episode_id, position, played, last_played, play_count, starred
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(episode_id) DO UPDATE SET
            position = excluded.position,
            played = excluded.played,
            last_played = excluded.last_played,
            play_count = excluded.play_count,
            starred = excluded.starred",
        params![
            playback.episode_id,
            playback.position,
            playback.played.as_i64(),
            playback.last_played.map(|t| t.timestamp()),
            playback.play_count as i64,
            playback.starred,
        ],
    )?;
    Ok(())
}

/// Set an episode's played state without touching `starred` / `play_count` (the
/// triage actions, Phase 6b-ii-b). Marking Unplayed also rewinds the resume
/// `position` (the Belfry behaviour). Creates the playback row if absent.
pub(crate) fn set_episode_played(
    conn: &Connection,
    episode_id: i64,
    state: PlayedState,
    when: Option<i64>,
) -> Result<()> {
    let reset_position = state == PlayedState::Unplayed;
    conn.execute(
        "INSERT INTO playback (episode_id, played, position, last_played)
         VALUES (?1, ?2, 0, ?3)
         ON CONFLICT(episode_id) DO UPDATE SET
            played = excluded.played,
            last_played = excluded.last_played,
            position = CASE WHEN ?4 THEN 0 ELSE position END",
        params![episode_id, state.as_i64(), when, reset_position],
    )?;
    Ok(())
}

/// Toggle an episode's `starred` flag without touching played/position (6b-ii-b).
/// Creates the playback row if absent.
pub(crate) fn set_episode_starred(conn: &Connection, episode_id: i64, starred: bool) -> Result<()> {
    conn.execute(
        "INSERT INTO playback (episode_id, starred) VALUES (?1, ?2)
         ON CONFLICT(episode_id) DO UPDATE SET starred = excluded.starred",
        params![episode_id, starred],
    )?;
    Ok(())
}

/// Persist an episode's resume position during playback (Phase 6b-ii-c-2): the
/// engine's insurance-interval / pause / seek write. Marks the episode
/// `InProgress` and stamps `last_played`, preserving `starred` / `play_count`
/// (the partial-upsert discipline of the triage writes). Creates the row if
/// absent. The completion bump is `complete_episode`, not this.
pub(crate) fn set_episode_position(
    conn: &Connection,
    episode_id: i64,
    position: f64,
    when: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO playback (episode_id, position, played, last_played)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(episode_id) DO UPDATE SET
            position = excluded.position,
            played = excluded.played,
            last_played = excluded.last_played",
        params![episode_id, position, PlayedState::InProgress.as_i64(), when],
    )?;
    Ok(())
}

/// Record an episode played through to the end (Phase 6b-ii-c-2): the engine's
/// natural-EOF write. Marks `PlayedFully`, bumps `play_count`, stamps
/// `last_played`, and rewinds `position` to 0 (a finished episode re-plays from
/// the start), preserving `starred`. The podcast analogue of
/// `increment_play_count` + the music cursor's end-of-file handling.
pub(crate) fn complete_episode(
    conn: &Connection,
    episode_id: i64,
    when: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO playback (episode_id, position, played, last_played, play_count)
         VALUES (?1, 0, ?2, ?3, 1)
         ON CONFLICT(episode_id) DO UPDATE SET
            position = 0,
            played = excluded.played,
            last_played = excluded.last_played,
            play_count = play_count + 1",
        params![episode_id, PlayedState::PlayedFully.as_i64(), when],
    )?;
    Ok(())
}

/// Upsert a show's per-show overrides by `show_id` (spec §3.7).
pub(crate) fn upsert_show_settings(conn: &Connection, settings: &ShowSettings) -> Result<()> {
    conn.execute(
        "INSERT INTO show_settings (
            show_id, playback_speed, smart_speed, voice_boost, skip_intro,
            skip_outro, skip_forward, skip_back, inbox_policy
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(show_id) DO UPDATE SET
            playback_speed = excluded.playback_speed,
            smart_speed = excluded.smart_speed,
            voice_boost = excluded.voice_boost,
            skip_intro = excluded.skip_intro,
            skip_outro = excluded.skip_outro,
            skip_forward = excluded.skip_forward,
            skip_back = excluded.skip_back,
            inbox_policy = excluded.inbox_policy",
        params![
            settings.show_id,
            settings.playback_speed,
            settings.smart_speed,
            settings.voice_boost,
            settings.skip_intro as i64,
            settings.skip_outro as i64,
            settings.skip_forward.map(|v| v as i64),
            settings.skip_back.map(|v| v as i64),
            settings.inbox_policy.as_str(),
        ],
    )?;
    Ok(())
}

/// Replace an episode's chapter set (spec §8): clear and re-insert in one
/// transaction so a reader never sees a partial set. The `id` on each `Chapter`
/// is ignored (the rows are reassigned).
pub(crate) fn replace_chapters(
    conn: &mut Connection,
    episode_id: i64,
    chapters: &[Chapter],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM chapters WHERE episode_id = ?1",
        params![episode_id],
    )?;
    for ch in chapters {
        tx.execute(
            "INSERT INTO chapters (episode_id, start_time, end_time, title, url, image_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                episode_id,
                ch.start_time,
                ch.end_time,
                ch.title,
                ch.url,
                ch.image_path,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Resolve a tag name to its id, creating it on first sight (spec §8). The name
/// is the unique key, mirroring `get_or_create_genre`.
pub(crate) fn get_or_create_tag(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        params![name],
    )?;
    let id = conn.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
        r.get(0)
    })?;
    Ok(id)
}

/// Replace a show's tag set (spec §8, the OPML round-trip side): clear its
/// `show_tags` links and re-link the given names (get-or-create each), in one
/// transaction. Mirrors `set_track_genres`.
pub(crate) fn set_show_tags(conn: &mut Connection, show_id: i64, tags: &[String]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM show_tags WHERE show_id = ?1", params![show_id])?;
    for name in tags {
        tx.execute(
            "INSERT INTO tags (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            params![name],
        )?;
        let tag_id: i64 =
            tx.query_row("SELECT id FROM tags WHERE name = ?1", params![name], |r| {
                r.get(0)
            })?;
        tx.execute(
            "INSERT INTO show_tags (show_id, tag_id) VALUES (?1, ?2)
             ON CONFLICT(show_id, tag_id) DO NOTHING",
            params![show_id, tag_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}
