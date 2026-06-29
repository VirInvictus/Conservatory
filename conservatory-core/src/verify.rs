//! Audio-file integrity verification (Phase 8a, roadmap "Phase 8 / 8a").
//!
//! Decode-verify a file and classify it into Lattice's four tiers. FLAC goes
//! through `flac -t` (authoritative: it MD5-verifies the decoded stream, so it
//! catches bit-rot and truncation a plain decode misses); everything else goes
//! through a strict `ffmpeg` decode to a null sink. Both are external tools
//! (ATTRIBUTIONS.md, spec §11), shelled out the same way as `rsgain`
//! (`replaygain.rs`) and `ffprobe` (`conservatory-audiobooks`); a player decoder
//! was rejected because it is lenient by design and would weaken the verdicts.
//!
//! The shell-out wrappers are thin; the *classification* (which tier a tool's
//! exit + stderr maps to) is factored into pure functions so it is unit-tested
//! without the binaries present.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::errors::{Error, Result};

/// The integrity verdict for one file (Lattice's four tiers), ordered by
/// severity. Stored in the DB as the lowercase string from [`Self::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyVerdict {
    /// Decoded clean.
    Ok,
    /// Audio intact; only a container / tag warning.
    Metadata,
    /// Decoded to the end but the tool complained (or there was trailing data).
    Suspect,
    /// The decoder errored, or a FLAC decoded fewer samples than declared.
    Corrupt,
}

impl VerifyVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            VerifyVerdict::Ok => "ok",
            VerifyVerdict::Metadata => "metadata",
            VerifyVerdict::Suspect => "suspect",
            VerifyVerdict::Corrupt => "corrupt",
        }
    }

    /// The Lattice scriptable contract keys its non-zero exit on this.
    pub fn is_corrupt(self) -> bool {
        matches!(self, VerifyVerdict::Corrupt)
    }
}

impl std::str::FromStr for VerifyVerdict {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, ()> {
        match s {
            "ok" => Ok(VerifyVerdict::Ok),
            "metadata" => Ok(VerifyVerdict::Metadata),
            "suspect" => Ok(VerifyVerdict::Suspect),
            "corrupt" => Ok(VerifyVerdict::Corrupt),
            _ => Err(()),
        }
    }
}

