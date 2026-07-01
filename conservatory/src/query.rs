//! Filter-bar composition (Phase 3c, spec §3.4): facet narrowing AND the
//! search-grammar expression, intersected. "The panes filter; the grammar
//! searches; they are the same surface." The grammar lives in
//! `conservatory-search` and the storage/SQL side in `conservatory-core`; this
//! is the thin consumer glue that joins them (the same shape as the CLI's
//! `search` verb), kept out of the GTK widgets so it stays headless-testable.

use std::collections::HashSet;

use chrono::NaiveDate;

use conservatory_core::db::{
    FacetFilter, PlaylistOrder, ReadPool, SearchRow, SqlParam, TrackBrief, facet_tracks,
    ordered_track_ids, perspective_expression, search_rows, search_track_ids,
};
use conservatory_search::{
    PerspectiveResolver, SearchItem, SqlValue, evaluate, parse_with_resolver, try_translate,
};

/// Resolves `vl:NAME` against the saved Perspectives table (Phase 3c). Opens a
/// fresh read handle per lookup; lookups are rare (only on `vl:` in the bar).
pub(crate) struct PoolResolver<'a>(pub(crate) &'a ReadPool);

impl PerspectiveResolver for PoolResolver<'_> {
    fn expression(&self, name: &str) -> Option<String> {
        let conn = self.0.open().ok()?;
        perspective_expression(&conn, name).ok().flatten()
    }
}

/// The leaf set for the current browse: `facet_tracks(filters)` narrowed by the
/// filter-bar `query`, with the facet browse order preserved (the grammar only
/// removes rows; column headers do the sorting). Returns the forgiving parser's
/// warnings, non-empty when the input degraded (the UI tints the bar). An empty
/// query is just the facet set.
pub fn query_leaf(
    pool: &ReadPool,
    filters: &[FacetFilter],
    query: &str,
    today: NaiveDate,
) -> (Vec<TrackBrief>, Vec<String>) {
    let Ok(conn) = pool.open() else {
        return (Vec::new(), Vec::new());
    };
    let mut tracks = facet_tracks(&conn, filters).unwrap_or_default();
    if query.trim().is_empty() {
        return (tracks, Vec::new());
    }

    // SQL fast path when the whole expression translates; else in-memory eval
    // over the search rows (the all-or-nothing dual path, mirroring the CLI).
    // `vl:NAME` expands from the saved Perspectives at parse time.
    let parsed = parse_with_resolver(query, &PoolResolver(pool));
    let matched: HashSet<i64> = match try_translate(&parsed.expr, today) {
        Some(clause) => {
            let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
            search_track_ids(&conn, &clause.sql, &params)
                .unwrap_or_default()
                .into_iter()
                .collect()
        }
        None => search_rows(&conn)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| evaluate(&parsed.expr, &to_item(r), today))
            .map(|r| r.track_id)
            .collect(),
    };
    tracks.retain(|t| matched.contains(&t.id));
    (tracks, parsed.warnings)
}

/// Materialise a smart playlist to ordered, limited track ids (Phase 16d-ii). The
/// SQL fast path lets core order + limit in one query; a regex / fuzzy query that
/// does not translate falls back to in-memory eval + a best-effort sort. Mirrors
/// the CLI's `materialize_smart`; core stays free of the search grammar (spec §2.2).
pub fn materialize_smart(
    pool: &ReadPool,
    query: &str,
    order: PlaylistOrder,
    limit: Option<i64>,
    today: NaiveDate,
) -> Vec<i64> {
    let Ok(conn) = pool.open() else {
        return Vec::new();
    };
    let parsed = parse_with_resolver(query, &PoolResolver(pool));
    match try_translate(&parsed.expr, today) {
        Some(clause) => {
            let params: Vec<SqlParam> = clause.params.iter().map(to_param).collect();
            ordered_track_ids(&conn, &clause.sql, &params, order, limit).unwrap_or_default()
        }
        None => {
            let mut rows: Vec<SearchRow> = search_rows(&conn)
                .unwrap_or_default()
                .into_iter()
                .filter(|r| evaluate(&parsed.expr, &to_item(r), today))
                .collect();
            sort_search_rows(&mut rows, order);
            let mut ids: Vec<i64> = rows.into_iter().map(|r| r.track_id).collect();
            if let Some(n) = limit
                && n >= 0
            {
                ids.truncate(n as usize);
            }
            ids
        }
    }
}

