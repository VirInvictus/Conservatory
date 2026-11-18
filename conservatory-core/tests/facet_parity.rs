//! The incremental-facet parity test (issue #10): `facet_track_ids` (the
//! id-set query the GUI's master-store filter runs per facet switch) must
//! select exactly the tracks `facet_tracks` (the full projection the browse
//! used to re-run per switch) selects, for every facet field and for
//! combinations. Both shapes run the same `filter_sql` today by
//! construction; this pins that construction, so a future edit to one path
//! and not the other fails here instead of silently narrowing the GUI
//! differently from the data.

use std::collections::HashSet;

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    FacetField, FacetFilter, ReadPool, facet_rows, facet_track_ids, facet_tracks, spawn_worker,
};
use tempfile::tempdir;

#[tokio::test]
async fn facet_track_ids_matches_facet_tracks_for_every_field() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();

    // Every facet field, one sampled value and one impossible value (the
    // empty-match arm), plus a two-field combination and no filters at all.
    let mut cases: Vec<Vec<FacetFilter>> = vec![Vec::new()];
    for field in [
        FacetField::Genre,
        FacetField::AlbumArtist,
        FacetField::Album,
        FacetField::ShelfGenre,
        FacetField::Artist,
        FacetField::Year,
        FacetField::Format,
        FacetField::Rating,
        FacetField::Added,
    ] {
        let rows = facet_rows(&conn, field, &[]).unwrap();
        assert!(!rows.is_empty(), "{field:?} facet has no values to sample");
        cases.push(vec![FacetFilter {
            field,
            values: vec![rows[0].value.clone()],
        }]);
        cases.push(vec![FacetFilter {
            field,
            values: vec!["no-such-value".into()],
        }]);
        // A combination with one other field's first value.
        let other = if field == FacetField::Genre {
            FacetField::Format
        } else {
            FacetField::Genre
        };
        let other_rows = facet_rows(&conn, other, &[]).unwrap();
        cases.push(vec![
            FacetFilter {
                field,
                values: vec![rows[0].value.clone()],
            },
            FacetFilter {
                field: other,
                values: vec![other_rows[0].value.clone()],
            },
        ]);
    }

    for filters in &cases {
        let mut full = facet_tracks(&conn, filters).unwrap();
        full.sort_by_key(|t| t.id);
        let mut narrow = facet_track_ids(&conn, filters).unwrap();
        narrow.sort();
        let full_ids: Vec<i64> = full.iter().map(|t| t.id).collect();
        assert_eq!(
            full_ids, narrow,
            "id-set divergence for filters: {filters:?}"
        );
        // And set-equality regardless of projection order, for good measure.
        let full_set: HashSet<i64> = full_ids.into_iter().collect();
        let narrow_set: HashSet<i64> = narrow.into_iter().collect();
        assert_eq!(full_set, narrow_set);
    }

    worker.shutdown_ack().await.unwrap();
}
