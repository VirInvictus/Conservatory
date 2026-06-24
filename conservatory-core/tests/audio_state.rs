//! Phase 5.5c: the audio configuration (playback defaults + DSP modules + output)
//! persistence through the single-writer worker and the read pool (temp DB).
//! Hermetic, no audio. The DSP `af` stage strings are unit-tested in
//! `player::dsp`; the libmpv `af` syntax is exercised by `tests/playback.rs`.

use conservatory_core::db::{ReadPool, WorkerHandle};
use conservatory_core::db::{ResamplerQuality, get_audio_state, spawn_worker};
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, WorkerHandle, ReadPool) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    let pool = ReadPool::new(path, 3).unwrap();
    (dir, worker, pool)
}

#[tokio::test]
async fn audio_state_defaults_then_round_trips() {
    let (_dir, worker, pool) = fresh();

    // The migration seeds the defaults: album RG, gapless, all DSP off, auto out.
    {
        let conn = pool.open().unwrap();
        let s = get_audio_state(&conn).unwrap();
        assert_eq!(s.replaygain_mode, "album");
        assert_eq!(s.replaygain_preamp, 0.0);
        assert!(s.replaygain_clip);
        assert!(s.gapless);
        assert!(s.dsp.is_off());
        assert_eq!(s.output_backend, "auto");
        assert_eq!(s.resampler, ResamplerQuality::Default);
    }

    // Enable + tune every module, change output, and persist.
    let mut s = get_audio_state(&pool.open().unwrap()).unwrap();
    s.dsp.comp.enabled = true;
    s.dsp.comp.settings.ratio = 4.0;
    s.dsp.comp.settings.threshold_db = -24.0;
    s.dsp.limiter.enabled = true;
    s.dsp.limiter.settings.ceiling_db = -0.5;
    s.dsp.leveler.enabled = true;
    s.dsp.leveler.settings.gausssize = 51;
    s.replaygain_mode = "track".to_string();
    s.replaygain_preamp = -3.0;
    s.gapless = false;
    s.output_backend = "pipewire".to_string();
    s.resampler = ResamplerQuality::High;
    worker.set_audio_state(s.clone()).await.unwrap();

    let back = get_audio_state(&pool.open().unwrap()).unwrap();
    assert_eq!(back, s);
    assert!(!back.dsp.is_off());

    worker.shutdown_ack().await.unwrap();
}

#[tokio::test]
async fn disabled_module_keeps_its_parameters() {
    // Toggling a tuned module off and on again must restore its parameters (the
    // settings persist independently of the enabled flag).
    let (_dir, worker, pool) = fresh();

    let mut s = get_audio_state(&pool.open().unwrap()).unwrap();
    s.dsp.comp.enabled = true;
    s.dsp.comp.settings.ratio = 6.0;
    worker.set_audio_state(s).await.unwrap();

    // Disable it (params untouched in the model).
    let mut s = get_audio_state(&pool.open().unwrap()).unwrap();
    assert_eq!(s.dsp.comp.settings.ratio, 6.0);
    s.dsp.comp.enabled = false;
    worker.set_audio_state(s).await.unwrap();

    let back = get_audio_state(&pool.open().unwrap()).unwrap();
    assert!(!back.dsp.comp.enabled);
    assert_eq!(
        back.dsp.comp.settings.ratio, 6.0,
        "params survive an off toggle"
    );

    worker.shutdown_ack().await.unwrap();
}
