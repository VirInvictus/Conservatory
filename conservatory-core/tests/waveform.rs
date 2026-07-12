//! Phase 19a: the bucketing + cache format are unit-tested in the module; the
//! offline ffmpeg decode is a skip-if-absent integration test (the replaygain /
//! libmpv-smoke precedent), since ffmpeg is an external tool that may be missing
//! in CI. The committed tone fixtures are the same ones the replaygain scan uses.

use std::path::PathBuf;

use conservatory_core::{compute_envelope, envelope_for, ffmpeg_available};

fn fixture_audio(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audio")
        .join(name)
}

#[test]
fn decodes_a_committed_fixture_to_a_bounded_envelope() {
    if !ffmpeg_available() {
        eprintln!(
            "skipping decodes_a_committed_fixture_to_a_bounded_envelope: ffmpeg not installed"
        );
        return;
    }
    // The committed fixtures are short silent clips (they exist for tag/format
    // coverage), so this exercises the decode path + bucketing shape + range; a
    // silent input correctly yields an all-zero envelope, never a panic.
    let env = compute_envelope(&fixture_audio("sample.flac"), 200).expect("decode fixture");
    assert_eq!(env.buckets, 200);
    assert_eq!(env.peak.len(), 200);
    assert_eq!(env.rms.len(), 200);
    assert!(
        env.peak.iter().all(|&p| (0.0..=1.0).contains(&p)),
        "peaks must be normalized into 0.0..=1.0"
    );
}

#[test]
fn a_synthesized_tone_normalizes_to_unit_peak() {
    if !ffmpeg_available() {
        eprintln!("skipping a_synthesized_tone_normalizes_to_unit_peak: ffmpeg not installed");
        return;
    }
    // Synthesize a real 440 Hz tone so the decode sees actual signal (the
    // committed fixtures are silent). A full-scale sine normalizes to 1.0.
    let dir = tempfile::tempdir().unwrap();
    let tone = dir.path().join("tone.flac");
    let ok = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=1")
        .args(["-ac", "1"])
        .arg(&tone)
        .status()
        .expect("run ffmpeg")
        .success();
    assert!(ok, "ffmpeg should synthesize the tone");

    let env = compute_envelope(&tone, 200).expect("decode tone");
    let max = env.peak.iter().copied().fold(0.0f32, f32::max);
    assert!(
        (max - 1.0).abs() < 1e-6,
        "a full-scale sine should normalize to 1.0, got {max}"
    );
    assert!(
        env.peak.iter().any(|&p| p > 0.5),
        "a 440 Hz tone should be loud across buckets"
    );
}

#[test]
fn envelope_for_caches_under_xdg_cache_home() {
    if !ffmpeg_available() {
        eprintln!("skipping envelope_for_caches_under_xdg_cache_home: ffmpeg not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Point the cache at a temp dir so the test is hermetic and self-cleaning.
    unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };

    let f = fixture_audio("sample.opus");
    let first = envelope_for(&f, 128).expect("first envelope");
    assert_eq!(first.buckets, 128);

    // Exactly one cache file was written under the waveforms dir.
    let waveforms = dir.path().join("conservatory").join("waveforms");
    let count = std::fs::read_dir(&waveforms)
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        count, 1,
        "one cache file should be written on the first call"
    );

    // The second call reads the cache; it matches the first within the u16
    // quantization quantum (the cache is lossy by design, an optimization).
    let second = envelope_for(&f, 128).expect("second envelope");
    assert_eq!(second.buckets, 128);
    let quantum = 1.0 / u16::MAX as f32;
    for (a, b) in first.peak.iter().zip(second.peak.iter()) {
        assert!(
            (a - b).abs() <= quantum,
            "cache round-trip drifted: {a} vs {b}"
        );
    }

    unsafe { std::env::remove_var("XDG_CACHE_HOME") };
}
