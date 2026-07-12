//! Whole-track loudness envelope for the waveform seek bar (Phase 19a).
//!
//! The seek bar becomes the track's amplitude envelope: a fixed number of
//! buckets, each a normalized peak (and RMS body), drawn accent-tinted with a
//! played/unplayed split. libmpv exposes no offline decode, and the Phase 12d
//! PipeWire tap is a *live* monitor (it only sees audio as it plays), so the
//! envelope is computed by an offline `ffmpeg` PCM decode instead: the same
//! external-tool idiom as `verify.rs` (flac/ffmpeg) and `replaygain.rs`
//! (rsgain), no new Rust dependency. The decode + bucketing live here so they
//! are unit-testable headless (the spec §16.13 "logic in core" rule); the GTK
//! `DrawingArea` that renders the result lives in the binary.
//!
//! The result is cached under `$XDG_CACHE_HOME/conservatory/waveforms/`, keyed
//! by absolute path + mtime + bucket count + format version, so a track is
//! decoded once and re-decoded only when the file changes (the `verify.rs`
//! path+mtime staleness model). Covers stay glib-free here (core is
//! CLI-testable); the dir is resolved by hand exactly like `config_path`.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::{Error, Result};

/// The cache format version. Bump when the on-disk layout or the decode
/// parameters change, so a stale cache is silently regenerated rather than
/// mis-decoded.
pub const CACHE_VERSION: u32 = 1;

/// The decode sample rate. An amplitude envelope needs only the loudness
/// shape, not frequency content, so a low mono rate keeps the piped PCM small
/// (a 4-minute track is ~7.7 MB at 8 kHz) while preserving the peaks a viewer
/// reads. ffmpeg lowpasses at the Nyquist rate, which is fine for a visual.
const DECODE_RATE: u32 = 8_000;

/// The four-byte cache-file magic (`"CWV1"` in ASCII), a cheap corruption guard
/// so a truncated or foreign file is rejected rather than mis-read.
const MAGIC: [u8; 4] = *b"CWV1";

/// A track's normalized loudness envelope: `buckets` samples of `peak` (the
/// outline) and `rms` (the filled body), each in `0.0..=1.0`. The tallest peak
/// is `1.0`; `rms` shares that scale so the body stays proportional to the
/// outline.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformEnvelope {
    pub buckets: usize,
    pub peak: Vec<f32>,
    pub rms: Vec<f32>,
}

/// Reduce a mono PCM buffer to a `buckets`-long envelope. Pure and
/// deterministic (the unit-tested core). `buckets` is clamped to at least one
/// and never more than the sample count; empty or silent input yields an
/// all-zero envelope of the requested length (a flat bar, never a panic).
pub fn bucketize(samples: &[f32], buckets: usize) -> WaveformEnvelope {
    let buckets = buckets.max(1);
    if samples.is_empty() {
        return WaveformEnvelope {
            buckets,
            peak: vec![0.0; buckets],
            rms: vec![0.0; buckets],
        };
    }
    // Fewer samples than buckets: shrink so a bucket is never empty (which would
    // read as a false silent gap). A one-sample file collapses to one bucket.
    let buckets = buckets.min(samples.len());

    let mut peak = vec![0.0f32; buckets];
    let mut rms = vec![0.0f32; buckets];
    let n = samples.len();
    for (b, (peak_b, rms_b)) in peak.iter_mut().zip(rms.iter_mut()).enumerate() {
        // Half-open [lo, hi) span of this bucket; the last bucket absorbs the
        // remainder so every sample is counted exactly once.
        let lo = b * n / buckets;
        let hi = ((b + 1) * n / buckets).max(lo + 1);
        let span = &samples[lo..hi.min(n)];
        let mut p = 0.0f32;
        let mut sq = 0.0f64;
        for &s in span {
            p = p.max(s.abs());
            sq += (s as f64) * (s as f64);
        }
        *peak_b = p;
        *rms_b = (sq / span.len() as f64).sqrt() as f32;
    }

    // Normalize by the loudest peak so the tallest bar is full-height; a fully
    // silent track (max == 0) stays all zeros rather than dividing by zero.
    let max = peak.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        let inv = 1.0 / max;
        for v in peak.iter_mut().chain(rms.iter_mut()) {
            *v = (*v * inv).clamp(0.0, 1.0);
        }
    }
    WaveformEnvelope { buckets, peak, rms }
}

