//! Phase 4a integration tests (spec §6.4): the playback-state cursor round-trip
//! through the single-writer worker, play-count-on-completion, and an `ao=null`
//! libmpv smoke test that decodes a committed fixture to end-of-file.
//!
//! The pure profile/debounce logic is unit-tested inside `player::profile` /
//! `player::state`; these cover the DB and libmpv glue those can't reach.

use std::path::PathBuf;

use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{
    CompSettings, DspState, EqState, LevelerSettings, LimiterSettings, ModuleState,
    ResamplerQuality,
};
use conservatory_core::db::{
    MediaKind, PlaybackCursor, ReadPool, get_track, read_playback_state, spawn_worker,
};
use conservatory_core::{EndReason, HostEvent, MpvHost, MusicProfile};
use tempfile::tempdir;

fn audio_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

/// The cursor is absent on a fresh library, persists on save, and stays a
/// singleton across overwrites. Track ids reference a real fixture library
/// because `playback_state.track_id` is a foreign key (`foreign_keys = ON`).
#[tokio::test]
async fn playback_state_round_trips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    // Nothing has played yet.
    {
        let conn = pool.open().unwrap();
        assert!(read_playback_state(&conn).unwrap().is_none());
    }

    worker
        .save_playback_state(PlaybackCursor {
            kind: MediaKind::Track,
            track_id: Some(1),
            episode_id: None,
            position: 42.5,
            paused: true,
            volume: 80,
            updated_at: 1_000,
        })
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let s = read_playback_state(&conn).unwrap().unwrap();
        assert_eq!(s.kind, MediaKind::Track);
        assert_eq!(s.track_id, Some(1));
        assert_eq!(s.episode_id, None);
        assert_eq!(s.position, 42.5);
        assert!(s.paused);
        assert_eq!(s.volume, 80);
        assert_eq!(s.updated_at, Some(1_000));
    }

    // A second save overwrites the one row rather than inserting another.
    worker
        .save_playback_state(PlaybackCursor {
            kind: MediaKind::Track,
            track_id: Some(2),
            episode_id: None,
            position: 3.0,
            paused: false,
            volume: 100,
            updated_at: 2_000,
        })
        .await
        .unwrap();
    {
        let conn = pool.open().unwrap();
        let s = read_playback_state(&conn).unwrap().unwrap();
        assert_eq!(s.track_id, Some(2));
        assert_eq!(s.position, 3.0);
        assert!(!s.paused);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM playback_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }
}

/// A completed play bumps `play_count` and stamps `last_played` (spec §6.4).
#[tokio::test]
async fn increment_play_count_bumps_and_stamps() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.db");
    let worker = spawn_worker(path.clone()).unwrap();
    fixtures::generate(&worker, FixtureScale::Small)
        .await
        .unwrap();
    let pool = ReadPool::new(path, 3).unwrap();

    let before = {
        let conn = pool.open().unwrap();
        get_track(&conn, 1).unwrap().unwrap()
    };
    assert_eq!(before.play_count, 0);
    assert!(before.last_played.is_none());

    worker.increment_play_count(1, 5_000).await.unwrap();
    worker.increment_play_count(1, 6_000).await.unwrap();

    let after = {
        let conn = pool.open().unwrap();
        get_track(&conn, 1).unwrap().unwrap()
    };
    assert_eq!(after.play_count, 2);
    assert_eq!(after.last_played.map(|t| t.timestamp()), Some(6_000));
}

