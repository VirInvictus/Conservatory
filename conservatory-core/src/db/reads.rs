//! Read helpers. These take a borrowed `&Connection` so they run on a read-only
//! pool handle (`ReadPool::open`), never on the writer (spec §2.1). The CLI and,
//! later, the GTK side consume the `Artist` / `Album` / `Track` models through
//! these. Phase 1b ships counts and basic lookups; richer queries (faceting,
//! search) arrive with `conservatory-search` at Phase 3.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{Album, Artist, Track};
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
    pub shelf_genre: Option<String>,
    pub album_artist_sort: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub title: String,
    pub track_artist: Option<String>,
    pub format: Option<String>,
}

/// Every track with the album/artist fields needed to render its target path.
/// Ordered to mirror the default tree (genre → album artist → album → disc/track)
/// so CLI output and previews read top-down.
pub fn track_render_rows(conn: &Connection) -> Result<Vec<TrackRenderRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.track_no, t.disc_no, t.format,
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
            shelf_genre: row.get("shelf_genre")?,
            album_artist_sort: row.get("album_artist_sort")?,
            album: row.get("album")?,
            year: row.get("year")?,
            track_no: track_no.map(|n| n as u32),
            disc_no: disc_no.map(|n| n as u32),
            title: row.get("title")?,
            track_artist: row.get("track_artist")?,
            format: row.get("format")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
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