/// Decode `abs_path` to mono f32 PCM via ffmpeg. A spawn failure (ffmpeg not
/// installed) is a typed error distinct from a decode failure, mirroring
/// `verify.rs`. `-vn` drops any embedded cover-art video stream so only the
/// audio is decoded.
fn decode_pcm(abs_path: &Path) -> Result<Vec<f32>> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(abs_path)
        .args(["-vn", "-ac", "1", "-ar"])
        .arg(DECODE_RATE.to_string())
        .args(["-f", "f32le", "-"])
        .output()
        .map_err(|e| Error::Waveform(format!("running ffmpeg (is it installed?): {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(Error::Waveform(format!(
            "ffmpeg failed to decode {}: {line}",
            abs_path.display()
        )));
    }
    let samples = out
        .stdout
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    Ok(samples)
}

/// Decode `abs_path` and reduce it to a `buckets`-long envelope, ignoring any
/// cache. `envelope_for` is the cached entry point most callers want.
pub fn compute_envelope(abs_path: &Path, buckets: usize) -> Result<WaveformEnvelope> {
    Ok(bucketize(&decode_pcm(abs_path)?, buckets))
}

/// The waveform cache directory: `$XDG_CACHE_HOME/conservatory/waveforms/`, or
/// `~/.cache/conservatory/waveforms/` when `XDG_CACHE_HOME` is unset. Resolved
/// by hand (no glib) so core stays CLI-testable; matches `glib::user_cache_dir`
/// in the GTK binary.
pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("conservatory").join("waveforms")
}

/// The cache filename for `abs_path` at `buckets`, incorporating the file's
/// mtime and the format version so an edited file or a format bump misses the
/// stale entry. A hash keeps the name filesystem-safe and fixed-length.
fn cache_key(abs_path: &Path, buckets: usize, mtime_ns: u128) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    abs_path.hash(&mut h);
    buckets.hash(&mut h);
    mtime_ns.hash(&mut h);
    CACHE_VERSION.hash(&mut h);
    format!("{:016x}.cwv", h.finish())
}

/// The file mtime in nanoseconds since the epoch (0 if unavailable), the
/// staleness key: an in-place tag edit or a re-encode changes it.
fn mtime_ns(abs_path: &Path) -> u128 {
    std::fs::metadata(abs_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Serialize an envelope to the compact on-disk form: magic + version + bucket
/// count, then `buckets` pairs of `u16` (peak, rms) quantized from `0.0..=1.0`.
/// A hand-rolled format keeps the core free of a serde dependency here.
fn encode(env: &WaveformEnvelope) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12 + env.buckets * 4);
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(env.buckets as u32).to_le_bytes());
    for (&p, &r) in env.peak.iter().zip(env.rms.iter()) {
        bytes.extend_from_slice(&quantize(p).to_le_bytes());
        bytes.extend_from_slice(&quantize(r).to_le_bytes());
    }
    bytes
}