/// Whether `format`/the path extension marks this as a FLAC (the `flac -t` path).
fn is_flac(path: &Path, format: Option<&str>) -> bool {
    if let Some(f) = format
        && f.eq_ignore_ascii_case("flac")
    {
        return true;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
}

/// Decode-verify one file, returning its tier and a short detail message
/// (`None` for OK). `format` (the DB's `tracks.format`, when known) picks the
/// decoder; the path extension is the fallback. A spawn failure (the tool is not
/// installed) is an `Err`, distinct from a CORRUPT verdict.
pub fn verify_file(
    abs_path: &Path,
    format: Option<&str>,
) -> Result<(VerifyVerdict, Option<String>)> {
    if is_flac(abs_path, format) {
        let out = Command::new("flac")
            .arg("-t")
            .arg("-s")
            .arg(abs_path)
            .output()
            .map_err(|e| Error::Verify(format!("running flac (is it installed?): {e}")))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        Ok(classify_flac(out.status.success(), &stderr))
    } else {
        let out = Command::new("ffmpeg")
            .args(["-v", "warning", "-nostdin", "-i"])
            .arg(abs_path)
            .args(["-f", "null", "-"])
            .output()
            .map_err(|e| Error::Verify(format!("running ffmpeg (is it installed?): {e}")))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        Ok(classify_ffmpeg(out.status.success(), &stderr))
    }
}

/// Classify a `flac -t` run. `flac -t` is effectively binary: it verifies the
/// audio stream and its MD5, so a non-zero exit means a bad stream (corruption
/// or truncation, "decoded fewer samples than declared"). A clean exit is OK
/// regardless of any benign tag note `-s` did not suppress. Pure.
pub fn classify_flac(exit_ok: bool, stderr: &str) -> (VerifyVerdict, Option<String>) {
    if exit_ok {
        (VerifyVerdict::Ok, None)
    } else {
        (VerifyVerdict::Corrupt, first_meaningful_line(stderr))
    }
}

/// Hard-error substrings: their presence (or a non-zero exit) means the decoder
/// could not read the audio, so the file is CORRUPT.
const FFMPEG_ERROR_MARKERS: &[&str] = &[
    "invalid data",
    "error while decoding",
    "error reading",
    "could not",
    "failed to read",
    "no such file",
    "header missing",
    "moov atom not found",
    "partial file",
    "end of file",
];

/// Softer markers: the decode reached the end but the tool flagged something, so
/// the file is SUSPECT (worth a human look, not a hard failure).
const FFMPEG_SUSPECT_MARKERS: &[&str] = &[
    "trailing",
    "non-monotonous",
    "non monotonically",
    "discontinuity",
    "overread",
    "concealing",
];

/// Benign `ffmpeg` notes that say nothing about integrity: a clean file routinely
/// emits these, so they must not flag it. Filtered out before classification (an
/// mp3's "Estimating duration from bitrate" was mislabelling every clean mp3).
const FFMPEG_BENIGN_MARKERS: &[&str] = &[
    "estimating duration",
    "using bitrate to estimate",
    "could not find codec parameters", // ffmpeg often recovers; not an audio error
];

/// Classify an `ffmpeg -f null` decode from its exit status and (`-v warning`)
/// stderr. Benign notes are filtered first; then a non-zero exit or any
/// hard-error marker is CORRUPT, a softer marker is SUSPECT, any other surviving
/// note (a real container/tag warning) is METADATA, and nothing left on a clean
/// exit is OK. Pure (the fuzzy logic, tested without ffmpeg). Case-insensitive.
pub fn classify_ffmpeg(exit_ok: bool, stderr: &str) -> (VerifyVerdict, Option<String>) {
    // The meaningful lines: non-blank and not pure benign noise.
    let meaningful: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            !FFMPEG_BENIGN_MARKERS.iter().any(|m| low.contains(m))
        })
        .collect();
    let detail = meaningful.first().map(|l| l.to_string());
    let joined = meaningful.join("\n").to_ascii_lowercase();
    let has = |markers: &[&str]| markers.iter().any(|m| joined.contains(m));

    if !exit_ok || has(FFMPEG_ERROR_MARKERS) {
        (
            VerifyVerdict::Corrupt,
            detail.or_else(|| first_meaningful_line(stderr)),
        )
    } else if has(FFMPEG_SUSPECT_MARKERS) {
        (VerifyVerdict::Suspect, detail)
    } else if meaningful.is_empty() {
        (VerifyVerdict::Ok, None)
    } else {
        (VerifyVerdict::Metadata, detail)
    }
}

/// The first non-blank stderr line, trimmed, as the stored detail.
fn first_meaningful_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Verify many files in parallel and return `(path, verdict, detail)` per file
/// (in arbitrary order; the caller re-associates by path). Parallelism is
/// `available_parallelism()` scoped threads pulling from a shared cursor, so a
/// large library saturates the CPU without a thread per file and without a new
/// dependency. A file whose tool fails to spawn yields a CORRUPT verdict with
/// the error as detail (a missing tool surfaces once, per file, rather than
/// aborting the whole run).
pub fn verify_files(
    items: &[(PathBuf, Option<String>)],
) -> Vec<(PathBuf, VerifyVerdict, Option<String>)> {
    if items.is_empty() {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(items.len());
    let cursor = AtomicUsize::new(0);
    let out: Mutex<Vec<(PathBuf, VerifyVerdict, Option<String>)>> =
        Mutex::new(Vec::with_capacity(items.len()));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some((path, format)) = items.get(i) else {
                        break;
                    };
                    let (verdict, detail) = match verify_file(path, format.as_deref()) {
                        Ok(r) => r,
                        Err(e) => (VerifyVerdict::Corrupt, Some(e.to_string())),
                    };
                    out.lock().unwrap().push((path.clone(), verdict, detail));
                }
            });
        }
    });

    out.into_inner().unwrap()
}

