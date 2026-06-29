//! Faceted-browse queries (spec §3.3, roadmap Phase 3b). The deadbeef-cui
//! Columns UI logic lives here, headless and CLI-testable (the CLAUDE.md hard
//! rule); the GTK binary only renders these results.
//!
//! A pane shows the distinct values of one [`FacetField`] with per-value track
//! counts, narrowed by the selections in the panes above it ([`FacetFilter`]s).
//! Genre is multi-valued: a track tagged `Electronic; Ambient` counts under both
//! rows (the §5.2 facet/filesystem decoupling). All filters AND together.

use rusqlite::Connection;

use crate::errors::Result;

/// A browse facet. The default hierarchy is Genre → AlbumArtist → Album; the
/// pane set and order are configurable (spec §3.2, `config.toml [browse].panes`,
/// Phase 10c). The keys/titles align with the search grammar field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetField {
    Genre,
    ShelfGenre,
    AlbumArtist,
    Artist,
    Album,
    Year,
    Format,
}

impl FacetField {
    /// Every facet, in menu order (also the editor's option order, Phase 10c).
    pub const ALL: [FacetField; 7] = [
        FacetField::Genre,
        FacetField::ShelfGenre,
        FacetField::AlbumArtist,
        FacetField::Artist,
        FacetField::Album,
        FacetField::Year,
        FacetField::Format,
    ];

    /// The config token (aligned with the search grammar field names).
    pub fn as_key(self) -> &'static str {
        match self {
            FacetField::Genre => "genre",
            FacetField::ShelfGenre => "shelfgenre",
            FacetField::AlbumArtist => "albumartist",
            FacetField::Artist => "artist",
            FacetField::Album => "album",
            FacetField::Year => "year",
            FacetField::Format => "format",
        }
    }

    /// The pane column header.
    pub fn title(self) -> &'static str {
        match self {
            FacetField::Genre => "Genre",
            FacetField::ShelfGenre => "Shelf Genre",
            FacetField::AlbumArtist => "Album Artist",
            FacetField::Artist => "Artist",
            FacetField::Album => "Album",
            FacetField::Year => "Year",
            FacetField::Format => "Format",
        }
    }

    /// The noun for the `[All (N …)]` header row.
    pub fn plural(self) -> &'static str {
        match self {
            FacetField::Genre => "genres",
            FacetField::ShelfGenre => "shelf genres",
            FacetField::AlbumArtist => "album artists",
            FacetField::Artist => "artists",
            FacetField::Album => "albums",
            FacetField::Year => "years",
            FacetField::Format => "formats",
        }
    }

    /// Parse a config key (case-insensitive; accepts the underscored aliases).
    pub fn parse(key: &str) -> Option<FacetField> {
        match key.to_ascii_lowercase().as_str() {
            "genre" => Some(FacetField::Genre),
            "shelfgenre" | "shelf_genre" => Some(FacetField::ShelfGenre),
            "albumartist" | "album_artist" => Some(FacetField::AlbumArtist),
            "artist" => Some(FacetField::Artist),
            "album" => Some(FacetField::Album),
            "year" => Some(FacetField::Year),
            "format" => Some(FacetField::Format),
            _ => None,
        }
    }

    /// Resolve the `[browse].panes` config keys to facets: unknown keys are
    /// dropped (warned), the list is capped at 5 (spec §3.2), and an empty
    /// result falls back to the default hierarchy so browse is never paneless.
    pub fn panes_from_config(keys: &[String]) -> Vec<FacetField> {
        let mut fields: Vec<FacetField> = keys
            .iter()
            .filter_map(|k| {
                let f = FacetField::parse(k);
                if f.is_none() {
                    tracing::warn!("unknown browse pane field {k:?}; ignored");
                }
                f
            })
            .take(5)
            .collect();
        if fields.is_empty() {
            fields = vec![
                FacetField::Genre,
                FacetField::AlbumArtist,
                FacetField::Album,
            ];
        }
        fields
    }
}

/// An upstream pane's selection. An empty `values` is the `[All]` row (no
/// constraint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetFilter {
    pub field: FacetField,
    pub values: Vec<String>,
}

/// One row in a facet pane: a value and how many tracks fall under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetRow {
    pub value: String,
    pub count: i64,
}

