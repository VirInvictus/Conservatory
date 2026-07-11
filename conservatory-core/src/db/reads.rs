//! Read helpers. These take a borrowed `&Connection` so they run on a read-only
//! pool handle (`ReadPool::open`), never on the writer (spec §2.1). The CLI and,
//! later, the GTK side consume the `Artist` / `Album` / `Track` models through
//! these. Phase 1b ships counts and basic lookups; richer queries (faceting,
//! search) arrive with `conservatory-search` at Phase 3.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{
    Album, ApeStripRow, Artist, AudioState, Book, BookChapter, BookPerson, BookPlayback, Chapter,
    CompSettings, DspState, EQ_BAND_COUNT, Episode, EqPreset, EqState, InboxPolicy,
    LevelerSettings, LimiterSettings, MediaKind, ModuleState, PendingScrobble, Perspective,
    Playback, PlayedState, Playlist, PlaylistKind, PlaylistOrder, QueueItem, ResamplerQuality,
    Series, Show, ShowSettings, Tag, Track, VerifyResultRow,
};
use crate::errors::Result;
use crate::verify::VerifyVerdict;

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

/// One track plus the album context the duplicate-detection tiers need (Phase
/// 8b): the album it belongs to (id + managed folder + artist *name* + title),
/// its ordinal/title/format/duration, and its path. Artist names (not sort
/// names) so the normalization keys match what a user reads.
#[derive(Debug, Clone)]
pub struct DedupRow {
    pub album_id: Option<i64>,
    pub album_folder: Option<String>,
    pub album_artist: Option<String>,
    pub album_title: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub title: String,
    pub track_artist: Option<String>,
    pub format: Option<String>,
    pub duration: Option<f64>,
    pub file_path: String,
}

