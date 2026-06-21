//! ReplayGain 2.0 scanning via `rsgain` (Phase 5c, spec §16.7).
//!
//! `rsgain` (libebur128) decodes every format the library uses — including Opus,
//! which a pure-Rust decoder cannot — computes album + track gain/peak, and
//! writes the correct format-specific tags itself (ID3 TXXX, Vorbis, Opus R128,
//! MP4). We shell out per album, then re-read the files to refresh the DB
//! `replaygain_*` columns the player's profile resolution consults. rsgain is an
//! external tool (ATTRIBUTIONS.md), the Lattice `scripts/replaygain.py` lineage.

use std::path::Path;
use std::process::Command;

use crate::errors::{Error, Result};
use crate::tags::read_track;

/// The ReplayGain 2.0 reference loudness (rsgain's default).
pub const DEFAULT_TARGET_LUFS: f64 = -18.0;

/// Run an `rsgain` album scan over `files` (the tracks of one album), writing
/// RG2.0 tags in place. `-a` album gain, `-s i` write tags, `-c p` clip
/// protection, `-l` target loudness (the Lattice invocation).
pub fn scan_album_files<P: AsRef<Path>>(files: &[P], target_lufs: f64) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    let mut cmd = Command::new("rsgain");
    cmd.arg("custom")
        .arg("-q")
        .arg("-a")
        .arg("-s")
        .arg("i")
        .arg("-c")
        .arg("p")
        .arg("-l")
        .arg(format!("{target_lufs}"));
    for f in files {
        cmd.arg(f.as_ref());
    }
    let status = cmd
        .status()
        .map_err(|e| Error::ReplayGain(format!("running rsgain (is it installed?): {e}")))?;
    if !status.success() {
        return Err(Error::ReplayGain(format!("rsgain exited with {status}")));
    }
    Ok(())
}

/// Read the (track gain, album gain) currently in a file's tags, for syncing the
/// DB after a scan. Hermetic (no rsgain): reuses the tag reader.
pub fn replaygain_from_file(path: &Path) -> Result<(Option<f64>, Option<f64>)> {
    let draft = read_track(path)?;
    Ok((draft.replaygain_track, draft.replaygain_album))
}

/// Whether `rsgain` is on `PATH` (so the CLI/GUI can fail with a helpful message
/// rather than a raw spawn error).
pub fn rsgain_available() -> bool {
    Command::new("rsgain")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