/// A track as shown in the leaf list (the deadbeef-cui columns: artist, album,
/// genre, title, duration, rating).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackBrief {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Raw track genres, comma-joined and sorted, for the Genre column (display
    /// only; the multi-valued facet still drives narrowing). Empty when untagged.
    pub genres: String,
    pub duration: Option<f64>,
    pub rating: u8,
}

/// A sortable leaf column (Phase 3c). The GTK `ColumnView` header drives this;
/// the comparator lives here so it is testable headless (the CLAUDE.md rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackSort {
    Artist,
    Album,
    Genre,
    Title,
    Duration,
    Rating,
}

fn nocase(s: &Option<String>) -> String {
    s.as_deref().unwrap_or("").to_lowercase()
}

/// Pairwise comparison of two leaf rows by `key`, case-insensitive for text. The
/// GTK `CustomSorter` and [`sort_tracks`] share this so the two paths never
/// diverge (the SQL-vs-eval discipline, applied to sorting).
pub fn cmp_tracks(
    a: &TrackBrief,
    b: &TrackBrief,
    key: TrackSort,
    descending: bool,
) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    let ord = match key {
        TrackSort::Artist => nocase(&a.artist).cmp(&nocase(&b.artist)),
        TrackSort::Album => nocase(&a.album).cmp(&nocase(&b.album)),
        TrackSort::Genre => a.genres.to_lowercase().cmp(&b.genres.to_lowercase()),
        TrackSort::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        TrackSort::Duration => a
            .duration
            .unwrap_or(0.0)
            .partial_cmp(&b.duration.unwrap_or(0.0))
            .unwrap_or(Equal),
        TrackSort::Rating => a.rating.cmp(&b.rating),
    };
    if descending { ord.reverse() } else { ord }
}

/// Stable, case-insensitive sort of the leaf set by `key`. `sort_by` is stable,
/// so ties keep the browse order `facet_tracks` already produced.
pub fn sort_tracks(tracks: &mut [TrackBrief], key: TrackSort, descending: bool) {
    tracks.sort_by(|a, b| cmp_tracks(a, b, key, descending));
}

const VARIOUS: &str = "Various Artists";
const UNKNOWN: &str = "Unknown";
const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// The `WHERE` fragment for a set of filters (AND of self-contained `EXISTS`
/// subqueries against the outer `tracks t`), pushing the bound values in order.
/// Empty filters → `1=1`.
fn filter_sql(filters: &[FacetFilter], params: &mut Vec<String>) -> String {
    let mut parts = Vec::new();
    for f in filters {
        if f.values.is_empty() {
            continue;
        }
        let placeholders = vec!["?"; f.values.len()].join(", ");
        params.extend(f.values.iter().cloned());
        parts.push(match f.field {
            FacetField::Genre => format!(
                "EXISTS (SELECT 1 FROM track_genres tg JOIN genres g ON g.id = tg.genre_id \
                 WHERE tg.track_id = t.id AND g.name IN ({placeholders}))"
            ),
            FacetField::AlbumArtist => format!(
                "EXISTS (SELECT 1 FROM albums a LEFT JOIN artists ar ON ar.id = a.album_artist_id \
                 WHERE a.id = t.album_id AND COALESCE(ar.name, '{VARIOUS}') IN ({placeholders}))"
            ),
            FacetField::Album => format!(
                "EXISTS (SELECT 1 FROM albums a WHERE a.id = t.album_id AND a.title IN ({placeholders}))"
            ),
            FacetField::ShelfGenre => format!(
                "EXISTS (SELECT 1 FROM albums a WHERE a.id = t.album_id \
                 AND COALESCE(a.shelf_genre, '{UNKNOWN}') IN ({placeholders}))"
            ),
            FacetField::Artist => format!(
                "COALESCE((SELECT ar.name FROM artists ar WHERE ar.id = t.artist_id), \
                 '{UNKNOWN_ARTIST}') IN ({placeholders})"
            ),
            FacetField::Year => format!(
                "EXISTS (SELECT 1 FROM albums a WHERE a.id = t.album_id \
                 AND COALESCE(CAST(a.year AS TEXT), '{UNKNOWN}') IN ({placeholders}))"
            ),
            FacetField::Format => {
                format!("COALESCE(t.format, '{UNKNOWN}') IN ({placeholders})")
            }
        });
    }
    if parts.is_empty() {
        "1=1".to_string()
    } else {
        parts.join(" AND ")
    }
}

