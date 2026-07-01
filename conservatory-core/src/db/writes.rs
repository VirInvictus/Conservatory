//! Write helpers run on the single writer connection (via the worker).
//!
//! Phase 1b ships the inserts the import pipeline and the fixture builder need;
//! update/delete land with the editor and mover in later phases. Reads never
//! come through here: they use the read pool (`reads.rs`).

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::models::{
    Album, ApeStripRow, Artist, AudioState, Book, BookChapter, BookPlayback, Chapter, Episode,
    EqState, Playback, PlaybackCursor, PlayedState, Show, ShowSettings, Track, VerifyResultRow,
    EQ_BAND_COUNT,
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

/// Overwrite the singleton active EQ state (Phase 5.5b): the live band values +
/// the selected preset name (`None` for a custom edit).
pub(crate) fn set_eq_state(conn: &Connection, state: &EqState) -> Result<()> {
    conn.execute(
        "UPDATE eq_state SET preset_name = ?1, bands = ?2 WHERE id = 0",
        params![state.preset, EqState::format_bands(&state.bands)],
    )?;
    Ok(())
}

/// Save (or overwrite) a named EQ preset (Phase 5.5b).
pub(crate) fn save_eq_preset(
    conn: &Connection,
    name: &str,
    bands: &[f64; EQ_BAND_COUNT],
) -> Result<()> {
    conn.execute(
        "INSERT INTO eq_presets (name, bands) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET bands = excluded.bands",
        params![name, EqState::format_bands(bands)],
    )?;
    Ok(())
}

/// Delete a named EQ preset (idempotent). `Flat` is seeded by the migration; the
/// CLI/UI guards against deleting it, but the write itself is unconditional.
pub(crate) fn delete_eq_preset(conn: &Connection, name: &str) -> Result<()> {
    conn.execute("DELETE FROM eq_presets WHERE name = ?1", params![name])?;
    Ok(())
}

/// Overwrite the singleton active audio configuration (Phase 5.5c): the playback
/// defaults, the DSP modules, and the output backend / resampler. Each module's
/// settings are written unconditionally (preserved across an off toggle), gated
/// only by its `enabled` flag.
pub(crate) fn set_audio_state(conn: &Connection, state: &AudioState) -> Result<()> {
    let dsp = &state.dsp;
    conn.execute(
        "UPDATE audio_state SET
            replaygain_mode = ?1, replaygain_preamp = ?2, replaygain_clip = ?3, gapless = ?4,
            comp_enabled = ?5, comp_threshold_db = ?6, comp_ratio = ?7,
            comp_attack_ms = ?8, comp_release_ms = ?9,
            limiter_enabled = ?10, limiter_ceiling_db = ?11,
            leveler_enabled = ?12, leveler_target_peak = ?13, leveler_gausssize = ?14,
            output_backend = ?15, resampler_quality = ?16, smart_speed_level = ?17
         WHERE id = 0",
        params![
            state.replaygain_mode,
            state.replaygain_preamp,
            state.replaygain_clip,
            state.gapless,
            dsp.comp.enabled,
            dsp.comp.settings.threshold_db,
            dsp.comp.settings.ratio,
            dsp.comp.settings.attack_ms,
            dsp.comp.settings.release_ms,
            dsp.limiter.enabled,
            dsp.limiter.settings.ceiling_db,
            dsp.leveler.enabled,
            dsp.leveler.settings.target_peak,
            dsp.leveler.settings.gausssize,
            state.output_backend,
            state.resampler.as_str(),
            state.smart_speed_level,
        ],
    )?;
    Ok(())
}

/// Upsert the singleton playback cursor (Phase 4a, spec §6.4). One row, id = 1;
/// later saves overwrite it. The cursor's `kind` (Phase 6b-ii-c-2) records what
/// was last playing so a restart reopens it: `track_id` is set for a track,
/// `episode_id` for an episode (the other is `None`). A cursor with neither id
/// is a cleared cursor.
pub(crate) fn save_playback_state(conn: &Connection, cursor: &PlaybackCursor) -> Result<()> {
    conn.execute(
        "INSERT INTO playback_state (id, kind, track_id, episode_id, book_id, position, paused, volume, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            track_id = excluded.track_id,
            episode_id = excluded.episode_id,
            book_id = excluded.book_id,
            position = excluded.position,
            paused = excluded.paused,
            volume = excluded.volume,
            updated_at = excluded.updated_at",
        params![
            cursor.kind.as_str(),
            cursor.track_id,
            cursor.episode_id,
            cursor.book_id,
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

/// Append books at the queue tail, preserving order (Phase 7c-iii), the
/// audiobook twin of `enqueue_episodes`.
pub(crate) fn enqueue_books(conn: &mut Connection, book_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    let base: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue",
        [],
        |r| r.get(0),
    )?;
    for (offset, &book_id) in book_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, book_id) VALUES (?1, 'audiobook', ?2)",
            params![base + offset as i64, book_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Replace the whole queue with these books in order ("play these now").
pub(crate) fn replace_queue_with_books(conn: &mut Connection, book_ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM queue", [])?;
    for (pos, &book_id) in book_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, book_id) VALUES (?1, 'audiobook', ?2)",
            params![pos as i64, book_id],
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

/// Remove a track from the library (Phase 16a, spec §3.5 context menu). This is a
/// DB-only unlink: the file is left on disk (re-importable), the row deleted. It
/// leans on the schema's cascades / triggers so nothing is orphaned: the FTS row
/// (the `tracks_ad` trigger), the genre links and any queue entries (`ON DELETE
/// CASCADE`), and the playback cursor (`ON DELETE SET NULL`). Requires
/// `foreign_keys = ON` (the schema default). The album/artist rows are left even
/// if now empty; a zero-track album never appears in the track-derived facets.
pub(crate) fn delete_track(conn: &Connection, track_id: i64) -> Result<()> {
    conn.execute("DELETE FROM tracks WHERE id = ?1", params![track_id])?;
    Ok(())
}

/// Insert `track_ids` into the queue starting at `at`, shifting every entry at or
/// after `at` up by `track_ids.len()` so positions stay contiguous (the Play Next
/// path; mirrors the engine `InsertItems`). `at` is clamped to `[0, len]`.
pub(crate) fn insert_queue_tracks_at(
    conn: &mut Connection,
    at: i64,
    track_ids: &[i64],
) -> Result<()> {
    if track_ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM queue", [], |r| r.get(0))?;
    let at = at.clamp(0, count);
    let k = track_ids.len() as i64;
    // Open the gap: push entries at/after `at` up by k (descending-safe: the
    // UNIQUE-free position column lets a single bulk shift work).
    tx.execute(
        "UPDATE queue SET position = position + ?1 WHERE position >= ?2",
        params![k, at],
    )?;
    for (offset, &track_id) in track_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO queue (position, kind, track_id) VALUES (?1, 'track', ?2)",
            params![at + offset as i64, track_id],
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

/// Append one listening session (Phase 6c-ii, spec §6.3). Append-only: every
/// session is a fresh row (no upsert), so the history and the Smart Speed
/// time-saved totals are a running ledger; the id autoincrements. The engine
/// writes one row per episode boundary from its `SessionAccumulator`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_listening_session(
    conn: &Connection,
    episode_id: Option<i64>,
    book_id: Option<i64>,
    started_at: i64,
    ended_at: i64,
    real_seconds: f64,
    audio_seconds: f64,
    smart_speed_saved: f64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO listening_sessions
            (episode_id, book_id, started_at, ended_at, real_seconds, audio_seconds, smart_speed_saved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            episode_id,
            book_id,
            started_at,
            ended_at,
            real_seconds,
            audio_seconds,
            smart_speed_saved
        ],
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

// --- Audiobooks (spec §4.5, Phase 7a-i) ---------------------------------------

/// Resolve an audiobook person by `sort_name` (the unique key, the Calibre
/// author_sort trick shared with [`get_or_create_artist`]), creating them on
/// first sight. The display `name` of an existing person is left as-is. Authors
/// and narrators share this table; the role is the link table, not the row.
pub(crate) fn get_or_create_book_person(
    conn: &Connection,
    name: &str,
    sort_name: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO book_people (name, sort_name) VALUES (?1, ?2)
         ON CONFLICT(sort_name) DO NOTHING",
        params![name, sort_name],
    )?;
    let id = conn.query_row(
        "SELECT id FROM book_people WHERE sort_name = ?1",
        params![sort_name],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Resolve a series by `name` (its unique key), creating it on first sight.
pub(crate) fn get_or_create_series(conn: &Connection, name: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO series (name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
        params![name],
    )?;
    let id = conn.query_row(
        "SELECT id FROM series WHERE name = ?1",
        params![name],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Insert a book and return its id. The `books_ai` trigger seeds its `book_fts`
/// row (title + series); the author/narrator FTS columns fill in as the links
/// are added (`link_book_author` / `link_book_narrator`).
pub(crate) fn insert_book(conn: &Connection, book: &Book) -> Result<i64> {
    conn.execute(
        "INSERT INTO books (
            title, subtitle, series_id, series_sequence, year, publisher, isbn,
            asin, description, language, shelf_genre, cover_path, accent_rgb,
            folder_path, rating, starred, added_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
        )",
        params![
            book.title,
            book.subtitle,
            book.series_id,
            book.series_sequence,
            book.year,
            book.publisher,
            book.isbn,
            book.asin,
            book.description,
            book.language,
            book.shelf_genre,
            book.cover_path,
            book.accent_rgb,
            book.folder_path,
            book.rating as i64,
            book.starred,
            book.added_at.map(|t| t.timestamp()),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Link an author to a book (role-tagged many-to-many). Idempotent: a repeated
/// credit is a no-op. The `book_authors_ai` trigger re-aggregates the book's
/// `author` FTS column.
pub(crate) fn link_book_author(conn: &Connection, book_id: i64, person_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO book_authors (book_id, person_id) VALUES (?1, ?2)
         ON CONFLICT(book_id, person_id) DO NOTHING",
        params![book_id, person_id],
    )?;
    Ok(())
}

/// Link a narrator to a book (role-tagged many-to-many). Idempotent. The
/// `book_narrators_ai` trigger re-aggregates the book's `narrator` FTS column.
pub(crate) fn link_book_narrator(conn: &Connection, book_id: i64, person_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO book_narrators (book_id, person_id) VALUES (?1, ?2)
         ON CONFLICT(book_id, person_id) DO NOTHING",
        params![book_id, person_id],
    )?;
    Ok(())
}

/// Replace a book's ordered chapter set in one transaction (the `replace_chapters`
/// pattern). Each row addresses either a standalone per-chapter file (`file_offset`
/// 0) or a span inside one M4B.
pub(crate) fn replace_book_chapters(
    conn: &mut Connection,
    book_id: i64,
    chapters: &[BookChapter],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM book_chapters WHERE book_id = ?1",
        params![book_id],
    )?;
    for ch in chapters {
        tx.execute(
            "INSERT INTO book_chapters (book_id, idx, title, file_path, file_offset, duration)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                book_id,
                ch.idx,
                ch.title,
                ch.file_path,
                ch.file_offset,
                ch.duration,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Upsert a book's resume row by `book_id` (spec §6.4, §4.5). The per-book
/// `speed` / `smart_speed` / `voice_boost` overrides are `None` to inherit the
/// global default.
pub(crate) fn upsert_book_playback(conn: &Connection, playback: &BookPlayback) -> Result<()> {
    conn.execute(
        "INSERT INTO book_playback (
            book_id, position, finished, last_played, speed, smart_speed, voice_boost
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(book_id) DO UPDATE SET
            position = excluded.position,
            finished = excluded.finished,
            last_played = excluded.last_played,
            speed = excluded.speed,
            smart_speed = excluded.smart_speed,
            voice_boost = excluded.voice_boost",
        params![
            playback.book_id,
            playback.position,
            playback.finished,
            playback.last_played.map(|t| t.timestamp()),
            playback.speed,
            playback.smart_speed,
            playback.voice_boost,
        ],
    )?;
    Ok(())
}

/// Persist a book's absolute resume position during playback (spec §6.4): the
/// engine's insurance-interval / pause / seek write. Clears `finished` (resuming
/// a finished book un-finishes it) and stamps `last_played`, preserving the
/// per-book overrides. Creates the row if absent. The completion write is
/// `complete_book`, the audiobook analogue of `complete_episode`.
pub(crate) fn set_book_position(
    conn: &Connection,
    book_id: i64,
    position: f64,
    when: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO book_playback (book_id, position, finished, last_played)
         VALUES (?1, ?2, 0, ?3)
         ON CONFLICT(book_id) DO UPDATE SET
            position = excluded.position,
            finished = 0,
            last_played = excluded.last_played",
        params![book_id, position, when],
    )?;
    Ok(())
}

/// Record a book played through to the end (spec §6.4): marks `finished`, stamps
/// `last_played`, and rewinds `position` to 0, preserving the per-book overrides.
/// Creates the row if absent.
pub(crate) fn complete_book(conn: &Connection, book_id: i64, when: Option<i64>) -> Result<()> {
    conn.execute(
        "INSERT INTO book_playback (book_id, position, finished, last_played)
         VALUES (?1, 0, 1, ?2)
         ON CONFLICT(book_id) DO UPDATE SET
            position = 0,
            finished = 1,
            last_played = excluded.last_played",
        params![book_id, when],
    )?;
    Ok(())
}

/// Set a book's `cover_path` (Phase 7a-iii), optionally refreshing `accent_rgb`
/// (on a cover change; `None` keeps the existing accent). The mirror of
/// [`set_album_cover_path`]; `cover_path` is relative to the library root.
pub(crate) fn set_book_cover_path(
    conn: &Connection,
    book_id: i64,
    cover_path: Option<&str>,
    accent_rgb: Option<u32>,
) -> Result<()> {
    conn.execute(
        "UPDATE books SET cover_path = ?2, accent_rgb = COALESCE(?3, accent_rgb) WHERE id = ?1",
        params![book_id, cover_path, accent_rgb],
    )?;
    Ok(())
}

/// Edit a book's scalar metadata (Phase 7a-iii `audiobook set`, broadened at
/// 7b-iii): title, year, series sequence, shelf genre, rating, starred. Each
/// argument is `None` to leave that column unchanged (the `update_album` shape).
/// The `books_au` trigger refreshes the title in `book_fts`. The path-affecting
/// fields (title / year / series sequence) are written here, but the *move* that
/// follows is the caller's job (the book reorganize, spec §5.7); changing the
/// series itself is [`set_book_series`] (it can clear to standalone, which
/// `COALESCE` cannot express).
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_book(
    conn: &Connection,
    book_id: i64,
    title: Option<&str>,
    year: Option<i32>,
    series_sequence: Option<f64>,
    shelf_genre: Option<&str>,
    rating: Option<u8>,
    starred: Option<bool>,
) -> Result<()> {
    conn.execute(
        "UPDATE books SET
            title = COALESCE(?2, title),
            year = COALESCE(?3, year),
            series_sequence = COALESCE(?4, series_sequence),
            shelf_genre = COALESCE(?5, shelf_genre),
            rating = COALESCE(?6, rating),
            starred = COALESCE(?7, starred)
         WHERE id = ?1",
        params![
            book_id,
            title,
            year,
            series_sequence,
            shelf_genre,
            rating.map(|r| r as i64),
            starred
        ],
    )?;
    Ok(())
}

/// Set or clear a book's series (Phase 7b-iii). `Some(id)` files it under that
/// series; `None` makes it standalone, also clearing `series_sequence` (a
/// standalone has no number). `COALESCE` cannot null a column, so the clear path
/// is its own statement. The `books_au` trigger refreshes the `series` FTS column.
pub(crate) fn set_book_series(
    conn: &Connection,
    book_id: i64,
    series_id: Option<i64>,
) -> Result<()> {
    match series_id {
        Some(id) => conn.execute(
            "UPDATE books SET series_id = ?2 WHERE id = ?1",
            params![book_id, id],
        )?,
        None => conn.execute(
            "UPDATE books SET series_id = NULL, series_sequence = NULL WHERE id = ?1",
            params![book_id],
        )?,
    };
    Ok(())
}

/// Replace a book's author set (Phase 7b-iii): clear its `book_authors` links and
/// re-link the given person ids. One transaction so a reader never sees a partial
/// set; the `book_authors_ad` / `book_authors_ai` triggers re-aggregate the
/// `author` FTS column on the delete and each insert.
pub(crate) fn set_book_authors(
    conn: &mut Connection,
    book_id: i64,
    person_ids: &[i64],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM book_authors WHERE book_id = ?1",
        params![book_id],
    )?;
    for id in person_ids {
        tx.execute(
            "INSERT INTO book_authors (book_id, person_id) VALUES (?1, ?2)
             ON CONFLICT(book_id, person_id) DO NOTHING",
            params![book_id, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Replace a book's narrator set (Phase 7b-iii), the [`set_book_authors`] shape
/// against `book_narrators`; the `book_narrators_*` triggers re-aggregate the
/// `narrator` FTS column.
pub(crate) fn set_book_narrators(
    conn: &mut Connection,
    book_id: i64,
    person_ids: &[i64],
) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM book_narrators WHERE book_id = ?1",
        params![book_id],
    )?;
    for id in person_ids {
        tx.execute(
            "INSERT INTO book_narrators (book_id, person_id) VALUES (?1, ?2)
             ON CONFLICT(book_id, person_id) DO NOTHING",
            params![book_id, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Upsert a batch of integrity-verification results (Phase 8a), keyed by
/// `file_path`, in one transaction. A re-verify of an unchanged file overwrites
/// its row (new `checked_at`); a changed file (different size/mtime) overwrites
/// the stale verdict. Empty input is a no-op.
pub(crate) fn upsert_verify_results(conn: &mut Connection, rows: &[VerifyResultRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for r in rows {
        tx.execute(
            "INSERT INTO verify_results
                 (file_path, file_size, file_mtime, verdict, detail, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(file_path) DO UPDATE SET
                 file_size  = excluded.file_size,
                 file_mtime = excluded.file_mtime,
                 verdict    = excluded.verdict,
                 detail     = excluded.detail,
                 checked_at = excluded.checked_at",
            params![
                r.file_path,
                r.file_size,
                r.file_mtime,
                r.verdict.as_str(),
                r.detail,
                r.checked_at,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Record an APE-strip undo row (Phase 8c-iii), keyed by `file_path`. Written
/// before the file is touched so the strip is reversible; a re-strip of the
/// same path overwrites the prior backup.
pub(crate) fn record_ape_strip(conn: &Connection, row: &ApeStripRow) -> Result<()> {
    conn.execute(
        "INSERT INTO ape_strips
             (file_path, ape_bytes, tag_start, orig_size, orig_mtime, stripped_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(file_path) DO UPDATE SET
             ape_bytes   = excluded.ape_bytes,
             tag_start   = excluded.tag_start,
             orig_size   = excluded.orig_size,
             orig_mtime  = excluded.orig_mtime,
             stripped_at = excluded.stripped_at",
        params![
            row.file_path,
            row.ape_bytes,
            row.tag_start,
            row.orig_size,
            row.orig_mtime,
            row.stripped_at,
        ],
    )?;
    Ok(())
}

/// Delete an APE-strip undo row (Phase 8c-iii) after the strip has been undone.
pub(crate) fn delete_ape_strip(conn: &Connection, file_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM ape_strips WHERE file_path = ?1",
        params![file_path],
    )?;
    Ok(())
}