/// Parse the on-disk form back into an envelope; any mismatch (bad magic, wrong
/// version, short buffer) yields `None`, so a corrupt or foreign cache file is
/// treated as a miss and regenerated.
fn decode_cache(bytes: &[u8]) -> Option<WaveformEnvelope> {
    if bytes.len() < 12 || bytes[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != CACHE_VERSION {
        return None;
    }
    let buckets = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let body = &bytes[12..];
    if body.len() != buckets * 4 {
        return None;
    }
    let mut peak = Vec::with_capacity(buckets);
    let mut rms = Vec::with_capacity(buckets);
    for pair in body.chunks_exact(4) {
        peak.push(dequantize(u16::from_le_bytes([pair[0], pair[1]])));
        rms.push(dequantize(u16::from_le_bytes([pair[2], pair[3]])));
    }
    Some(WaveformEnvelope { buckets, peak, rms })
}

fn quantize(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

fn dequantize(v: u16) -> f32 {
    v as f32 / u16::MAX as f32
}

/// The cached entry point: return the stored envelope for `abs_path` at
/// `buckets` if the cache is fresh, otherwise decode, store best-effort, and
/// return. A cache write failure is non-fatal (the envelope is still returned):
/// the cache is an optimization, not a source of truth.
pub fn envelope_for(abs_path: &Path, buckets: usize) -> Result<WaveformEnvelope> {
    let key = cache_key(abs_path, buckets, mtime_ns(abs_path));
    let path = cache_dir().join(&key);
    if let Ok(bytes) = std::fs::read(&path)
        && let Some(env) = decode_cache(&bytes)
    {
        return Ok(env);
    }
    let env = compute_envelope(abs_path, buckets)?;
    if let Err(e) = store(&path, &env) {
        tracing::warn!(target: "conservatory::io", "waveform cache write failed: {e}");
    }
    Ok(env)
}

/// Write `env` to `path`, creating the cache directory on first use.
fn store(path: &Path, env: &WaveformEnvelope) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, encode(env))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucketize_normalizes_to_unit_peak() {
        // A ramp: the last (loudest) sample defines the peak, so the tallest
        // bucket normalizes to exactly 1.0 and buckets rise monotonically.
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let env = bucketize(&samples, 10);
        assert_eq!(env.buckets, 10);
        assert_eq!(env.peak.len(), 10);
        assert_eq!(env.rms.len(), 10);
        let max = env.peak.iter().copied().fold(0.0, f32::max);
        assert!(
            (max - 1.0).abs() < 1e-6,
            "tallest peak should be 1.0, got {max}"
        );
        for w in env.peak.windows(2) {
            assert!(w[1] >= w[0], "ramp buckets should be non-decreasing");
        }
    }

    #[test]
    fn bucketize_silence_is_all_zero() {
        let env = bucketize(&[0.0; 500], 8);
        assert_eq!(env.buckets, 8);
        assert!(env.peak.iter().all(|&p| p == 0.0));
        assert!(env.rms.iter().all(|&r| r == 0.0));
    }

    #[test]
    fn bucketize_handles_empty_and_short_input() {
        let empty = bucketize(&[], 16);
        assert_eq!(empty.peak.len(), 16);
        assert!(empty.peak.iter().all(|&p| p == 0.0));

        // Three samples, sixteen requested buckets: clamp to three so no bucket
        // is empty.
        let short = bucketize(&[1.0, -0.5, 0.25], 16);
        assert_eq!(short.buckets, 3);
        assert_eq!(short.peak.len(), 3);
        assert!((short.peak[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bucketize_zero_buckets_clamps_to_one() {
        let env = bucketize(&[0.5, 1.0], 0);
        assert_eq!(env.buckets, 1);
        assert_eq!(env.peak.len(), 1);
    }

    #[test]
    fn encode_decode_round_trips() {
        let env = WaveformEnvelope {
            buckets: 4,
            peak: vec![0.0, 0.5, 1.0, 0.25],
            rms: vec![0.0, 0.4, 0.8, 0.1],
        };
        let decoded = decode_cache(&encode(&env)).expect("round trip");
        assert_eq!(decoded.buckets, 4);
        // u16 quantization is lossy; check each value survives within a quantum.
        for (a, b) in env.peak.iter().zip(decoded.peak.iter()) {
            assert!((a - b).abs() < 1.0 / u16::MAX as f32);
        }
        for (a, b) in env.rms.iter().zip(decoded.rms.iter()) {
            assert!((a - b).abs() < 1.0 / u16::MAX as f32);
        }
    }

    #[test]
    fn decode_cache_rejects_bad_input() {
        assert!(decode_cache(&[]).is_none());
        assert!(decode_cache(b"XXXX\x01\x00\x00\x00").is_none()); // bad magic
        let mut wrong_ver = encode(&bucketize(&[1.0], 1));
        wrong_ver[4] = 0xFF; // corrupt the version field
        assert!(decode_cache(&wrong_ver).is_none());
    }

    #[test]
    fn cache_key_varies_with_inputs() {
        let p = Path::new("/music/a.flac");
        let base = cache_key(p, 1500, 100);
        assert_ne!(
            base,
            cache_key(p, 1000, 100),
            "buckets should change the key"
        );
        assert_ne!(base, cache_key(p, 1500, 200), "mtime should change the key");
        assert_ne!(
            base,
            cache_key(Path::new("/music/b.flac"), 1500, 100),
            "path should change the key"
        );
        assert_eq!(base, cache_key(p, 1500, 100), "same inputs are stable");
    }

    #[test]
    fn cache_dir_honours_xdg_cache_home() {
        let prev = std::env::var_os("XDG_CACHE_HOME");
        unsafe { std::env::set_var("XDG_CACHE_HOME", "/tmp/xdg-cache-test") };
        assert_eq!(
            cache_dir(),
            PathBuf::from("/tmp/xdg-cache-test/conservatory/waveforms")
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_CACHE_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CACHE_HOME") },
        }
    }
}
