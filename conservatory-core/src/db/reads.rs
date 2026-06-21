//! Read helpers. These take a borrowed `&Connection` so they run on a read-only
//! pool handle (`ReadPool::open`), never on the writer (spec §2.1). The CLI and,
//! later, the GTK side consume the `Artist` / `Album` / `Track` models through
//! these. Phase 1b ships counts and basic lookups; richer queries (faceting,
//! search) arrive with `conservatory-search` at Phase 3.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{Album, Artist, MediaKind, Perspective, QueueItem, Track};
use crate::errors::Result;

/// Library-wide row counts, the Phase 1b "does it load" sanity surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibraryCounts {
    pub artists: i64,
    pub albums: i64,
    pub tracks: i64,
}

fn count(conn: &Connection, table: &str) -> Result<i64> {
    // `table` is a fixed internal string, never user input.
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

pub fn library_counts(conn: &Connection) -> Result<LibraryCounts> {
    Ok(LibraryCounts {
        artists: count(conn, "artists")?,
        albums: count(conn, "albums")?,
        tracks: count(conn, "tracks")?,
    })
}

pub fn get_artist(conn: &Connection, id: i64) -> Result<Option<Artist>> {
    conn.query_row(
        "SELECT * FROM artists WHERE id = ?1",
        params![id],
        row_to_artist,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_album(conn: &Connection, id: i64) -> Result<Option<Album>> {
    conn.query_row(
        "SELECT * FROM albums WHERE id = ?1",
        params![id],
        row_to_album,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_track(conn: &Connection, id: i64) -> Result<Option<Track>> {
    conn.query_row(
        "SELECT * FROM tracks WHERE id = ?1",
        params![id],
        row_to_track,
    )
    .optional()
    .map_err(Into::into)
}

/// Fetch many tracks by id in one read (Phase 4b-ii: building a play queue from a
/// browse list). Chunked under SQLite's bound-variable limit so a full-library
/// activation still works; the result order is unspecified, so the caller pairs
/// rows back to its own order by `Track::id`.
pub fn get_tracks(conn: &Connection, ids: &[i64]) -> Result<Vec<Track>> {
    // Comfortably under SQLite's default SQLITE_MAX_VARIABLE_NUMBER (~999/32766).
    const CHUNK: usize = 900;
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!("SELECT * FROM tracks WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), row_to_track)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

pub fn list_albums(conn: &Connection) -> Result<Vec<Album>> {
    let mut stmt = conn.prepare("SELECT * FROM albums ORDER BY title")?;
    let rows = stmt.query_map([], row_to_album)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A track joined with the album/artist context the path-template engine needs
/// (spec §5.1, Phase 2a). `album_artist_sort` is `None` for a compilation, which
/// the renderer buckets under Various Artists. `track_id` carries through so the
/// caller can pair a rendered path back to its row (the future mover).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRenderRow {
    pub track_id: i64,
    pub album_id: Option<i64>,
    pub shelf_genre: Option<String>,
    pub album_artist_sort: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub title: String,
    pub track_artist: Option<String>,
    pub format: Option<String>,
    /// The track's current managed path (relative to the library root); the move
    /// source and the `db_old` value for the journal (Phase 2c).
    pub file_path: String,
}

/// Every track with the album/artist fields needed to render its target path.
/// Ordered to mirror the default tree (genre → album artist → album → disc/track)
/// so CLI output and previews read top-down.
pub fn track_render_rows(conn: &Connection) -> Result<Vec<TrackRenderRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.title, t.track_no, t.disc_no, t.format, t.file_path,
                al.title AS album, al.shelf_genre, al.year,
                aa.sort_name AS album_artist_sort,
                ta.name AS track_artist
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         ORDER BY al.shelf_genre, aa.sort_name, al.title, t.disc_no, t.track_no",
    )?;
    let rows = stmt.query_map([], |row| {
        let track_no: Option<i64> = row.get("track_no")?;
        let disc_no: Option<i64> = row.get("disc_no")?;
        Ok(TrackRenderRow {
            track_id: row.get("id")?,
            album_id: row.get("album_id")?,
            shelf_genre: row.get("shelf_genre")?,
            album_artist_sort: row.get("album_artist_sort")?,
            album: row.get("album")?,
            year: row.get("year")?,
            track_no: track_no.map(|n| n as u32),
            disc_no: disc_no.map(|n| n as u32),
            title: row.get("title")?,
            track_artist: row.get("track_artist")?,
            format: row.get("format")?,
            file_path: row.get("file_path")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The singleton playback cursor (spec §6.4, Phase 4a): what was playing and
/// where, read on startup to resume. Absent (`None`) on a library that has
/// never played anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackStateRow {
    pub track_id: Option<i64>,
    pub position: f64,
    pub paused: bool,
    pub volume: i64,
    pub updated_at: Option<i64>,
}

/// Read the saved playback cursor, if any (the row with id = 1).
pub fn read_playback_state(conn: &Connection) -> Result<Option<PlaybackStateRow>> {
    conn.query_row(
        "SELECT track_id, position, paused, volume, updated_at
         FROM playback_state WHERE id = 1",
        [],
        |row| {
            Ok(PlaybackStateRow {
                track_id: row.get("track_id")?,
                position: row.get("position")?,
                paused: row.get::<_, i64>("paused")? != 0,
                volume: row.get("volume")?,
                updated_at: row.get("updated_at")?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// The unified queue in order (spec §4.3, §6.1, Phase 4b). The engine resolves
/// each `track` row into a `PlayableItem`; episode/book rows arrive at Phases
/// 6/7. Ordered by the contiguous `position`.
pub fn load_queue(conn: &Connection) -> Result<Vec<QueueItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, position, kind, track_id, episode_id, book_id
         FROM queue ORDER BY position",
    )?;
    let rows = stmt.query_map([], |row| {
        let kind: String = row.get("kind")?;
        // The kind column is CHECK-constrained to the three known values, so a
        // parse failure here means a corrupt DB; surface it as a row error.
        let kind = kind.parse::<MediaKind>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(QueueItem {
            id: row.get("id")?,
            position: row.get("position")?,
            kind,
            track_id: row.get("track_id")?,
            episode_id: row.get("episode_id")?,
            book_id: row.get("book_id")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A track projected for search (Phase 3a). The CLI/GUI maps this to
/// `conservatory_search::SearchItem` for the in-memory fallback path; `track_id`
/// pairs a match back to its row.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    pub track_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub shelf_genre: Option<String>,
    pub genres: Vec<String>,
    pub year: Option<i32>,
    pub added: Option<i64>,
    pub rating: u8,
    pub bitrate: Option<i32>,
    pub duration: Option<f64>,
    pub format: Option<String>,
    pub played: bool,
    pub starred: bool,
    pub queued: bool,
}

const GENRE_SEP: char = '\u{1f}'; // unit separator: safe group_concat delimiter

/// Every track with the full search projection (Phase 3a). Genres are aggregated
/// via `group_concat`; `played`/`queued` are derived. Ordered by track id.
pub fn search_rows(conn: &Connection) -> Result<Vec<SearchRow>> {
    // `queued` is membership in the unified queue (Phase 4b): a track is queued
    // iff a `kind='track'` row references it. EXISTS keeps it one row per track
    // regardless of how many times the track appears in the queue.
    let sql = format!(
        "SELECT t.id, t.title, t.added_at, t.rating, t.bitrate, t.duration, t.format,
                t.play_count, t.starred,
                ta.name AS track_artist, aa.name AS album_artist,
                al.title AS album, al.shelf_genre, al.year,
                EXISTS (SELECT 1 FROM queue q
                         WHERE q.kind = 'track' AND q.track_id = t.id) AS queued,
                (SELECT group_concat(g.name, '{GENRE_SEP}')
                   FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                  WHERE tg.track_id = t.id) AS genres
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         ORDER BY t.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let genres: Option<String> = row.get("genres")?;
        let play_count: i64 = row.get("play_count")?;
        Ok(SearchRow {
            track_id: row.get("id")?,
            title: row.get("title")?,
            artist: row.get("track_artist")?,
            album_artist: row.get("album_artist")?,
            album: row.get("album")?,
            shelf_genre: row.get("shelf_genre")?,
            genres: genres
                .map(|g| g.split(GENRE_SEP).map(str::to_string).collect())
                .unwrap_or_default(),
            year: row.get("year")?,
            added: row.get("added_at")?,
            rating: row.get::<_, i64>("rating")? as u8,
            bitrate: row.get("bitrate")?,
            duration: row.get("duration")?,
            format: row.get("format")?,
            played: play_count > 0,
            starred: row.get("starred")?,
            queued: row.get::<_, i64>("queued")? != 0,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A bindable value for the search SQL path, mirroring
/// `conservatory_search::SqlValue` so the CLI/GUI need not depend on rusqlite.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlParam {
    Text(String),
    Int(i64),
    Real(f64),
}

/// Run a translated search `WHERE` fragment against `tracks`, returning the
/// matching track ids (Phase 3a SQL fast path). The fragment and `params` come
/// from `conservatory_search::try_translate`; binding happens here so the crate
/// stays storage-agnostic.
pub fn search_track_ids(
    conn: &Connection,
    where_sql: &str,
    params: &[SqlParam],
) -> Result<Vec<i64>> {
    let sql = format!("SELECT id FROM tracks WHERE {where_sql} ORDER BY id");
    let bound: Vec<rusqlite::types::Value> = params
        .iter()
        .map(|p| match p {
            SqlParam::Text(s) => rusqlite::types::Value::Text(s.clone()),
            SqlParam::Int(n) => rusqlite::types::Value::Integer(*n),
            SqlParam::Real(x) => rusqlite::types::Value::Real(*x),
        })
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bound), |r| r.get(0))?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// FTS5 `bm25` scores for a `track_fts MATCH` query, keyed by track id (Phase 3a
/// ranking). Lower `bm25` magnitude is a better match; the caller blends it with
/// recency via `conservatory_search::blend_relevance`.
pub fn fts_rank(conn: &Connection, match_query: &str) -> Result<HashMap<i64, f64>> {
    let mut stmt =
        conn.prepare("SELECT rowid, bm25(track_fts) FROM track_fts WHERE track_fts MATCH ?1")?;
    let rows = stmt.query_map(params![match_query], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (id, score) = row?;
        out.insert(id, score);
    }
    Ok(out)
}

/// Per-track raw genre lists for one album, used by the shelf-genre resolver
/// (spec §5.2). One inner `Vec` per track, in track id order; a track with no
/// genres contributes an empty `Vec` so track counts stay accurate.
pub fn album_track_genres(conn: &Connection, album_id: i64) -> Result<Vec<Vec<String>>> {
    let mut stmt = conn.prepare(
        "SELECT t.id AS track_id, g.name AS genre
         FROM tracks t
         LEFT JOIN track_genres tg ON tg.track_id = t.id
         LEFT JOIN genres g ON g.id = tg.genre_id
         WHERE t.album_id = ?1
         ORDER BY t.id",
    )?;
    let rows = stmt.query_map(params![album_id], |row| {
        Ok((
            row.get::<_, i64>("track_id")?,
            row.get::<_, Option<String>>("genre")?,
        ))
    })?;

    let mut out: Vec<Vec<String>> = Vec::new();
    let mut current_id: Option<i64> = None;
    for row in rows {
        let (track_id, genre) = row?;
        if current_id != Some(track_id) {
            out.push(Vec::new());
            current_id = Some(track_id);
        }
        if let Some(name) = genre {
            out.last_mut().expect("row pushed above").push(name);
        }
    }
    Ok(out)
}

fn epoch_to_dt(secs: Option<i64>) -> Option<DateTime<Utc>> {
    secs.and_then(|s| Utc.timestamp_opt(s, 0).single())
}

pub(crate) fn row_to_artist(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: row.get("id")?,
        name: row.get("name")?,
        sort_name: row.get("sort_name")?,
        musicbrainz_id: row.get("musicbrainz_id")?,
    })
}

pub(crate) fn row_to_album(row: &rusqlite::Row<'_>) -> rusqlite::Result<Album> {
    let accent: Option<i64> = row.get("accent_rgb")?;
    let added_at: Option<i64> = row.get("added_at")?;
    Ok(Album {
        id: row.get("id")?,
        title: row.get("title")?,
        album_artist_id: row.get("album_artist_id")?,
        shelf_genre: row.get("shelf_genre")?,
        year: row.get("year")?,
        release_date: row.get("release_date")?,
        musicbrainz_release_id: row.get("musicbrainz_release_id")?,
        cover_path: row.get("cover_path")?,
        accent_rgb: accent.map(|v| v as u32),
        folder_path: row.get("folder_path")?,
        added_at: epoch_to_dt(added_at),
    })
}

pub(crate) fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    let last_played: Option<i64> = row.get("last_played")?;
    let added_at: Option<i64> = row.get("added_at")?;
    Ok(Track {
        id: row.get("id")?,
        album_id: row.get("album_id")?,
        artist_id: row.get("artist_id")?,
        title: row.get("title")?,
        track_no: row.get("track_no")?,
        disc_no: row.get("disc_no")?,
        duration: row.get("duration")?,
        file_path: row.get("file_path")?,
        format: row.get("format")?,
        bitrate: row.get("bitrate")?,
        sample_rate: row.get("sample_rate")?,
        replaygain_track: row.get("replaygain_track")?,
        replaygain_album: row.get("replaygain_album")?,
        rating: row.get::<_, i64>("rating")? as u8,
        play_count: row.get::<_, i64>("play_count")? as u32,
        last_played: epoch_to_dt(last_played),
        starred: row.get("starred")?,
        musicbrainz_recording_id: row.get("musicbrainz_recording_id")?,
        added_at: epoch_to_dt(added_at),
    })
}

/// All saved Perspectives, ordered by name (Phase 3c, spec §3.4).
pub fn list_perspectives(conn: &Connection) -> Result<Vec<Perspective>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, expression, scope FROM perspectives ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Perspective {
            id: r.get("id")?,
            name: r.get("name")?,
            expression: r.get("expression")?,
            scope: r.get("scope")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The raw expression text of a Perspective by name, for `vl:NAME` expansion
/// (the storage-backed `PerspectiveResolver`). `None` if no such name.
pub fn perspective_expression(conn: &Connection, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT expression FROM perspectives WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?)
}