/// Whether `flac` is on `PATH` (so the CLI can warn helpfully up front).
pub fn flac_available() -> bool {
    Command::new("flac")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Whether `ffmpeg` is on `PATH`.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flac_clean_exit_is_ok() {
        assert_eq!(classify_flac(true, ""), (VerifyVerdict::Ok, None));
        // A benign tag note on a clean exit does not downgrade.
        assert_eq!(
            classify_flac(true, "flac: WARNING: skipping unknown ID3v2 tag"),
            (VerifyVerdict::Ok, None)
        );
    }

    #[test]
    fn flac_nonzero_exit_is_corrupt_with_detail() {
        let (v, d) = classify_flac(
            false,
            "\nfile.flac: ERROR while decoding data\nstate = FLAC__STREAM_DECODER_ERROR",
        );
        assert_eq!(v, VerifyVerdict::Corrupt);
        assert_eq!(d.as_deref(), Some("file.flac: ERROR while decoding data"));
    }

    #[test]
    fn ffmpeg_clean_is_ok() {
        assert_eq!(classify_ffmpeg(true, ""), (VerifyVerdict::Ok, None));
        assert_eq!(
            classify_ffmpeg(true, "   \n  \n"),
            (VerifyVerdict::Ok, None)
        );
    }

    #[test]
    fn ffmpeg_hard_error_is_corrupt() {
        let (v, _) = classify_ffmpeg(true, "[mp3 @ 0x..] Error while decoding stream");
        assert_eq!(v, VerifyVerdict::Corrupt);
        // A non-zero exit alone (e.g. could not open) is corrupt.
        let (v2, _) = classify_ffmpeg(false, "");
        assert_eq!(v2, VerifyVerdict::Corrupt);
        // "Invalid data found when processing input" is the classic header break.
        let (v3, d3) = classify_ffmpeg(
            true,
            "[matroska @ 0x..] Invalid data found when processing input",
        );
        assert_eq!(v3, VerifyVerdict::Corrupt);
        assert!(d3.unwrap().contains("Invalid data"));
    }

    #[test]
    fn ffmpeg_trailing_is_suspect() {
        let (v, d) = classify_ffmpeg(true, "[mp3 @ 0x..] Trailing data found in the stream");
        assert_eq!(v, VerifyVerdict::Suspect);
        assert!(d.unwrap().contains("Trailing"));
    }

    #[test]
    fn ffmpeg_benign_note_is_metadata() {
        // Non-empty stderr that is neither a hard error nor a suspect marker:
        // audio decoded, only a container/tag note survived.
        let (v, _) = classify_ffmpeg(true, "[mp4 @ 0x..] multiple edit list entries, a/v desync");
        assert_eq!(v, VerifyVerdict::Metadata);
    }

    #[test]
    fn ffmpeg_bitrate_note_is_ok_not_metadata() {
        // The benign "estimating duration" note every clean mp3 emits must not
        // flag the file (the e2e regression that motivated the allowlist).
        let (v, d) = classify_ffmpeg(
            true,
            "[mp3 @ 0x..] Estimating duration from bitrate, this may be inaccurate",
        );
        assert_eq!(v, VerifyVerdict::Ok);
        assert_eq!(d, None);
        // Benign note alongside a real error: the error still wins.
        let (v2, _) = classify_ffmpeg(
            true,
            "[mp3 @ 0x..] Estimating duration from bitrate\n[mp3 @ 0x..] Error while decoding stream",
        );
        assert_eq!(v2, VerifyVerdict::Corrupt);
    }

    #[test]
    fn verdict_string_round_trips() {
        for v in [
            VerifyVerdict::Ok,
            VerifyVerdict::Metadata,
            VerifyVerdict::Suspect,
            VerifyVerdict::Corrupt,
        ] {
            assert_eq!(v.as_str().parse(), Ok(v));
        }
        assert!("bogus".parse::<VerifyVerdict>().is_err());
    }

    #[test]
    fn flac_detection() {
        assert!(is_flac(Path::new("/x/y.flac"), None));
        assert!(is_flac(Path::new("/x/y.FLAC"), None));
        assert!(is_flac(Path::new("/x/y.mp3"), Some("flac"))); // format wins
        assert!(!is_flac(Path::new("/x/y.mp3"), Some("mp3")));
        assert!(!is_flac(Path::new("/x/y.opus"), None));
    }
}
