//! Phase 3c: Perspective storage (spec §3.4). Save/overwrite/delete through the
//! worker, and the storage-backed `vl:NAME` resolution that lets a saved search
//! be referenced from another expression.

use conservatory_core::db::{ReadPool, list_perspectives, perspective_expression, spawn_worker};
use conservatory_search::{PerspectiveResolver, parse, parse_with_resolver};
use tempfile::tempdir;

/// The production resolver shape: look the name up in the perspectives table.
struct PoolResolver(ReadPool);

impl PerspectiveResolver for PoolResolver {
    fn expression(&self, name: &str) -> Option<String> {
        let conn = self.0.open().ok()?;
        perspective_expression(&conn, name).ok().flatten()
    }
}

#[tokio::test]
async fn crud_and_vl_resolution() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("library.db");
    let worker = spawn_worker(path.clone()).unwrap();

    let id = worker
        .save_perspective(
            "Favourites".into(),
            "rating:>=4".into(),
            "tracks".into(),
            100,
        )
        .await
        .unwrap();
    worker
        .save_perspective(
            "Lossless".into(),
            "format:flac".into(),
            "tracks".into(),
            101,
        )
        .await
        .unwrap();
    // Saving an existing name overwrites the expression in place (same id).
    let id_again = worker
        .save_perspective("Favourites".into(), "rating:5".into(), "tracks".into(), 200)
        .await
        .unwrap();
    assert_eq!(id, id_again, "same name overwrites, keeping the row id");

    let pool = ReadPool::new(path, 3).unwrap();
    let conn = pool.open().unwrap();
    let list = list_perspectives(&conn).unwrap();
    assert_eq!(list.len(), 2, "two distinct names");
    let fav = list.iter().find(|p| p.name == "Favourites").unwrap();
    assert_eq!(fav.expression, "rating:5", "overwrite took effect");
    let fav_id = fav.id;
    drop(conn);

    // vl:Favourites expands to the stored expression at parse time, so it equals
    // parsing that text directly.
    let resolver = PoolResolver(pool);
    let direct = parse("rating:5");
    let via_vl = parse_with_resolver("vl:Favourites", &resolver);
    assert_eq!(
        via_vl.expr, direct.expr,
        "vl: resolves to the saved expression"
    );
    assert!(via_vl.warnings.is_empty());

    // An unknown name degrades (forgiving), it does not error.
    let missing = parse_with_resolver("vl:NoSuchThing", &resolver);
    assert!(!missing.warnings.is_empty(), "unknown perspective warns");

    worker.delete_perspective(fav_id).await.unwrap();
    let conn = resolver.0.open().unwrap();
    assert_eq!(
        list_perspectives(&conn).unwrap().len(),
        1,
        "delete removed one"
    );

    worker.shutdown_ack().await.unwrap();
}
