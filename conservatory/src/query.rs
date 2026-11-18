//! Filter-bar composition (Phase 3c, spec §3.4): facet narrowing AND the
//! search-grammar expression, intersected. "The panes filter; the grammar
//! searches; they are the same surface." The grammar lives in the shared
//! `vir_search` crate and the storage/SQL side in `conservatory-core`; this
//! is the thin consumer glue that joins them (the same shape as the CLI's
//! `search` verb), kept out of the GTK widgets so it stays headless-testable.

use std::collections::HashSet;

use chrono::NaiveDate;

use conservatory_core::db::{
    FacetFilter, PlaylistOrder, ReadPool, SearchRow, SqlParam, TrackBrief, facet_track_ids,
    facet_tracks, ordered_track_ids, perspective_expression, search_rows, search_track_ids,
};
use conservatory_core::search::{
    Field, SearchItem, SortKey, SqlValue, State, evaluate, try_translate,
};
use vir_search::parse::{PerspectiveResolver, parse_with_resolver};

/// Resolves `vl:NAME` against the saved Perspectives table (Phase 3c). Opens a
/// fresh read handle per lookup; lookups are rare (only on `vl:` in the bar).
pub(crate) struct PoolResolver<'a>(pub(crate) &'a ReadPool);

impl PerspectiveResolver<Field, State> for PoolResolver<'_> {
    fn expression(&self, name: &str) -> Option<String> {
        let conn = self.0.open().ok()?;
        perspective_expression(&conn, name).ok().flatten()
    }
}

/// The full library load behind the leaf's master store (the window's
/// data-refresh path): every track in browse order, the one projection query a
/// data refresh runs. Facet and query switches never call this — they narrow
/// by id set through [`query_leaf_ids`] (issue #10, the master-store shape).
pub fn load_master(pool: &ReadPool) -> Vec<TrackBrief> {
    let Ok(conn) = pool.open() else {
        return Vec::new();
    };
    facet_tracks(&conn, &[]).unwrap_or_default()
}

/// The leaf narrowing for the current browse, as id sets (issue #10): the
/// facet filters' id set (`facet_track_ids`, the same `WHERE` the browse
/// always evaluated, minus the per-row projection) and the filter-bar
/// grammar's id set, each `None` when that surface imposes no constraint
/// (an empty facet selection or an empty query runs no SQL at all). The
/// master-store filter intersects the two by membership. Returns the
/// forgiving parser's warnings, non-empty when the input degraded (the UI
/// tints the bar).
pub fn query_leaf_ids(
    pool: &ReadPool,
    filters: &[FacetFilter],
    query: &str,
    today: NaiveDate,
) -> (
    Option<std::collections::HashSet<i64>>,
    Option<std::collections::HashSet<i64>>,
    Vec<String>,
) {
    let facet_ids = (!filters.is_empty()).then(|| {
        pool.open()
            .ok()
            .and_then(|c| facet_track_ids(&c, filters).ok())
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<i64>>()
    });
    if query.trim().is_empty() {
        return (facet_ids, None, Vec::new());
    }

    // SQL fast path when the whole expression translates; else in-memory eval
    // over the search rows (the all-or-nothing dual path, mirroring the CLI).
    // `vl:NAME` expands from the saved Perspectives at parse time.
    let parsed = parse_with_resolver::<Field, State, SortKey, _>(query, &PoolResolver(pool));
    let Ok(conn) = pool.open() else {
        return (facet_ids, None, parsed.warnings);
    };
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
    (facet_ids, Some(matched), parsed.warnings)
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
    let parsed = parse_with_resolver::<Field, State, SortKey, _>(query, &PoolResolver(pool));
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
        composers: r.composers.clone(),
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

        // Empty filters and query → no constraints at all (no id sets, so the
        // master store shows everything; the small fixture: 80 tracks).
        let (facet_ids, query_ids, warns) = query_leaf_ids(&pool, &[], "", today);
        assert!(warns.is_empty());
        assert_eq!(facet_ids, None);
        assert_eq!(query_ids, None);
        assert_eq!(load_master(&pool).len(), 80);

        // A grammar genre query narrows to the matching id subset.
        let (no_facets, electronic, _) = query_leaf_ids(&pool, &[], "genre:Electronic", today);
        assert_eq!(no_facets, None);
        let electronic = electronic.expect("a query yields a set");
        assert!(!electronic.is_empty() && electronic.len() < 80);
        assert!(
            electronic
                .iter()
                .all(|id| load_master(&pool).iter().any(|t| t.id == *id)),
            "the query set is a subset of the master rows"
        );

        // A facet filter yields its own id set, intersecting the query's.
        let genre_values = {
            use conservatory_core::db::{FacetField, facet_rows};
            facet_rows(&pool.open().unwrap(), FacetField::Genre, &[]).unwrap()
        };
        let value = genre_values
            .iter()
            .find(|r| r.value == "Electronic")
            .map(|r| r.value.clone())
            .unwrap_or_else(|| genre_values[0].value.clone());
        let (facet_ids, query_ids, _) = query_leaf_ids(
            &pool,
            &[FacetFilter {
                field: conservatory_core::db::FacetField::Genre,
                values: vec![value],
            }],
            "genre:Electronic",
            today,
        );
        let facet_ids = facet_ids.expect("a facet filter yields a set");
        let query_ids = query_ids.expect("a query yields a set");
        assert_eq!(
            facet_ids, query_ids,
            "the facet value and the grammar term select the same tracks"
        );

        // An unknown field degrades to substring and warns, never errors.
        let (_, _, bogus_warns) = query_leaf_ids(&pool, &[], "bogusfield:x", today);
        assert!(!bogus_warns.is_empty(), "unknown field warns");

        // A saved Perspective referenced as `vl:name` resolves to the same id
        // set as its expression (save → reload through the filter bar).
        worker
            .save_perspective("Elec".into(), "genre:Electronic".into(), "tracks".into(), 0)
            .await
            .unwrap();
        let (_, via_vl, vl_warns) = query_leaf_ids(&pool, &[], "vl:Elec", today);
        assert!(vl_warns.is_empty(), "a known vl: does not warn");
        assert_eq!(via_vl, Some(electronic), "vl:Elec == genre:Electronic");

        worker.shutdown_ack().await.unwrap();
    }
}