/// Best-effort order for the eval fallback (regex / fuzzy). `SearchRow` carries no
/// `last_played`, so `lastplayed` degrades to `added` here; the SQL path is exact.
fn sort_search_rows(rows: &mut [SearchRow], order: PlaylistOrder) {
    match order {
        PlaylistOrder::Added | PlaylistOrder::LastPlayed => {
            rows.sort_by(|a, b| b.added.cmp(&a.added).then(a.track_id.cmp(&b.track_id)))
        }
        PlaylistOrder::Rating => {
            rows.sort_by(|a, b| b.rating.cmp(&a.rating).then(b.added.cmp(&a.added)))
        }
        PlaylistOrder::Title => rows.sort_by_key(|r| r.title.to_lowercase()),
        PlaylistOrder::Artist => {
            rows.sort_by_key(|r| r.artist.as_deref().unwrap_or("").to_lowercase())
        }
    }
}

fn to_param(value: &SqlValue) -> SqlParam {
    match value {
        SqlValue::Text(s) => SqlParam::Text(s.clone()),
        SqlValue::Int(n) => SqlParam::Int(*n),
        SqlValue::Real(x) => SqlParam::Real(*x),
    }
}

fn to_item(r: &SearchRow) -> SearchItem {
    SearchItem {
        title: r.title.clone(),
        artist: r.artist.clone(),
        album_artist: r.album_artist.clone(),
        album: r.album.clone(),
        shelf_genre: r.shelf_genre.clone(),
        genres: r.genres.clone(),
        year: r.year,
        added: r.added,
        rating: r.rating,
        bitrate: r.bitrate,
        duration: r.duration,
        format: r.format.clone(),
        played: r.played,
        starred: r.starred,
        queued: r.queued,
        // Music rows carry no audiobook projection (the shelf is matched in memory).
        ..SearchItem::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conservatory_core::db::fixtures::{self, FixtureScale};
    use conservatory_core::db::spawn_worker;
    use tempfile::tempdir;

    #[tokio::test]
    async fn filter_intersects_facets_and_is_forgiving() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.db");
        let worker = spawn_worker(path.clone()).unwrap();
        fixtures::generate(&worker, FixtureScale::Small)
            .await
            .unwrap();
        let pool = ReadPool::new(path, 3).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();

        // Empty query → the full facet leaf (small fixture: 80 tracks).
        let (all, warns) = query_leaf(&pool, &[], "", today);
        assert!(warns.is_empty());
        assert_eq!(all.len(), 80);

        // A grammar genre filter narrows it to a subset of the same rows.
        let (electronic, _) = query_leaf(&pool, &[], "genre:Electronic", today);
        assert!(!electronic.is_empty() && electronic.len() < all.len());
        assert!(
            electronic.iter().all(|t| all.iter().any(|a| a.id == t.id)),
            "the filtered set is a subset of the facet set"
        );

        // An unknown field degrades to substring and warns, never errors.
        let (_, bogus_warns) = query_leaf(&pool, &[], "bogusfield:x", today);
        assert!(!bogus_warns.is_empty(), "unknown field warns");

        // A saved Perspective referenced as `vl:name` resolves to the same leaf
        // as its expression (save → reload through the filter bar).
        worker
            .save_perspective("Elec".into(), "genre:Electronic".into(), "tracks".into(), 0)
            .await
            .unwrap();
        let (via_vl, vl_warns) = query_leaf(&pool, &[], "vl:Elec", today);
        assert!(vl_warns.is_empty(), "a known vl: does not warn");
        let via_vl_ids: Vec<i64> = via_vl.iter().map(|t| t.id).collect();
        let electronic_ids: Vec<i64> = electronic.iter().map(|t| t.id).collect();
        assert_eq!(via_vl_ids, electronic_ids, "vl:Elec == genre:Electronic");

        worker.shutdown_ack().await.unwrap();
    }
}
