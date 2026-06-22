//! Read helpers. These take a borrowed `&Connection` so they run on a read-only
//! pool handle (`ReadPool::open`), never on the writer (spec §2.1). The CLI and,
//! later, the GTK side consume the `Artist` / `Album` / `Track` models through
//! these. Phase 1b ships counts and basic lookups; richer queries (faceting,
//! search) arrive with `conservatory-search` at Phase 3.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{
    Album, Artist, Chapter, Episode, InboxPolicy, MediaKind, Perspective, Playback, PlayedState,
    QueueItem, Show, ShowSettings, Tag, Track,
};
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

/// A track with the full descriptive metadata to write back into its file
/// (Phase 5b, spec §5.5). Unlike `TrackRenderRow` this carries both the display
/// name and the sort name for the track and album artists, plus the raw genres,
/// since the embedded tags want all of them. Totals are not persisted, so they
/// are not written (§5.6 does not need them).
#[derive(Debug, Clone, PartialEq)]
pub struct WritebackRow {
    pub track_id: i64,
    pub file_path: String,
    pub format: Option<String>,
    pub title: String,
    pub track_artist: Option<String>,
    pub track_artist_sort: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub album_artist_sort: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genres: Vec<String>,
}

impl From<&WritebackRow> for crate::tags::TagWrite {
    fn from(r: &WritebackRow) -> Self {
        crate::tags::TagWrite {
            title: r.title.clone(),
            track_artist: r.track_artist.clone(),
            track_artist_sort: r.track_artist_sort.clone(),
            album: r.album.clone(),
            album_artist: r.album_artist.clone(),
            album_artist_sort: r.album_artist_sort.clone(),
            year: r.year,
            track_no: r.track_no,
            disc_no: r.disc_no,
            genres: r.genres.clone(),
        }
    }
}

