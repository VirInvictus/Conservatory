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

/// A browse facet. The default hierarchy is Genre → AlbumArtist → Album.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetField {
    Genre,
    AlbumArtist,
    Album,
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

/// A track as shown in the leaf list (Phase 3c enriches this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackBrief {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
}

const VARIOUS: &str = "Various Artists";

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
    let sql = format!(
        "SELECT t.id, t.title, ta.name AS artist
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
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}