/// The value expression and joins for a target facet's outer query.
fn target_sql(target: FacetField) -> (&'static str, &'static str) {
    match target {
        FacetField::Genre => (
            "g.name",
            "JOIN track_genres tg ON tg.track_id = t.id JOIN genres g ON g.id = tg.genre_id",
        ),
        FacetField::AlbumArtist => (
            "COALESCE(ar.name, 'Various Artists')",
            "JOIN albums al ON t.album_id = al.id LEFT JOIN artists ar ON ar.id = al.album_artist_id",
        ),
        FacetField::Album => ("al.title", "JOIN albums al ON t.album_id = al.id"),
        FacetField::ShelfGenre => (
            "COALESCE(al.shelf_genre, 'Unknown')",
            "JOIN albums al ON t.album_id = al.id",
        ),
        FacetField::Artist => (
            "COALESCE(ta.name, 'Unknown Artist')",
            "LEFT JOIN artists ta ON ta.id = t.artist_id",
        ),
        FacetField::Year => (
            "COALESCE(CAST(al.year AS TEXT), 'Unknown')",
            "JOIN albums al ON t.album_id = al.id",
        ),
        FacetField::Format => ("COALESCE(t.format, 'Unknown')", ""),
    }
}

/// The distinct values of `target` with track counts, narrowed by `filters`,
/// ordered case-insensitively.
pub fn facet_rows(
    conn: &Connection,
    target: FacetField,
    filters: &[FacetFilter],
) -> Result<Vec<FacetRow>> {
    let mut params = Vec::new();
    let where_sql = filter_sql(filters, &mut params);
    let (value_expr, joins) = target_sql(target);
    let sql = format!(
        "SELECT {value_expr} AS v, COUNT(DISTINCT t.id) AS c
         FROM tracks t {joins}
         WHERE {where_sql}
         GROUP BY v ORDER BY v COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        Ok(FacetRow {
            value: r.get("v")?,
            count: r.get("c")?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The leaf track set matching all `filters`, in browse order.
pub fn facet_tracks(conn: &Connection, filters: &[FacetFilter]) -> Result<Vec<TrackBrief>> {
    let mut params = Vec::new();
    let where_sql = filter_sql(filters, &mut params);
    // The genre column is a comma-joined, name-ordered roll-up of the track's raw
    // genres (the inner ORDER BY feeds group_concat in order, so it is stable).
    let sql = format!(
        "SELECT t.id, t.title, t.duration, t.rating, ta.name AS artist, al.title AS album,
                (SELECT group_concat(name, ', ') FROM (
                    SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
                    WHERE tg.track_id = t.id ORDER BY g.name COLLATE NOCASE
                )) AS genres
         FROM tracks t
         LEFT JOIN artists ta ON ta.id = t.artist_id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN artists aa ON aa.id = al.album_artist_id
         WHERE {where_sql}
         ORDER BY aa.sort_name COLLATE NOCASE, al.title COLLATE NOCASE, t.disc_no, t.track_no"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        Ok(TrackBrief {
            id: r.get("id")?,
            title: r.get("title")?,
            artist: r.get("artist")?,
            album: r.get("album")?,
            genres: r.get::<_, Option<String>>("genres")?.unwrap_or_default(),
            duration: r.get("duration")?,
            rating: r.get::<_, i64>("rating")?.clamp(0, 5) as u8,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(
        id: i64,
        artist: &str,
        album: &str,
        genres: &str,
        title: &str,
        rating: u8,
    ) -> TrackBrief {
        TrackBrief {
            id,
            title: title.into(),
            artist: (!artist.is_empty()).then(|| artist.into()),
            album: (!album.is_empty()).then(|| album.into()),
            genres: genres.into(),
            duration: Some(id as f64), // distinct, so duration sort order is checkable
            rating,
        }
    }

    fn ids(tracks: &[TrackBrief]) -> Vec<i64> {
        tracks.iter().map(|t| t.id).collect()
    }

    #[test]
    fn sorts_case_insensitively_by_text_key() {
        let mut tracks = vec![
            brief(1, "boards of canada", "", "", "", 0),
            brief(2, "Aphex Twin", "", "", "", 0),
            brief(3, "amon tobin", "", "", "", 0),
        ];
        sort_tracks(&mut tracks, TrackSort::Artist, false);
        assert_eq!(ids(&tracks), vec![3, 2, 1]);
        sort_tracks(&mut tracks, TrackSort::Artist, true);
        assert_eq!(ids(&tracks), vec![1, 2, 3]);
    }

    #[test]
    fn ties_keep_prior_browse_order() {
        // Same artist → the comparator is Equal, and the stable sort must preserve
        // the incoming order (the browse order facet_tracks produced).
        let mut tracks = vec![
            brief(10, "X", "", "", "", 0),
            brief(11, "X", "", "", "", 0),
            brief(12, "X", "", "", "", 0),
        ];
        sort_tracks(&mut tracks, TrackSort::Artist, false);
        assert_eq!(ids(&tracks), vec![10, 11, 12]);
        sort_tracks(&mut tracks, TrackSort::Artist, true);
        assert_eq!(ids(&tracks), vec![10, 11, 12], "reverse must not flip ties");
    }

    #[test]
    fn numeric_keys_order_by_value() {
        let mut tracks = vec![
            brief(1, "", "", "", "", 5),
            brief(2, "", "", "", "", 1),
            brief(3, "", "", "", "", 3),
        ];
        sort_tracks(&mut tracks, TrackSort::Rating, false);
        assert_eq!(ids(&tracks), vec![2, 3, 1]);

        // Duration was seeded to the id, so ascending duration is ascending id.
        sort_tracks(&mut tracks, TrackSort::Duration, false);
        assert_eq!(ids(&tracks), vec![1, 2, 3]);
    }

    #[test]
    fn cmp_and_sort_agree() {
        let a = brief(1, "Aphex Twin", "", "", "", 0);
        let b = brief(2, "Boards of Canada", "", "", "", 0);
        assert_eq!(
            cmp_tracks(&a, &b, TrackSort::Artist, false),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            cmp_tracks(&a, &b, TrackSort::Artist, true),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn field_keys_round_trip_and_have_labels() {
        for f in FacetField::ALL {
            assert_eq!(FacetField::parse(f.as_key()), Some(f), "{f:?} key");
            assert!(!f.title().is_empty(), "{f:?} title");
            assert!(!f.plural().is_empty(), "{f:?} plural");
        }
        // Underscored aliases parse too; unknown keys do not.
        assert_eq!(
            FacetField::parse("album_artist"),
            Some(FacetField::AlbumArtist)
        );
        assert_eq!(
            FacetField::parse("shelf_genre"),
            Some(FacetField::ShelfGenre)
        );
        assert_eq!(FacetField::parse("ALBUM"), Some(FacetField::Album));
        assert_eq!(FacetField::parse("composer"), None);
    }

    #[test]
    fn panes_from_config_resolves_skips_caps_and_defaults() {
        let keys = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // A valid list resolves in order.
        assert_eq!(
            FacetField::panes_from_config(&keys(&["year", "albumartist", "format"])),
            vec![
                FacetField::Year,
                FacetField::AlbumArtist,
                FacetField::Format
            ],
        );
        // Unknown keys are dropped, the rest kept.
        assert_eq!(
            FacetField::panes_from_config(&keys(&["genre", "bogus", "album"])),
            vec![FacetField::Genre, FacetField::Album],
        );
        // Capped at 5.
        assert_eq!(
            FacetField::panes_from_config(&keys(&[
                "genre",
                "shelfgenre",
                "albumartist",
                "artist",
                "album",
                "year",
                "format",
            ]))
            .len(),
            5,
        );
        // Empty (or all-unknown) falls back to the default hierarchy.
        assert_eq!(
            FacetField::panes_from_config(&[]),
            vec![
                FacetField::Genre,
                FacetField::AlbumArtist,
                FacetField::Album
            ],
        );
        assert_eq!(
            FacetField::panes_from_config(&keys(&["nope"])),
            vec![
                FacetField::Genre,
                FacetField::AlbumArtist,
                FacetField::Album
            ],
        );
    }
}
