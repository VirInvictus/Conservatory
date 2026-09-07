//! 19b-iii integration tests: track credits round-trip through the embedded
//! tags, the import wiring into `track_credits` (the shared `artists` rows),
//! and the `composer:` search field on both the SQL and eval paths. Committed
//! fixtures keep CI hermetic.

use std::path::{Path, PathBuf};

use conservatory_core::db::{
    ReadPool, WorkerHandle, search_rows, search_track_ids, spawn_worker, track_credits,
};
use conservatory_core::mover::MoveMode;
use conservatory_core::search::try_translate;
use conservatory_core::{
    Credit, CreditRole, ImportOptions, TagWrite, import_folder, read_track, write_track_tags,
};
use tempfile::tempdir;

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

fn tagged_target() -> TagWrite {
    TagWrite {
        title: "Credit Track".into(),
        track_artist: Some("Performer One".into()),
        album: Some("Credit Album".into()),
        album_artist: Some("Performer One".into()),
        year: Some(2026),
        track_no: Some(1),
        genres: vec!["Ambient".into()],
        credits: vec![
            Credit {
                role: CreditRole::Composer,
                name: "Composer Two".into(),
            },
            Credit {
                role: CreditRole::Composer,
                name: "Composer One".into(),
            },
            Credit {
                role: CreditRole::Performer,
                name: "Performer One".into(),
            },
            Credit {
                role: CreditRole::Producer,
                name: "Producer Zero".into(),
            },
        ],
        ..Default::default()
    }
}

#[test]
fn credits_round_trip_through_embedded_tags() {
    let dir = tempdir().unwrap();
    // Vorbis comments (flac) and ID3v2 (mp3) exercise two of the per-format
    // lofty mappings; the m4a freeform path shares the generic key resolution.
    for name in ["sample.flac", "sample.mp3"] {
        let path = dir.path().join(name);
        std::fs::copy(fixture_audio(name), &path).unwrap();
        write_track_tags(&path, &tagged_target()).unwrap();
        let draft = read_track(&path).unwrap();
        let got: Vec<(CreditRole, String)> = draft
            .credits
            .iter()
            .map(|c| (c.role, c.name.clone()))
            .collect();
        if name == "sample.flac" {
            let mut got = got;
            got.sort();
            let mut want: Vec<(CreditRole, String)> = tagged_target()
                .credits
                .iter()
                .map(|c| (c.role, c.name.clone()))
                .collect();
            want.sort();
            assert_eq!(got, want, "{name} kept every credit");
        } else {
            // ID3v2 reality: only Composer has a reliable frame home (TCOM).
            // The database keeps every credit (§5.6: the DB is canonical).
            let roles: std::collections::HashSet<_> = got.iter().map(|(r, _)| *r).collect();
            assert_eq!(
                roles,
                std::collections::HashSet::from([CreditRole::Composer]),
                "{name} carried the one role ID3v2 reliably holds"
            );
            let written: std::collections::HashSet<_> = tagged_target()
                .credits
                .iter()
                .map(|c| (c.role, c.name.clone()))
                .collect();
            for g in &got {
                assert!(written.contains(g), "{name} kept only written names");
            }
        }
    }
}

/// Import a credit-tagged file and prove the credits land in `track_credits`
/// resolved through the shared `artists` rows.
async fn imported_lib(dir: &Path) -> (ReadPool, WorkerHandle) {
    let db = dir.join("lib.db");
    let lib = dir.join("lib");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::copy(fixture_audio("sample.flac"), src.join("sample.flac")).unwrap();
    write_track_tags(&src.join("sample.flac"), &tagged_target()).unwrap();
    let worker = spawn_worker(db.clone()).unwrap();
    let pool = ReadPool::new(db, 3).unwrap();
    import_folder(
        &worker,
        &pool,
        &src,
        &ImportOptions {
            library_root: lib,
            mode: MoveMode::Copy,
        },
    )
    .await
    .unwrap();
    (pool, worker)
}

#[tokio::test]
async fn import_carries_credits_and_composer_search_finds_them() {
    use conservatory_core::db::SqlParam;
    use conservatory_core::search::{Field as SearchField, SortKey, State as SearchState};
    use vir_search::parse;

    let to_params = |clause: &conservatory_core::search::SqlClause| -> Vec<SqlParam> {
        clause
            .params
            .iter()
            .map(|p| match p {
                conservatory_core::search::SqlValue::Text(s) => SqlParam::Text(s.clone()),
                conservatory_core::search::SqlValue::Int(n) => SqlParam::Int(*n),
                conservatory_core::search::SqlValue::Real(x) => SqlParam::Real(*x),
            })
            .collect()
    };

    let dir = tempdir().unwrap();
    let (pool, worker) = imported_lib(dir.path()).await;

    let conn = pool.open().unwrap();
    let credits = track_credits(&conn, 1).unwrap();
    let rendered: Vec<(String, String)> = credits
        .iter()
        .map(|c| (c.role.clone(), c.name.clone()))
        .collect();
    assert_eq!(
        rendered,
        vec![
            ("Composer".into(), "Composer One".into()),
            ("Composer".into(), "Composer Two".into()),
            ("Performer".into(), "Performer One".into()),
            ("Producer".into(), "Producer Zero".into()),
        ],
        "credits read back in role-then-sort order"
    );

    // The projection carries composers for the eval fallback...
    let rows = search_rows(&conn).unwrap();
    assert_eq!(rows[0].composers, vec!["Composer One", "Composer Two"]);

    // ...and `composer:` translates to SQL (the all-or-nothing path stays
    // intact: the same query must not have degraded to eval).
    let today = chrono::Utc::now().date_naive();
    let parsed = parse::<SearchField, SearchState, SortKey>("composer:\"Composer One\"");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    let clause = try_translate(&parsed.expr, today).expect("composer: pushes down to SQL");
    assert!(clause.sql.contains("track_credits"), "{}", clause.sql);
    let ids = search_track_ids(&conn, &clause.sql, &to_params(&clause)).unwrap();
    assert_eq!(ids, vec![1]);

    // A `composer:` term with no match narrows to nothing through the same path.
    let parsed = parse::<SearchField, SearchState, SortKey>("composer:nobody");
    let clause = try_translate(&parsed.expr, today).unwrap();
    let ids = search_track_ids(&conn, &clause.sql, &to_params(&clause)).unwrap();
    assert!(ids.is_empty());

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn credit_link_is_idempotent_and_cascades_on_delete() {
    let dir = tempdir().unwrap();
    let (pool, worker) = imported_lib(dir.path()).await;

    let artist_id = worker
        .get_or_create_artist(
            "Composer One".into(),
            conservatory_core::names::derive_sort_name("Composer One"),
            None,
        )
        .await
        .unwrap();
    for _ in 0..2 {
        worker
            .link_track_credit(1, artist_id, CreditRole::Composer)
            .await
            .unwrap();
    }
    let conn = pool.open().unwrap();
    assert_eq!(
        track_credits(&conn, 1).unwrap().len(),
        4,
        "re-link inserts nothing"
    );

    // Deleting the track clears its credits (the schema cascade).
    worker.delete_track(1).await.unwrap();
    assert!(track_credits(&conn, 1).unwrap().is_empty());

    worker.shutdown_ack().await.unwrap();
}