/// Fetch the write-back metadata for many tracks by id (Phase 5b). Chunked under
/// SQLite's bound-variable limit, like [`get_tracks`]; order is unspecified, so
/// the caller pairs rows back to its own list by `track_id`.
pub fn writeback_rows(conn: &Connection, ids: &[i64]) -> Result<Vec<WritebackRow>> {
    const CHUNK: usize = 900;
    let mut out = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT t.id, t.file_path, t.format, t.title, t.track_no, t.disc_no,
                    ta.name AS track_artist, ta.sort_name AS track_artist_sort,
                    al.title AS album, al.year,
                    aa.name AS album_artist, aa.sort_name AS album_artist_sort,
                    (SELECT group_concat(name, '{GENRE_SEP}') FROM
                       (SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                         WHERE tg.track_id = t.id ORDER BY g.name)) AS genres
             FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             LEFT JOIN artists aa ON al.album_artist_id = aa.id
             LEFT JOIN artists ta ON t.artist_id = ta.id
             WHERE t.id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
            let genres: Option<String> = row.get("genres")?;
            let track_no: Option<i64> = row.get("track_no")?;
            let disc_no: Option<i64> = row.get("disc_no")?;
            Ok(WritebackRow {
                track_id: row.get("id")?,
                file_path: row.get("file_path")?,
                format: row.get("format")?,
                title: row.get("title")?,
                track_artist: row.get("track_artist")?,
                track_artist_sort: row.get("track_artist_sort")?,
                album: row.get("album")?,
                album_artist: row.get("album_artist")?,
                album_artist_sort: row.get("album_artist_sort")?,
                year: row.get("year")?,
                track_no: track_no.map(|n| n as u32),
                disc_no: disc_no.map(|n| n as u32),
                genres: genres
                    .map(|g| g.split(GENRE_SEP).map(str::to_string).collect())
                    .unwrap_or_default(),
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
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

/// The display metadata for the currently-playing track (Phase 4c-i, MPRIS):
/// what the engine snapshot's `track_id` resolves to. `length` is the track
/// duration in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub length: Option<f64>,
    /// The album cover's path relative to the library root (Phase 5d), for the
    /// Now-bar thumbnail and MPRIS `mpris:artUrl`. `None` until a cover is on disk.
    pub album_cover_path: Option<String>,
}

/// Resolve a track id to its title / artist / album / length / cover (the MPRIS
/// `Metadata` and Now-bar source). Joins the track artist and album.
pub fn track_metadata(conn: &Connection, track_id: i64) -> Result<Option<NowPlaying>> {
    conn.query_row(
        "SELECT t.title, t.duration, ar.name AS artist, al.title AS album,
                al.cover_path AS album_cover_path
         FROM tracks t
         LEFT JOIN artists ar ON ar.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE t.id = ?1",
        params![track_id],
        |row| {
            Ok(NowPlaying {
                title: row.get("title")?,
                artist: row.get("artist")?,
                album: row.get("album")?,
                length: row.get("duration")?,
                album_cover_path: row.get("album_cover_path")?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// A unified-queue entry with the display fields the queue panel renders
/// (Phase 4b-ii-b). `title`/`artist` come from the joined track (empty/None for
/// a missing or non-track row; episode/book titles arrive at Phases 6/7).
#[derive(Debug, Clone, PartialEq)]
pub struct QueueDisplayRow {
    pub position: i64,
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub title: String,
    pub artist: Option<String>,
}

/// The unified queue in order, with display fields joined in (Phase 4b-ii-b).
pub fn load_queue_display(conn: &Connection) -> Result<Vec<QueueDisplayRow>> {
    let mut stmt = conn.prepare(
        "SELECT q.position, q.kind, q.track_id, t.title, ar.name AS artist
         FROM queue q
         LEFT JOIN tracks t ON t.id = q.track_id
         LEFT JOIN artists ar ON ar.id = t.artist_id
         ORDER BY q.position",
    )?;
    let rows = stmt.query_map([], |row| {
        let kind: String = row.get("kind")?;
        let kind = kind.parse::<MediaKind>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        Ok(QueueDisplayRow {
            position: row.get("position")?,
            kind,
            track_id: row.get("track_id")?,
            title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
            artist: row.get("artist")?,
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

// --- Podcast reads (Phase 6a-i, spec §4.2). The `conservatory-podcasts` plugin
// (Phases 6a-ii+) consumes these through the read pool; writes go through the
// worker (`writes.rs`).

fn row_to_show(row: &rusqlite::Row<'_>) -> rusqlite::Result<Show> {
    let accent: Option<i64> = row.get("accent_rgb")?;
    let last_fetched: Option<i64> = row.get("last_fetched")?;
    Ok(Show {
        id: row.get("id")?,
        slug: row.get("slug")?,
        feed_url: row.get("feed_url")?,
        title: row.get("title")?,
        author: row.get("author")?,
        description: row.get("description")?,
        homepage_url: row.get("homepage_url")?,
        cover_path: row.get("cover_path")?,
        accent_rgb: accent.map(|v| v as u32),
        apple_podcasts_id: row.get("apple_podcasts_id")?,
        last_fetched: epoch_to_dt(last_fetched),
        last_modified: row.get("last_modified")?,
        etag: row.get("etag")?,
        fetch_interval: row.get::<_, i64>("fetch_interval")? as u32,
        auth_user: row.get("auth_user")?,
        auth_pass_ref: row.get("auth_pass_ref")?,
        auto_download: row.get("auto_download")?,
        keep_count: row.get::<_, i64>("keep_count")? as u32,
        priority: row.get::<_, i64>("priority")? as i32,
        folder_path: row.get("folder_path")?,
    })
}

fn row_to_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Episode> {
    let pub_date: Option<i64> = row.get("pub_date")?;
    Ok(Episode {
        id: row.get("id")?,
        show_id: row.get("show_id")?,
        guid: row.get("guid")?,
        title: row.get("title")?,
        description: row.get("description")?,
        pub_date: epoch_to_dt(pub_date),
        duration: row.get::<_, Option<i64>>("duration")?.map(|v| v as u32),
        file_size: row.get::<_, Option<i64>>("file_size")?.map(|v| v as u64),
        audio_url: row.get("audio_url")?,
        audio_path: row.get("audio_path")?,
        folder_path: row.get("folder_path")?,
        mime_type: row.get("mime_type")?,
        season: row.get::<_, Option<i64>>("season")?.map(|v| v as u32),
        episode_number: row
            .get::<_, Option<i64>>("episode_number")?
            .map(|v| v as u32),
        episode_type: row.get("episode_type")?,
    })
}

fn row_to_playback(row: &rusqlite::Row<'_>) -> rusqlite::Result<Playback> {
    let played: i64 = row.get("played")?;
    let played = PlayedState::from_i64(played).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    let last_played: Option<i64> = row.get("last_played")?;
    Ok(Playback {
        episode_id: row.get("episode_id")?,
        position: row.get("position")?,
        played,
        last_played: epoch_to_dt(last_played),
        play_count: row.get::<_, i64>("play_count")? as u32,
        starred: row.get("starred")?,
    })
}

fn row_to_show_settings(row: &rusqlite::Row<'_>) -> rusqlite::Result<ShowSettings> {
    let policy: String = row.get("inbox_policy")?;
    let inbox_policy = policy.parse::<InboxPolicy>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(ShowSettings {
        show_id: row.get("show_id")?,
        playback_speed: row.get("playback_speed")?,
        smart_speed: row.get("smart_speed")?,
        voice_boost: row.get("voice_boost")?,
        skip_intro: row.get::<_, i64>("skip_intro")? as u32,
        skip_outro: row.get::<_, i64>("skip_outro")? as u32,
        skip_forward: row.get::<_, Option<i64>>("skip_forward")?.map(|v| v as u32),
        skip_back: row.get::<_, Option<i64>>("skip_back")?.map(|v| v as u32),
        inbox_policy,
    })
}

fn row_to_chapter(row: &rusqlite::Row<'_>) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: row.get("id")?,
        episode_id: row.get("episode_id")?,
        start_time: row.get("start_time")?,
        end_time: row.get("end_time")?,
        title: row.get("title")?,
        url: row.get("url")?,
        image_path: row.get("image_path")?,
    })
}

pub fn get_show(conn: &Connection, id: i64) -> Result<Option<Show>> {
    conn.query_row(
        "SELECT * FROM shows WHERE id = ?1",
        params![id],
        row_to_show,
    )
    .optional()
    .map_err(Into::into)
}

/// All subscriptions, highest `priority` first (the Overcast ordering), then
/// title (Phase 6a-i).
pub fn list_shows(conn: &Connection) -> Result<Vec<Show>> {
    let mut stmt =
        conn.prepare("SELECT * FROM shows ORDER BY priority DESC, title COLLATE NOCASE")?;
    let rows = stmt.query_map([], row_to_show)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Resolve an episode by its `(show_id, guid)` identity (spec §8), the key the
/// fetch loop dedups against. `None` if not yet seen.
pub fn get_episode_by_guid(conn: &Connection, show_id: i64, guid: &str) -> Result<Option<Episode>> {
    conn.query_row(
        "SELECT * FROM episodes WHERE show_id = ?1 AND guid = ?2",
        params![show_id, guid],
        row_to_episode,
    )
    .optional()
    .map_err(Into::into)
}

/// Resolve an episode by its primary key (the download path, 6a-iii-b).
pub fn get_episode(conn: &Connection, id: i64) -> Result<Option<Episode>> {
    conn.query_row(
        "SELECT * FROM episodes WHERE id = ?1",
        params![id],
        row_to_episode,
    )
    .optional()
    .map_err(Into::into)
}

/// A show's episodes, newest first (the triage list order).
pub fn list_episodes_for_show(conn: &Connection, show_id: i64) -> Result<Vec<Episode>> {
    let mut stmt =
        conn.prepare("SELECT * FROM episodes WHERE show_id = ?1 ORDER BY pub_date DESC, id DESC")?;
    let rows = stmt.query_map(params![show_id], row_to_episode)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// An episode's triage/playback row, or `None` if it has never been touched
/// (an untouched episode is Inbox by default).
pub fn get_playback(conn: &Connection, episode_id: i64) -> Result<Option<Playback>> {
    conn.query_row(
        "SELECT * FROM playback WHERE episode_id = ?1",
        params![episode_id],
        row_to_playback,
    )
    .optional()
    .map_err(Into::into)
}

/// A show's per-show overrides, or `None` if it uses the global defaults.
pub fn get_show_settings(conn: &Connection, show_id: i64) -> Result<Option<ShowSettings>> {
    conn.query_row(
        "SELECT * FROM show_settings WHERE show_id = ?1",
        params![show_id],
        row_to_show_settings,
    )
    .optional()
    .map_err(Into::into)
}

/// An episode's chapters in playback order.
pub fn list_chapters(conn: &Connection, episode_id: i64) -> Result<Vec<Chapter>> {
    let mut stmt =
        conn.prepare("SELECT * FROM chapters WHERE episode_id = ?1 ORDER BY start_time")?;
    let rows = stmt.query_map(params![episode_id], row_to_chapter)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A show's tags, ordered by name (Phase 6a-i).
pub fn list_tags_for_show(conn: &Connection, show_id: i64) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name FROM tags t
         JOIN show_tags st ON st.tag_id = t.id
         WHERE st.show_id = ?1
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![show_id], |r| {
        Ok(Tag {
            id: r.get("id")?,
            name: r.get("name")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One episode as the triage list renders it (Phase 6b-ii-a): the display fields
/// plus the show title and the triage state joined from `playback` (defaulting
/// to Unplayed when there is no playback row) and `queue` membership.
#[derive(Debug, Clone)]
pub struct EpisodeListRow {
    pub id: i64,
    pub show_id: i64,
    pub show_title: String,
    pub title: String,
    pub description: Option<String>,
    pub pub_date: Option<DateTime<Utc>>,
    pub duration: Option<u32>, // seconds
    pub played: PlayedState,
    pub position: f64, // resume cursor, seconds
    pub starred: bool,
    pub in_queue: bool,
}

/// The triage buckets (spec §3.7, §4.2). Derived, not stored: Queue is unified-
/// queue membership, Played is `playback.played >= PlayedFully`, Inbox is the
/// rest (untouched or partially-played episodes not in the queue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageBucket {
    Inbox,
    Queue,
    Played,
}

impl TriageBucket {
    /// Parse the CLI `--bucket` value.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "inbox" => Some(TriageBucket::Inbox),
            "queue" => Some(TriageBucket::Queue),
            "played" => Some(TriageBucket::Played),
            _ => None,
        }
    }

    /// The stable string name (CLI / display).
    pub fn as_str(self) -> &'static str {
        match self {
            TriageBucket::Inbox => "inbox",
            TriageBucket::Queue => "queue",
            TriageBucket::Played => "played",
        }
    }
}

// The shared projection for an `EpisodeListRow`: episode display fields, the
// show title, the played/position/starred state (COALESCEd so an episode with
// no playback row reads as Unplayed at position 0), and the `in_queue` flag.
const EPISODE_LIST_SELECT: &str = "
    SELECT e.id, e.show_id, s.title AS show_title, e.title, e.description,
           e.pub_date, e.duration,
           COALESCE(p.played, 0)    AS played,
           COALESCE(p.position, 0.0) AS position,
           COALESCE(p.starred, 0)   AS starred,
           EXISTS(SELECT 1 FROM queue q WHERE q.kind = 'episode' AND q.episode_id = e.id) AS in_queue
    FROM episodes e
    JOIN shows s ON s.id = e.show_id
    LEFT JOIN playback p ON p.episode_id = e.id";

fn row_to_episode_list_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EpisodeListRow> {
    let pub_date: Option<i64> = row.get("pub_date")?;
    let played: i64 = row.get("played")?;
    let played = PlayedState::from_i64(played).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(e))
    })?;
    Ok(EpisodeListRow {
        id: row.get("id")?,
        show_id: row.get("show_id")?,
        show_title: row.get("show_title")?,
        title: row.get("title")?,
        description: row.get("description")?,
        pub_date: epoch_to_dt(pub_date),
        duration: row.get::<_, Option<i64>>("duration")?.map(|v| v as u32),
        played,
        position: row.get("position")?,
        starred: row.get("starred")?,
        in_queue: row.get("in_queue")?,
    })
}

/// A show's episodes with their triage state, newest first (the triage list).
pub fn episodes_for_show(conn: &Connection, show_id: i64) -> Result<Vec<EpisodeListRow>> {
    let sql =
        format!("{EPISODE_LIST_SELECT} WHERE e.show_id = ?1 ORDER BY e.pub_date DESC, e.id DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![show_id], row_to_episode_list_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// All episodes in a triage bucket, across every subscription (spec §4.2):
/// Queue ordered by queue position, Inbox/Played newest first.
pub fn episodes_in_bucket(conn: &Connection, bucket: TriageBucket) -> Result<Vec<EpisodeListRow>> {
    // PlayedState::PlayedFully = 2; >= 2 also catches ArchivedUnlistened (3).
    let sql = match bucket {
        TriageBucket::Queue => format!(
            "{EPISODE_LIST_SELECT}
             JOIN queue q2 ON q2.kind = 'episode' AND q2.episode_id = e.id
             ORDER BY q2.position"
        ),
        TriageBucket::Played => format!(
            "{EPISODE_LIST_SELECT} WHERE COALESCE(p.played, 0) >= 2 ORDER BY e.pub_date DESC, e.id DESC"
        ),
        TriageBucket::Inbox => format!(
            "{EPISODE_LIST_SELECT}
             WHERE COALESCE(p.played, 0) < 2
               AND NOT EXISTS(SELECT 1 FROM queue q WHERE q.kind = 'episode' AND q.episode_id = e.id)
             ORDER BY e.pub_date DESC, e.id DESC"
        ),
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_episode_list_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}