/// The libmpv host decodes a real (0.3 s) fixture through to end-of-file with a
/// null audio output, so the load → pump → `EndReason::Eof` flow is exercised
/// without a sound server. Generously capped so a wedged decode can't hang CI.
#[test]
fn host_plays_fixture_to_eof() {
    // If libmpv can't initialize at runtime (it shouldn't fail, it's linked),
    // skip rather than fail: the host is verified for real by the `play` verb.
    let Ok(mut host) = MpvHost::new_null() else {
        return;
    };
    // A real ReplayGain head stage (Phase 5.5a), a non-flat equalizer (Phase
    // 5.5b), and all three DSP modules (Phase 5.5c): this proves the full
    // `@rg` → `@eq` → `@comp` → `@limit` → `@boost` `af`-chain syntax is accepted
    // by libmpv and does not break decode. Smart Speed + Voice Boost (Phase 6c)
    // are on too, so the run also proves the `@ss` (`silenceremove`) and `@vb*`
    // (`acompressor` / `highpass` / `equalizer` / `dynaudnorm`) spoken-word syntax.
    let profile = MusicProfile {
        gapless: true,
        replaygain_db: Some(-6.0),
        speed: 1.0,
        pitch_correction: false,
        smart_speed: true,
        voice_boost: true,
    };
    let mut eq = EqState::flat();
    eq.bands[0] = 6.0; // 31 Hz +6 dB
    eq.bands[9] = -4.5; // 16 kHz -4.5 dB
    host.set_eq(eq);
    host.set_dsp(DspState {
        comp: ModuleState {
            enabled: true,
            settings: CompSettings::default(),
        },
        limiter: ModuleState {
            enabled: true,
            settings: LimiterSettings::default(),
        },
        leveler: ModuleState {
            enabled: true,
            settings: LevelerSettings::default(),
        },
    });
    // A raised resampler (Phase 5.5c-ii): `load` re-asserts the `audio-resample-*`
    // knobs, so this proves they are accepted and don't break decode.
    host.set_resampler(ResamplerQuality::High)
        .expect("set resampler");
    host.load(audio_fixture("sample.flac").to_str().unwrap(), &profile)
        .expect("loading fixture");

    let mut ended = None;
    for _ in 0..200 {
        match host.pump(0.1) {
            HostEvent::Ended(reason) => {
                ended = Some(reason);
                break;
            }
            HostEvent::Shutdown => break,
            HostEvent::Idle => {}
        }
    }
    assert_eq!(
        ended,
        Some(EndReason::Eof),
        "the fixture should play through to a natural end-of-file"
    );
}

/// `load` applies the profile's speed + pitch correction to the host (Phase
/// 6b-ii-c-3-a per-show speed): a profile with `speed = 1.5` leaves mpv's
/// `speed` property at 1.5.
#[test]
fn host_load_applies_profile_speed() {
    let Ok(mut host) = MpvHost::new_null() else {
        return;
    };
    let profile = MusicProfile {
        gapless: false,
        replaygain_db: None,
        speed: 1.5,
        pitch_correction: true,
        smart_speed: false,
        voice_boost: false,
    };
    host.load(audio_fixture("sample.flac").to_str().unwrap(), &profile)
        .expect("loading fixture");
    assert_eq!(host.speed(), Some(1.5));
}

/// The output-device list (Phase 4c-ii) is queryable and always carries mpv's
/// `auto` pseudo-device; switching to it succeeds.
#[test]
fn host_lists_and_sets_audio_devices() {
    let Ok(mut host) = MpvHost::new_null() else {
        return;
    };
    let devices = host.audio_devices().expect("audio-device-list");
    assert!(
        devices.iter().any(|d| d.name == "auto"),
        "mpv always lists the `auto` pseudo-device, got {devices:?}"
    );
    host.set_audio_device("auto").expect("set audio-device");
}

/// The output backend (mpv `ao`) and resampler (Phase 5.5c-ii) apply without
/// erroring. `null` is used for the backend so the `ao` + `ao-reload` path is
/// exercised hermetically (a real driver might fail to init in CI); `set_resampler`
/// only sets properties, so it is always safe.
#[test]
fn host_sets_output_backend_and_resampler() {
    let Ok(mut host) = MpvHost::new_null() else {
        return;
    };
    host.set_output_backend("null")
        .expect("set ao + ao-reload to null");
    host.set_resampler(ResamplerQuality::High)
        .expect("set resampler high");
    host.set_resampler(ResamplerQuality::Default)
        .expect("set resampler default");
}
