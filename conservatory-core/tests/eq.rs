//! Phase 5.5b: the equalizer state + preset persistence through the single-writer
//! worker and the read pool (temp DB). Hermetic, no audio. The `@eq` chain string
//! itself is unit-tested in `player::chain`; the libmpv `af` syntax is exercised
//! by `tests/playback.rs`.

use conservatory_core::db::{
    EqState, ReadPool, get_eq_preset, get_eq_state, list_eq_presets, spawn_worker,
};
use tempfile::tempdir;

fn fresh() -> (
    tempfile::TempDir,
    conservatory_core::db::WorkerHandle,
    ReadPool,
) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    (dir, worker, pool)
}

#[tokio::test]
async fn eq_state_defaults_to_flat_and_round_trips() {
    let (_dir, worker, pool) = fresh();

    // The migration seeds a Flat state.
    {
        let conn = pool.open().unwrap();
        let state = get_eq_state(&conn).unwrap();
        assert!(state.is_flat());
        assert_eq!(state.preset.as_deref(), Some("Flat"));
    }

    // A custom edit persists the bands + drops the preset.
    let mut state = EqState::flat();
    state.bands[0] = 6.0;
    state.bands[9] = -4.5;
    state.preset = None;
    worker.set_eq_state(state).await.unwrap();

    let conn = pool.open().unwrap();
    let back = get_eq_state(&conn).unwrap();
    assert_eq!(back.bands[0], 6.0);
    assert_eq!(back.bands[9], -4.5);
    assert_eq!(back.preset, None);
    assert!(!back.is_flat());

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn presets_save_load_and_delete() {
    let (_dir, worker, pool) = fresh();

    // Flat plus the 16 built-ins (migration 0010) are seeded, alphabetical.
    {
        let conn = pool.open().unwrap();
        let names: Vec<_> = list_eq_presets(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names.len(), 17, "Flat + 16 built-ins");
        assert!(names.contains(&"Flat".to_string()));
        assert!(names.contains(&"Rock".to_string()));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list_eq_presets is alphabetical");
    }

    let mut bands = [0.0; 10];
    bands[0] = 8.0;
    bands[1] = 4.0;
    worker.save_eq_preset("Loud".into(), bands).await.unwrap();

    {
        let conn = pool.open().unwrap();
        let loud = get_eq_preset(&conn, "Loud").unwrap().unwrap();
        assert_eq!(loud[0], 8.0);
        assert_eq!(loud[1], 4.0);
        assert!(get_eq_preset(&conn, "Nope").unwrap().is_none());
        // The saved preset joins the 17 seeded ones, still alphabetical.
        let names: Vec<_> = list_eq_presets(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names.len(), 18, "17 seeded + the saved Loud");
        assert!(names.contains(&"Loud".to_string()));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    // Overwrite by name.
    let mut bands2 = [0.0; 10];
    bands2[0] = 10.0;
    worker.save_eq_preset("Loud".into(), bands2).await.unwrap();
    {
        let conn = pool.open().unwrap();
        assert_eq!(get_eq_preset(&conn, "Loud").unwrap().unwrap()[0], 10.0);
    }

    worker.delete_eq_preset("Loud".into()).await.unwrap();
    {
        let conn = pool.open().unwrap();
        assert!(get_eq_preset(&conn, "Loud").unwrap().is_none());
    }

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn builtin_presets_seeded_with_sane_bands() {
    let (_dir, worker, pool) = fresh();
    let conn = pool.open().unwrap();

    // A representative sample of the migration-0010 curves loads with the
    // expected shape (the full set is the migration's responsibility).
    let bass = get_eq_preset(&conn, "Bass Boost").unwrap().unwrap();
    assert_eq!(bass[0], 6.0); // 31 Hz lifted
    assert_eq!(bass[9], 0.0); // 16 kHz untouched

    let treble = get_eq_preset(&conn, "Treble Boost").unwrap().unwrap();
    assert_eq!(treble[0], 0.0);
    assert_eq!(treble[9], 6.0);

    // Spoken Word cuts rumble and lifts the speech band.
    let spoken = get_eq_preset(&conn, "Spoken Word").unwrap().unwrap();
    assert!(spoken[0] < 0.0, "sub-bass attenuated");
    assert!(spoken[6] > 0.0, "2 kHz presence lifted");

    // No built-in is wildly hot (headroom against octave-band stacking).
    for p in list_eq_presets(&conn).unwrap() {
        for g in p.bands {
            assert!(g.abs() <= 12.0, "{} band {g} dB out of sane range", p.name);
        }
    }

    worker.shutdown_ack().await.unwrap();
}

#[test]
fn eq_state_parse_is_forgiving() {
    // A short / malformed CSV reads the present bands and zero-fills the rest,
    // so a bad stored row never breaks playback.
    let bands = EqState::parse_bands("3.0,bad,,-2");
    assert_eq!(bands[0], 3.0);
    assert_eq!(bands[1], 0.0); // "bad" → 0
    assert_eq!(bands[2], 0.0); // empty → 0
    assert_eq!(bands[3], -2.0);
    assert_eq!(bands[4], 0.0); // missing → 0
    // Round-trips through format.
    let csv = EqState::format_bands(&bands);
    assert_eq!(EqState::parse_bands(&csv), bands);
}