/// Every track with its album context, for the `duplicates` report (Phase 8b).
/// Read-only; the four tiers all derive from this one denormalized pass.
pub fn dedup_rows(conn: &Connection) -> Result<Vec<DedupRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.album_id, t.title, t.track_no, t.disc_no, t.format, t.duration, t.file_path,
                al.title AS album_title, al.folder_path AS album_folder,
                aa.name AS album_artist,
                ta.name AS track_artist
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         ORDER BY al.folder_path, t.disc_no, t.track_no",
    )?;
    let rows = stmt.query_map([], |row| {
        let track_no: Option<i64> = row.get("track_no")?;
        let disc_no: Option<i64> = row.get("disc_no")?;
        Ok(DedupRow {
            album_id: row.get("album_id")?,
            album_folder: row.get("album_folder")?,
            album_artist: row.get("album_artist")?,
            album_title: row.get("album_title")?,
            track_no: track_no.map(|n| n as u32),
            disc_no: disc_no.map(|n| n as u32),
            title: row.get("title")?,
            track_artist: row.get("track_artist")?,
            format: row.get("format")?,
            duration: row.get("duration")?,
            file_path: row.get("file_path")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One track with the fields a playlist export needs (Phase 8d): its id (to
/// intersect with a resolved selector), the display `artist` (track artist
/// falling back to album artist), `title`, `album`, `duration` (for `#EXTINF`),
/// and the root-relative `file_path`.
#[derive(Debug, Clone)]
pub struct PlaylistRow {
    pub track_id: i64,
    pub artist: Option<String>,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub file_path: String,
}

/// Every track with its playlist fields, ordered album-artist / album / disc /
/// track so an exported `.m3u` reads in a sensible album order (Phase 8d). The
/// caller intersects with the resolved selector id set, preserving this order.
pub fn playlist_rows(conn: &Connection) -> Result<Vec<PlaylistRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.duration, t.file_path,
                al.title AS album,
                COALESCE(ta.name, aa.name) AS artist
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         ORDER BY aa.sort_name, al.title, t.disc_no, t.track_no",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PlaylistRow {
            track_id: row.get("id")?,
            artist: row.get("artist")?,
            title: row.get("title")?,
            album: row.get("album")?,
            duration: row.get("duration")?,
            file_path: row.get("file_path")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The id of the track at a given root-relative `file_path`, or `None` if no
/// managed track lives there (Phase 8d playlist import; the library is
/// database-canonical, so paths are matched exactly against `tracks.file_path`).
pub fn track_id_by_path(conn: &Connection, file_path: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM tracks WHERE file_path = ?1",
            params![file_path],
            |r| r.get(0),
        )
        .optional()?)
}

/// One track with the fields the health audits (Phase 8c) inspect: presence of
/// the critical tags, the bitrate, and the ReplayGain columns. `artist` is the
/// track artist falling back to the album artist; `genre_count` is the number
/// of raw `track_genres` rows (0 = missing genre).
#[derive(Debug, Clone)]
pub struct AuditTrackRow {
    pub track_id: i64,
    pub album_id: Option<i64>,
    pub title: String,
    pub artist: Option<String>,
    pub track_no: Option<u32>,
    pub genre_count: i64,
    pub format: Option<String>,
    pub bitrate: Option<u32>,
    pub replaygain_track: Option<f64>,
    pub replaygain_album: Option<f64>,
    pub file_path: String,
}

/// Every track with its audit fields (Phase 8c `audit`). Read-only.
pub fn audit_track_rows(conn: &Connection) -> Result<Vec<AuditTrackRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.album_id, t.title, t.track_no, t.format, t.bitrate,
                t.replaygain_track, t.replaygain_album, t.file_path,
                COALESCE(ta.name, aa.name) AS artist,
                (SELECT COUNT(*) FROM track_genres WHERE track_id = t.id) AS genre_count
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         ORDER BY al.folder_path, t.disc_no, t.track_no",
    )?;
    let rows = stmt.query_map([], |row| {
        let track_no: Option<i64> = row.get("track_no")?;
        let bitrate: Option<i64> = row.get("bitrate")?;
        Ok(AuditTrackRow {
            track_id: row.get("id")?,
            album_id: row.get("album_id")?,
            title: row.get("title")?,
            artist: row.get("artist")?,
            track_no: track_no.map(|n| n as u32),
            genre_count: row.get("genre_count")?,
            format: row.get("format")?,
            bitrate: bitrate.map(|n| n as u32),
            replaygain_track: row.get("replaygain_track")?,
            replaygain_album: row.get("replaygain_album")?,
            file_path: row.get("file_path")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One album with the fields the cover-art audits (Phase 8c) inspect.
#[derive(Debug, Clone)]
pub struct AuditAlbumRow {
    pub album_id: i64,
    pub artist: Option<String>,
    pub title: String,
    pub cover_path: Option<String>,
    pub folder_path: String,
}

/// Every album with its cover fields (Phase 8c `audit` art tiers). Read-only.
pub fn audit_album_rows(conn: &Connection) -> Result<Vec<AuditAlbumRow>> {
    let mut stmt = conn.prepare(
        "SELECT al.id, al.title, al.cover_path, al.folder_path, aa.name AS artist
         FROM albums al
         LEFT JOIN artists aa ON al.album_artist_id = aa.id
         ORDER BY al.folder_path",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AuditAlbumRow {
            album_id: row.get("id")?,
            artist: row.get("artist")?,
            title: row.get("title")?,
            cover_path: row.get("cover_path")?,
            folder_path: row.get("folder_path")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One track with the fields the library-statistics report (Phase 8c-ii)
/// aggregates: format / bitrate / duration / rating / path, plus the
/// fully-tagged inputs (title, artist, track number, genre count). `artist` is
/// the track artist falling back to the album artist.
#[derive(Debug, Clone)]
pub struct StatsTrackRow {
    pub format: Option<String>,
    pub bitrate: Option<u32>,
    pub duration: Option<f64>,
    pub rating: i64,
    pub file_path: String,
    pub title: String,
    pub artist: Option<String>,
    pub track_no: Option<u32>,
    pub genre_count: i64,
}

/// Every track with its statistics fields (Phase 8c-ii `stats`). Read-only.
pub fn stats_track_rows(conn: &Connection) -> Result<Vec<StatsTrackRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.format, t.bitrate, t.duration, t.rating, t.file_path, t.title, t.track_no,
                COALESCE(ta.name, aa.name) AS artist,
                (SELECT COUNT(*) FROM track_genres WHERE track_id = t.id) AS genre_count
         FROM tracks t
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists ta ON t.artist_id = ta.id
         LEFT JOIN artists aa ON al.album_artist_id = aa.id",
    )?;
    let rows = stmt.query_map([], |row| {
        let bitrate: Option<i64> = row.get("bitrate")?;
        let track_no: Option<i64> = row.get("track_no")?;
        Ok(StatsTrackRow {
            format: row.get("format")?,
            bitrate: bitrate.map(|n| n as u32),
            duration: row.get("duration")?,
            rating: row.get("rating")?,
            file_path: row.get("file_path")?,
            title: row.get("title")?,
            artist: row.get("artist")?,
            track_no: track_no.map(|n| n as u32),
            genre_count: row.get("genre_count")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One `(track, genre)` pairing with that track's rating, for the genre
/// distribution and the per-genre rating tally (Phase 8c-ii). A track with N
/// genres yields N rows.
#[derive(Debug, Clone)]
pub struct StatsGenreRow {
    pub genre: String,
    pub rating: i64,
}

/// Every track-genre pairing with its rating (Phase 8c-ii `stats`). Read-only.
pub fn stats_genre_rows(conn: &Connection) -> Result<Vec<StatsGenreRow>> {
    let mut stmt = conn.prepare(
        "SELECT g.name AS genre, t.rating AS rating
         FROM track_genres tg
         JOIN genres g ON tg.genre_id = g.id
         JOIN tracks t ON tg.track_id = t.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(StatsGenreRow {
            genre: row.get("genre")?,
            rating: row.get("rating")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
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
/// never played anything. The cursor carries its `kind` (Phase 6b-ii-c-2) so a
/// restart reopens an episode, not just the last track; `track_id` is set when
/// `kind == Track`, `episode_id` when `kind == Episode`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackStateRow {
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub episode_id: Option<i64>,
    /// The book the cursor points at when `kind = Audiobook` (Phase 7c-ii).
    pub book_id: Option<i64>,
    pub position: f64,
    pub paused: bool,
    pub volume: i64,
    pub updated_at: Option<i64>,
}

/// Read the saved playback cursor, if any (the row with id = 1).
pub fn read_playback_state(conn: &Connection) -> Result<Option<PlaybackStateRow>> {
    conn.query_row(
        "SELECT kind, track_id, episode_id, book_id, position, paused, volume, updated_at
         FROM playback_state WHERE id = 1",
        [],
        |row| {
            let kind: String = row.get("kind")?;
            let kind = kind.parse::<MediaKind>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(PlaybackStateRow {
                kind,
                track_id: row.get("track_id")?,
                episode_id: row.get("episode_id")?,
                book_id: row.get("book_id")?,
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
    /// The album / show / book extracted accent (packed `0x00RRGGBB`), for tinting
    /// the Now-bar cover frame and seek fill (Phase 12c). `None` when unextracted.
    pub album_accent_rgb: Option<u32>,
}

/// Resolve a track id to its title / artist / album / length / cover (the MPRIS
/// `Metadata` and Now-bar source). Joins the track artist and album.
pub fn track_metadata(conn: &Connection, track_id: i64) -> Result<Option<NowPlaying>> {
    conn.query_row(
        "SELECT t.title, t.duration, ar.name AS artist, al.title AS album,
                al.cover_path AS album_cover_path, al.accent_rgb AS album_accent_rgb
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
                album_accent_rgb: row
                    .get::<_, Option<i64>>("album_accent_rgb")?
                    .map(|v| v as u32),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// Resolve an episode id to the same [`NowPlaying`] shape as a track (v0.0.38),
/// so the Now-bar can show a playing episode without a stale music title/cover.
/// The episode title is the title, the show title stands in for the artist, and
/// the **show**'s cover (episodes have no per-episode art) is the cover. Joins
/// `episodes`→`shows`. `shows.cover_path` is usually `None` today (show artwork
/// is not fetched yet), in which case the Now-bar shows its placeholder rather
/// than the previous track's cover.
pub fn episode_metadata(conn: &Connection, episode_id: i64) -> Result<Option<NowPlaying>> {
    conn.query_row(
        "SELECT e.title, e.duration, s.title AS show_title, s.cover_path AS cover_path,
                s.accent_rgb AS album_accent_rgb
         FROM episodes e
         JOIN shows s ON s.id = e.show_id
         WHERE e.id = ?1",
        params![episode_id],
        |row| {
            Ok(NowPlaying {
                title: row.get("title")?,
                artist: row.get::<_, Option<String>>("show_title")?,
                album: row.get::<_, Option<String>>("show_title")?,
                length: row.get::<_, Option<i64>>("duration")?.map(|d| d as f64),
                album_cover_path: row.get("cover_path")?,
                album_accent_rgb: row
                    .get::<_, Option<i64>>("album_accent_rgb")?
                    .map(|v| v as u32),
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
    pub episode_id: Option<i64>,
    /// The book id for an audiobook row (Phase 7c-iii), so a resume can rebuild
    /// the book item. `None` for a track / episode row.
    pub book_id: Option<i64>,
    /// The episode's show (Phase 6b-ii-c-3-a), so a resume can resolve the
    /// per-show playback speed. `None` for a track row.
    pub show_id: Option<i64>,
    pub title: String,
    pub artist: Option<String>,
    /// Episode source (Phase 6b-ii-c-2), so a resume can rebuild episode items
    /// without a second read: the downloaded file (relative to the root) else
    /// the stream URL. Both `None` for a track row or an undownloaded+URL-less
    /// episode.
    pub audio_path: Option<String>,
    pub audio_url: Option<String>,
}

/// The unified queue in order, with display fields joined in (Phase 4b-ii-b;
/// episodes joined at 6b-ii-c). Title/artist coalesce across the kind: a track's
/// artist, an episode's show. Episode audio source is carried for resume.
pub fn load_queue_display(conn: &Connection) -> Result<Vec<QueueDisplayRow>> {
    let mut stmt = conn.prepare(
        "SELECT q.position, q.kind, q.track_id, q.episode_id, q.book_id,
                e.show_id AS show_id,
                COALESCE(t.title, e.title, bk.title) AS title,
                COALESCE(ar.name, s.title,
                  (SELECT p.name FROM book_authors ba JOIN book_people p ON p.id = ba.person_id
                    WHERE ba.book_id = bk.id ORDER BY p.sort_name LIMIT 1)) AS artist,
                e.audio_path AS audio_path,
                e.audio_url  AS audio_url
         FROM queue q
         LEFT JOIN tracks t ON t.id = q.track_id
         LEFT JOIN artists ar ON ar.id = t.artist_id
         LEFT JOIN episodes e ON e.id = q.episode_id
         LEFT JOIN shows s ON s.id = e.show_id
         LEFT JOIN books bk ON bk.id = q.book_id
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
            episode_id: row.get("episode_id")?,
            book_id: row.get("book_id")?,
            show_id: row.get("show_id")?,
            title: row.get::<_, Option<String>>("title")?.unwrap_or_default(),
            artist: row.get("artist")?,
            audio_path: row.get("audio_path")?,
            audio_url: row.get("audio_url")?,
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

// --- Playlists (Phase 16d) ---

fn row_to_playlist(r: &rusqlite::Row) -> rusqlite::Result<Playlist> {
    let kind_s: String = r.get(2)?;
    let order_s: Option<String> = r.get(5)?;
    Ok(Playlist {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: PlaylistKind::parse(&kind_s).unwrap_or(PlaylistKind::Static),
        query: r.get(3)?,
        limit_n: r.get(4)?,
        order_by: order_s.and_then(|s| PlaylistOrder::parse(&s)),
        created_at: r.get(6)?,
    })
}

/// Every playlist, newest first (Phase 16d).
pub fn list_playlists(conn: &Connection) -> Result<Vec<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, query, limit_n, order_by, created_at \
         FROM playlists ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt.query_map([], row_to_playlist)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// One playlist by id (Phase 16d).
pub fn get_playlist(conn: &Connection, id: i64) -> Result<Option<Playlist>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, query, limit_n, order_by, created_at FROM playlists WHERE id = ?1",
    )?;
    stmt.query_row([id], row_to_playlist)
        .optional()
        .map_err(Into::into)
}

/// A static playlist's track ids in position order (Phase 16d). Episode / book
/// entries are skipped here (v1 materialises tracks; the columns exist for later).
pub fn static_playlist_track_ids(conn: &Connection, playlist_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT track_id FROM playlist_entries \
         WHERE playlist_id = ?1 AND track_id IS NOT NULL ORDER BY position",
    )?;
    let rows = stmt.query_map([playlist_id], |r| r.get(0))?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The smart-playlist SQL path (Phase 16d): `SELECT id FROM tracks WHERE
/// <where_sql> ORDER BY <order> LIMIT <n>`. `where_sql` + `params` come from the
/// caller's `conservatory_search::try_translate` (core stays search-free at
/// runtime); `order` is a fixed whitelist fragment and `limit` an integer, so
/// there is no injection surface. The eval fallback (regex / fuzzy queries that
/// do not translate) is the caller's concern, since it needs the grammar.
pub fn ordered_track_ids(
    conn: &Connection,
    where_sql: &str,
    params: &[SqlParam],
    order: PlaylistOrder,
    limit: Option<i64>,
) -> Result<Vec<i64>> {
    let limit_sql = match limit {
        Some(n) if n >= 0 => format!(" LIMIT {n}"),
        _ => String::new(),
    };
    let sql = format!(
        "SELECT id FROM tracks WHERE {where_sql} ORDER BY {}{limit_sql}",
        order_sql(order),
    );
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

/// The `ORDER BY` fragment for a smart-playlist order. A fixed whitelist (no user
/// input), safe to interpolate; `id` is the stable tiebreak. Least-recently-played
/// sorts never-played (NULL `last_played`) first.
fn order_sql(order: PlaylistOrder) -> &'static str {
    match order {
        PlaylistOrder::Added => "added_at DESC, id ASC",
        PlaylistOrder::Rating => "rating DESC, added_at DESC, id ASC",
        PlaylistOrder::LastPlayed => "last_played ASC, added_at DESC, id ASC",
        PlaylistOrder::Title => "title COLLATE NOCASE ASC, id ASC",
        PlaylistOrder::Artist => {
            "(SELECT name FROM artists WHERE id = tracks.artist_id) COLLATE NOCASE ASC, id ASC"
        }
    }
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

/// Aggregate listening totals across every `listening_sessions` row (Phase
/// 6c-ii): the session count and summed real / audio / Smart-Speed-saved seconds,
/// for the `podcast stats` surface. An empty table sums to zero (the `COALESCE`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ListeningTotals {
    pub sessions: i64,
    pub real_seconds: f64,
    pub audio_seconds: f64,
    pub smart_speed_saved: f64,
}

pub fn listening_totals(conn: &Connection) -> Result<ListeningTotals> {
    conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(real_seconds), 0),
                COALESCE(SUM(audio_seconds), 0),
                COALESCE(SUM(smart_speed_saved), 0)
         FROM listening_sessions",
        [],
        |r| {
            Ok(ListeningTotals {
                sessions: r.get(0)?,
                real_seconds: r.get(1)?,
                audio_seconds: r.get(2)?,
                smart_speed_saved: r.get(3)?,
            })
        },
    )
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

/// Per-show overrides for many shows at once, keyed by `show_id` (Phase
/// 6b-ii-c-3-a): the episode-queue builders look up each episode's show speed
/// here. Shows with no stored overrides are simply absent from the map (the
/// caller treats absence as the default profile). Distinct ids are queried one
/// by one through the read pool; the show set per queue is small.
pub fn show_settings_map(
    conn: &Connection,
    show_ids: &[i64],
) -> Result<HashMap<i64, ShowSettings>> {
    let mut out = HashMap::new();
    for &id in show_ids {
        if let Some(s) = get_show_settings(conn, id)? {
            out.insert(id, s);
        }
    }
    Ok(out)
}

/// An episode's chapters in playback order.
pub fn list_chapters(conn: &Connection, episode_id: i64) -> Result<Vec<Chapter>> {
    let mut stmt =
        conn.prepare("SELECT * FROM chapters WHERE episode_id = ?1 ORDER BY start_time")?;
    let rows = stmt.query_map(params![episode_id], row_to_chapter)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Outbox rows ready to submit (Phase 9a): `next_attempt_at <= now`, oldest
/// first, capped at `limit`. The drain loop submits these and then deletes (on
/// success) or bumps the attempt (on failure) through the writer.
pub fn pending_scrobbles(conn: &Connection, now: i64, limit: i64) -> Result<Vec<PendingScrobble>> {
    let mut stmt = conn.prepare(
        "SELECT id, service, kind, listened_at, artist, track, album, track_number,
                duration_secs, recording_mbid, attempts
         FROM scrobble_outbox
         WHERE next_attempt_at <= ?1
         ORDER BY id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![now, limit], |r| {
        Ok(PendingScrobble {
            id: r.get("id")?,
            service: r.get("service")?,
            kind: r.get("kind")?,
            listened_at: r.get("listened_at")?,
            artist: r.get("artist")?,
            track: r.get("track")?,
            album: r.get("album")?,
            track_number: r.get("track_number")?,
            duration_secs: r.get("duration_secs")?,
            recording_mbid: r.get("recording_mbid")?,
            attempts: r.get("attempts")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Total queued listens (Phase 9a), for the `scrobble status` CLI verb.
pub fn count_pending_scrobbles(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM scrobble_outbox", [], |r| r.get(0))?)
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
    /// Downloaded local file (relative to the library root), or `None`.
    pub audio_path: Option<String>,
    /// The remote enclosure URL (for streaming when not downloaded).
    pub audio_url: Option<String>,
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

/// A sortable episode-list column (16.5d). The GTK `ColumnView` header drives
/// this; the comparator lives here so it is testable headless (the CLAUDE.md
/// rule, the `TrackSort`/`cmp_tracks` twin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpisodeSort {
    Title,
    Date,
    Length,
}

/// Pairwise comparison of two episode rows by `key`. Text is case-insensitive;
/// a missing date or duration sorts first ascending. Direction inversion is the
/// caller's (the `ColumnView` toggles its own sort order).
pub fn cmp_episodes(
    a: &EpisodeListRow,
    b: &EpisodeListRow,
    key: EpisodeSort,
) -> std::cmp::Ordering {
    match key {
        EpisodeSort::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        EpisodeSort::Date => a.pub_date.cmp(&b.pub_date),
        EpisodeSort::Length => a.duration.unwrap_or(0).cmp(&b.duration.unwrap_or(0)),
    }
}

/// Sidebar badge counts (16.5d): triage-bucket totals plus each show's
/// unfinished count (unplayed or in progress, the Inbox notion of "not done").
/// The bucket definitions mirror [`episodes_in_bucket`] exactly.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PodcastSidebarCounts {
    pub inbox: i64,
    pub queue: i64,
    pub played: i64,
    pub unplayed_by_show: HashMap<i64, i64>,
}

pub fn podcast_sidebar_counts(conn: &Connection) -> Result<PodcastSidebarCounts> {
    let inbox = conn.query_row(
        "SELECT COUNT(*) FROM episodes e
         LEFT JOIN playback p ON p.episode_id = e.id
         WHERE COALESCE(p.played, 0) < 2
           AND NOT EXISTS(SELECT 1 FROM queue q WHERE q.kind = 'episode' AND q.episode_id = e.id)",
        [],
        |r| r.get(0),
    )?;
    let queue = conn.query_row(
        "SELECT COUNT(*) FROM queue WHERE kind = 'episode'",
        [],
        |r| r.get(0),
    )?;
    let played = conn.query_row(
        "SELECT COUNT(*) FROM episodes e
         LEFT JOIN playback p ON p.episode_id = e.id
         WHERE COALESCE(p.played, 0) >= 2",
        [],
        |r| r.get(0),
    )?;
    let mut unplayed_by_show = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT e.show_id, COUNT(*) FROM episodes e
         LEFT JOIN playback p ON p.episode_id = e.id
         WHERE COALESCE(p.played, 0) < 2
         GROUP BY e.show_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (show, n) = row?;
        unplayed_by_show.insert(show, n);
    }
    Ok(PodcastSidebarCounts {
        inbox,
        queue,
        played,
        unplayed_by_show,
    })
}

// The shared projection for an `EpisodeListRow`: episode display fields, the
// show title, the played/position/starred state (COALESCEd so an episode with
// no playback row reads as Unplayed at position 0), and the `in_queue` flag.
const EPISODE_LIST_SELECT: &str = "
    SELECT e.id, e.show_id, s.title AS show_title, e.title, e.description,
           e.pub_date, e.duration, e.audio_path, e.audio_url,
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
        audio_path: row.get("audio_path")?,
        audio_url: row.get("audio_url")?,
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
///
/// Queue and Played can **overlap** by design (a played episode you re-queued
/// shows in both): Queue is pure queue membership and Played is `played >= 2`,
/// neither filters on the other. Only **Inbox** is exclusive: it is everything
/// that is neither played nor queued. The three are not a strict partition.
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

/// Every tag, name-ordered (the Podcasts sidebar Tags section, Phase 6b-ii-b).
pub fn list_all_tags(conn: &Connection) -> Result<Vec<Tag>> {
    let mut stmt = conn.prepare("SELECT id, name FROM tags ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |r| {
        Ok(Tag {
            id: r.get("id")?,
            name: r.get("name")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Episodes of every show carrying `tag_id`, with triage state, newest first.
pub fn episodes_for_tag(conn: &Connection, tag_id: i64) -> Result<Vec<EpisodeListRow>> {
    let sql = format!(
        "{EPISODE_LIST_SELECT}
         JOIN show_tags st ON st.show_id = e.show_id AND st.tag_id = ?1
         ORDER BY e.pub_date DESC, e.id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![tag_id], row_to_episode_list_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The active equalizer state (Phase 5.5b): the singleton `eq_state` row. The
/// migration seeds it to Flat, so a managed DB always has a row; a missing row
/// (impossible post-migration) falls back to flat rather than erroring.
pub fn get_eq_state(conn: &Connection) -> Result<EqState> {
    let row = conn
        .query_row(
            "SELECT preset_name, bands FROM eq_state WHERE id = 0",
            [],
            |row| {
                let preset_name: Option<String> = row.get("preset_name")?;
                let bands: String = row.get("bands")?;
                Ok((preset_name, bands))
            },
        )
        .optional()?;
    Ok(match row {
        Some((preset, bands)) => EqState {
            bands: EqState::parse_bands(&bands),
            preset,
        },
        None => EqState::flat(),
    })
}

/// Every named EQ preset, alphabetical (`Flat` is seeded by the migration).
pub fn list_eq_presets(conn: &Connection) -> Result<Vec<EqPreset>> {
    let mut stmt = conn.prepare("SELECT name, bands FROM eq_presets ORDER BY name")?;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get("name")?;
        let bands: String = row.get("bands")?;
        Ok(EqPreset {
            name,
            bands: EqState::parse_bands(&bands),
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A named preset's band gains, or `None` if no preset by that name.
pub fn get_eq_preset(conn: &Connection, name: &str) -> Result<Option<[f64; EQ_BAND_COUNT]>> {
    conn.query_row(
        "SELECT bands FROM eq_presets WHERE name = ?1",
        params![name],
        |row| row.get::<_, String>("bands"),
    )
    .optional()
    .map(|opt| opt.map(|csv| EqState::parse_bands(&csv)))
    .map_err(Into::into)
}

/// The active audio configuration (Phase 5.5c): the singleton `audio_state` row.
/// The migration seeds it, so a managed DB always has a row; a missing row
/// (impossible post-migration) and an unrecognized stored resampler value both
/// fall back to the defaults rather than erroring (the `get_eq_state` forgiving
/// stance).
pub fn get_audio_state(conn: &Connection) -> Result<AudioState> {
    let state = conn
        .query_row("SELECT * FROM audio_state WHERE id = 0", [], |row| {
            let resampler_raw: String = row.get("resampler_quality")?;
            Ok(AudioState {
                replaygain_mode: row.get("replaygain_mode")?,
                replaygain_preamp: row.get("replaygain_preamp")?,
                replaygain_clip: row.get("replaygain_clip")?,
                gapless: row.get("gapless")?,
                dsp: DspState {
                    comp: ModuleState {
                        enabled: row.get("comp_enabled")?,
                        settings: CompSettings {
                            threshold_db: row.get("comp_threshold_db")?,
                            ratio: row.get("comp_ratio")?,
                            attack_ms: row.get("comp_attack_ms")?,
                            release_ms: row.get("comp_release_ms")?,
                        },
                    },
                    limiter: ModuleState {
                        enabled: row.get("limiter_enabled")?,
                        settings: LimiterSettings {
                            ceiling_db: row.get("limiter_ceiling_db")?,
                        },
                    },
                    leveler: ModuleState {
                        enabled: row.get("leveler_enabled")?,
                        settings: LevelerSettings {
                            target_peak: row.get("leveler_target_peak")?,
                            gausssize: row.get("leveler_gausssize")?,
                        },
                    },
                },
                output_backend: row.get("output_backend")?,
                // Degrade an unrecognized stored value to the default.
                resampler: resampler_raw.parse().unwrap_or(ResamplerQuality::Default),
                smart_speed_level: row.get("smart_speed_level")?,
                repeat: row.get("repeat")?,
                shuffle: row.get("shuffle")?,
            })
        })
        .optional()?;
    Ok(state.unwrap_or_default())
}

// --- Audiobooks (spec §4.5, Phase 7a-i) ---------------------------------------

fn row_to_book(row: &rusqlite::Row<'_>) -> rusqlite::Result<Book> {
    let accent: Option<i64> = row.get("accent_rgb")?;
    let added_at: Option<i64> = row.get("added_at")?;
    Ok(Book {
        id: row.get("id")?,
        title: row.get("title")?,
        subtitle: row.get("subtitle")?,
        series_id: row.get("series_id")?,
        series_sequence: row.get("series_sequence")?,
        year: row.get::<_, Option<i64>>("year")?.map(|v| v as i32),
        publisher: row.get("publisher")?,
        isbn: row.get("isbn")?,
        asin: row.get("asin")?,
        description: row.get("description")?,
        language: row.get("language")?,
        shelf_genre: row.get("shelf_genre")?,
        cover_path: row.get("cover_path")?,
        accent_rgb: accent.map(|v| v as u32),
        folder_path: row.get("folder_path")?,
        rating: row.get::<_, i64>("rating")? as u8,
        starred: row.get("starred")?,
        added_at: epoch_to_dt(added_at),
    })
}

fn row_to_book_person(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookPerson> {
    Ok(BookPerson {
        id: row.get("id")?,
        name: row.get("name")?,
        sort_name: row.get("sort_name")?,
    })
}

fn row_to_book_chapter(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookChapter> {
    Ok(BookChapter {
        id: row.get("id")?,
        book_id: row.get("book_id")?,
        idx: row.get("idx")?,
        title: row.get("title")?,
        file_path: row.get("file_path")?,
        file_offset: row.get("file_offset")?,
        duration: row.get("duration")?,
    })
}

fn row_to_book_playback(row: &rusqlite::Row<'_>) -> rusqlite::Result<BookPlayback> {
    let last_played: Option<i64> = row.get("last_played")?;
    Ok(BookPlayback {
        book_id: row.get("book_id")?,
        position: row.get("position")?,
        finished: row.get("finished")?,
        last_played: epoch_to_dt(last_played),
        speed: row.get("speed")?,
        smart_speed: row.get::<_, Option<bool>>("smart_speed")?,
        voice_boost: row.get::<_, Option<bool>>("voice_boost")?,
    })
}

/// One book by id.
pub fn get_book(conn: &Connection, id: i64) -> Result<Option<Book>> {
    conn.query_row(
        "SELECT * FROM books WHERE id = ?1",
        params![id],
        row_to_book,
    )
    .optional()
    .map_err(Into::into)
}

/// All books, newest first (the shelf surface in Phase 7b reorders by progress).
pub fn list_books(conn: &Connection) -> Result<Vec<Book>> {
    let mut stmt = conn.prepare("SELECT * FROM books ORDER BY added_at DESC, id DESC")?;
    let rows = stmt.query_map([], row_to_book)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A book's authors (role-tagged link), in sort order.
pub fn book_authors(conn: &Connection, book_id: i64) -> Result<Vec<BookPerson>> {
    let mut stmt = conn.prepare(
        "SELECT p.* FROM book_people p
         JOIN book_authors ba ON ba.person_id = p.id
         WHERE ba.book_id = ?1
         ORDER BY p.sort_name",
    )?;
    let rows = stmt.query_map(params![book_id], row_to_book_person)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A book's narrators (role-tagged link), in sort order.
pub fn book_narrators(conn: &Connection, book_id: i64) -> Result<Vec<BookPerson>> {
    let mut stmt = conn.prepare(
        "SELECT p.* FROM book_people p
         JOIN book_narrators bn ON bn.person_id = p.id
         WHERE bn.book_id = ?1
         ORDER BY p.sort_name",
    )?;
    let rows = stmt.query_map(params![book_id], row_to_book_person)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// A book's ordered chapters (spec §4.5).
pub fn book_chapters(conn: &Connection, book_id: i64) -> Result<Vec<BookChapter>> {
    let mut stmt = conn.prepare("SELECT * FROM book_chapters WHERE book_id = ?1 ORDER BY idx")?;
    let rows = stmt.query_map(params![book_id], row_to_book_chapter)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The series a book belongs to, if any.
pub fn series_for_book(conn: &Connection, book_id: i64) -> Result<Option<Series>> {
    conn.query_row(
        "SELECT s.* FROM series s
         JOIN books b ON b.series_id = s.id
         WHERE b.id = ?1",
        params![book_id],
        |row| {
            Ok(Series {
                id: row.get("id")?,
                name: row.get("name")?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// A book's resume row, if it has been played (spec §6.4).
pub fn get_book_playback(conn: &Connection, book_id: i64) -> Result<Option<BookPlayback>> {
    conn.query_row(
        "SELECT * FROM book_playback WHERE book_id = ?1",
        params![book_id],
        row_to_book_playback,
    )
    .optional()
    .map_err(Into::into)
}

/// The currently-playing book's metadata for the Now-bar / Now Playing surface /
/// MPRIS (Phase 7c-iii, the audiobook analogue of [`episode_metadata`] /
/// `track_metadata`): the title; the first author (by `sort_name`, the same
/// folder convention) as `artist`; the series as `album`; the total duration
/// (the sum of the chapters); and the book cover for the thumbnail / `artUrl`.
pub fn book_metadata(conn: &Connection, book_id: i64) -> Result<Option<NowPlaying>> {
    conn.query_row(
        "SELECT b.title,
                (SELECT p.name FROM book_authors ba JOIN book_people p ON p.id = ba.person_id
                  WHERE ba.book_id = b.id ORDER BY p.sort_name LIMIT 1) AS author,
                (SELECT s.name FROM series s WHERE s.id = b.series_id) AS series,
                (SELECT COALESCE(SUM(c.duration), 0) FROM book_chapters c
                  WHERE c.book_id = b.id) AS dur,
                b.cover_path, b.accent_rgb AS album_accent_rgb
         FROM books b WHERE b.id = ?1",
        params![book_id],
        |row| {
            let dur: f64 = row.get("dur")?;
            Ok(NowPlaying {
                title: row.get("title")?,
                artist: row.get::<_, Option<String>>("author")?,
                album: row.get::<_, Option<String>>("series")?,
                length: (dur > 0.0).then_some(dur),
                album_cover_path: row.get("cover_path")?,
                album_accent_rgb: row
                    .get::<_, Option<i64>>("album_accent_rgb")?
                    .map(|v| v as u32),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

/// A book's listening state, derived (not stored) from its `book_playback` row
/// (spec §3.8): `New` when never started, `InProgress` once there is a position,
/// `Finished` when played through. The shelf surfaces in-progress books first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookState {
    New,
    InProgress,
    Finished,
}

impl BookState {
    /// Derive the state from the resume cursor: `finished` wins, then any
    /// position means in-progress, else new (the `book_playback` analogue of the
    /// podcast triage derivation).
    pub fn derive(position: f64, finished: bool) -> Self {
        if finished {
            BookState::Finished
        } else if position > 0.0 {
            BookState::InProgress
        } else {
            BookState::New
        }
    }

    /// The stable string name (CLI / display).
    pub fn as_str(self) -> &'static str {
        match self {
            BookState::New => "new",
            BookState::InProgress => "in_progress",
            BookState::Finished => "finished",
        }
    }
}

/// One book as the shelf renders it (Phase 7b-i): the display fields plus the
/// denormalized author/narrator credits, the series name, and the resume state
/// joined from `book_playback` (COALESCEd, so an unplayed book reads as New at
/// position 0). The author/narrator strings are `group_concat`ed in sort order,
/// the same denormalization the `book_fts` triggers do (spec §4.4). One read for
/// the whole shelf, the `EpisodeListRow` precedent (no per-book N+1).
#[derive(Debug, Clone)]
pub struct BookListRow {
    pub id: i64,
    pub title: String,
    pub subtitle: Option<String>,
    pub author_display: Option<String>,
    pub narrator_display: Option<String>,
    pub series_name: Option<String>,
    pub series_sequence: Option<f64>,
    pub year: Option<i32>,
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>,
    pub rating: u8,
    pub starred: bool,
    pub position: f64,
    pub finished: bool,
    pub last_played: Option<i64>, // unix seconds, for recency ordering
    pub total_duration: f64,      // sum of chapter durations, seconds
}

impl BookListRow {
    /// The derived listening state (spec §3.8).
    pub fn state(&self) -> BookState {
        BookState::derive(self.position, self.finished)
    }
}

/// Every book as a shelf row, in a stable base order (series, then sequence, then
/// title). The UI / [`sort_shelf`] reorders by state so in-progress books surface
/// first; keeping the SQL order stable means the ordering rule lives in one tested
/// place.
pub fn list_book_rows(conn: &Connection) -> Result<Vec<BookListRow>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.title, b.subtitle, b.year, b.cover_path, b.accent_rgb,
                b.rating, b.starred, b.series_sequence,
                sr.name AS series_name,
                COALESCE(bp.position, 0.0) AS position,
                COALESCE(bp.finished, 0)   AS finished,
                bp.last_played             AS last_played,
                COALESCE(
                    (SELECT SUM(c.duration) FROM book_chapters c WHERE c.book_id = b.id),
                    0.0
                ) AS total_duration,
                (SELECT group_concat(name, ', ') FROM
                    (SELECT p.name FROM book_authors ba JOIN book_people p ON p.id = ba.person_id
                     WHERE ba.book_id = b.id ORDER BY p.sort_name)
                ) AS author_display,
                (SELECT group_concat(name, ', ') FROM
                    (SELECT p.name FROM book_narrators bn JOIN book_people p ON p.id = bn.person_id
                     WHERE bn.book_id = b.id ORDER BY p.sort_name)
                ) AS narrator_display
         FROM books b
         LEFT JOIN series sr ON sr.id = b.series_id
         LEFT JOIN book_playback bp ON bp.book_id = b.id
         ORDER BY sr.name IS NOT NULL, sr.name, b.series_sequence, b.title",
    )?;
    let rows = stmt.query_map([], |row| {
        let rating: i64 = row.get("rating")?;
        Ok(BookListRow {
            id: row.get("id")?,
            title: row.get("title")?,
            subtitle: row.get("subtitle")?,
            author_display: row.get("author_display")?,
            narrator_display: row.get("narrator_display")?,
            series_name: row.get("series_name")?,
            series_sequence: row.get("series_sequence")?,
            year: row.get("year")?,
            cover_path: row.get("cover_path")?,
            accent_rgb: row.get::<_, Option<i64>>("accent_rgb")?.map(|v| v as u32),
            rating: rating as u8,
            starred: row.get("starred")?,
            position: row.get("position")?,
            finished: row.get("finished")?,
            last_played: row.get("last_played")?,
            total_duration: row.get("total_duration")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Order shelf rows for display (spec §3.8): in-progress first (most recently
/// played first), then new, then finished; ties broken by title. Pure, so the
/// ordering rule is unit-tested independently of the read.
pub fn sort_shelf(rows: &mut [BookListRow]) {
    fn rank(r: &BookListRow) -> u8 {
        match r.state() {
            BookState::InProgress => 0,
            BookState::New => 1,
            BookState::Finished => 2,
        }
    }
    rows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            // Most recently played first within a rank (None sorts last).
            .then_with(|| b.last_played.cmp(&a.last_played))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

/// A shelf ordering (16.5g): the spec §3.8 in-progress-first default plus the
/// simple keys a sort picker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShelfSort {
    #[default]
    InProgress,
    Title,
    Author,
    RecentlyPlayed,
}

/// Order shelf rows by `key` (16.5g): [`sort_shelf`] for the default, else a
/// stable case-insensitive sort (author falls back to title within an author;
/// never-played books sort last under recency). Pure, unit-tested.
pub fn sort_shelf_by(rows: &mut [BookListRow], key: ShelfSort) {
    match key {
        ShelfSort::InProgress => sort_shelf(rows),
        ShelfSort::Title => {
            rows.sort_by_key(|r| r.title.to_lowercase());
        }
        ShelfSort::Author => rows.sort_by(|a, b| {
            let name = |r: &BookListRow| {
                r.author_display
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
            };
            name(a)
                .cmp(&name(b))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        ShelfSort::RecentlyPlayed => rows.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
    }
}

/// A row-mapper for `verify_results`, shared by the lookup and report reads.
fn row_to_verify_result(row: &rusqlite::Row) -> rusqlite::Result<VerifyResultRow> {
    let verdict: String = row.get("verdict")?;
    Ok(VerifyResultRow {
        file_path: row.get("file_path")?,
        file_size: row.get("file_size")?,
        file_mtime: row.get("file_mtime")?,
        verdict: verdict.parse().unwrap_or(VerifyVerdict::Suspect),
        detail: row.get("detail")?,
        checked_at: row.get("checked_at")?,
    })
}

/// The cached verify rows for the given library-relative `paths`, as a
/// `path -> row` map (Phase 8a). Used to skip files whose on-disk size/mtime
/// still match a prior verdict. Chunked so a large selection stays under
/// SQLite's bound-parameter limit.
pub fn read_verify_results(
    conn: &Connection,
    paths: &[String],
) -> Result<HashMap<String, VerifyResultRow>> {
    let mut out = HashMap::with_capacity(paths.len());
    for chunk in paths.chunks(900) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT file_path, file_size, file_mtime, verdict, detail, checked_at
             FROM verify_results WHERE file_path IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(chunk.iter());
        let rows = stmt.query_map(params, row_to_verify_result)?;
        for r in rows {
            let r = r?;
            out.insert(r.file_path.clone(), r);
        }
    }
    Ok(out)
}

/// Every outstanding APE-strip undo row (Phase 8c-iii), ordered by path. The
/// `apestrip --undo` worklist.
pub fn ape_strips(conn: &Connection) -> Result<Vec<ApeStripRow>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, ape_bytes, tag_start, orig_size, orig_mtime, stripped_at
         FROM ape_strips ORDER BY file_path",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ApeStripRow {
            file_path: row.get("file_path")?,
            ape_bytes: row.get("ape_bytes")?,
            tag_start: row.get("tag_start")?,
            orig_size: row.get("orig_size")?,
            orig_mtime: row.get("orig_mtime")?,
            stripped_at: row.get("stripped_at")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Every cached file with a CORRUPT or SUSPECT verdict (Phase 8a), corrupt first
/// then suspect, each by path. The library-wide health report.
pub fn corrupt_or_suspect(conn: &Connection) -> Result<Vec<VerifyResultRow>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, file_size, file_mtime, verdict, detail, checked_at
         FROM verify_results
         WHERE verdict IN ('corrupt', 'suspect')
         ORDER BY CASE verdict WHEN 'corrupt' THEN 0 ELSE 1 END, file_path",
    )?;
    let rows = stmt.query_map([], row_to_verify_result)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}
