//! Embedded-chapter read via an `ffprobe` shell-out (Phase 7a-ii).
//!
//! lofty 0.21 cannot read MP4/M4B chapter atoms (confirmed). Rather than add a
//! Rust MP4-box dependency, the chapter resolver shells `ffprobe` for the chapter
//! list, the way `conservatory-core/src/replaygain.rs` shells `rsgain`: ffmpeg is
//! already installed, libmpv already links it, and it is the `m4b-tool` reference
//! technique (ATTRIBUTIONS.md). When `ffprobe` is absent the resolver degrades to
//! a whole-file single chapter, so a missing binary never aborts a read.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::error::{ReadError, Result};

/// One chapter as ffprobe reports it. Times are absolute seconds into the file.
#[derive(Debug, Clone, PartialEq)]
pub struct RawChapter {
    pub start: f64,
    pub end: f64,
    pub title: Option<String>,
}

/// Whether `ffprobe` is on PATH (the `rsgain_available` shape).
pub fn ffprobe_available() -> bool {
    Command::new("ffprobe")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Read a file's embedded chapter markers. An empty list (the common case for a
/// chapterless file) is `Ok(vec![])`; an absent binary is
/// [`ReadError::FfprobeMissing`], which the resolver treats as "no chapters".
pub fn probe_chapters(path: &Path) -> Result<Vec<RawChapter>> {
    let output = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_chapters"])
        .arg(path)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ReadError::FfprobeMissing
            } else {
                ReadError::Ffprobe(format!("running ffprobe: {e}"))
            }
        })?;
    if !output.status.success() {
        return Err(ReadError::Ffprobe(format!(
            "ffprobe exited with {}",
            output.status
        )));
    }
    let doc: ProbeDoc = serde_json::from_slice(&output.stdout)
        .map_err(|e| ReadError::Ffprobe(format!("parsing ffprobe json: {e}")))?;
    Ok(doc.chapters.into_iter().filter_map(to_raw).collect())
}

/// Build a [`RawChapter`] from one ffprobe chapter, dropping any whose start time
/// does not parse (ffprobe reports the times as decimal-second strings).
fn to_raw(c: ProbeChapter) -> Option<RawChapter> {
    let start = c.start_time.as_deref()?.parse().ok()?;
    let end = c
        .end_time
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(start);
    Some(RawChapter {
        start,
        end,
        title: c.tags.and_then(|t| t.title),
    })
}

#[derive(Deserialize)]
struct ProbeDoc {
    #[serde(default)]
    chapters: Vec<ProbeChapter>,
}

#[derive(Deserialize)]
struct ProbeChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    tags: Option<ProbeTags>,
}

#[derive(Deserialize)]
struct ProbeTags {
    title: Option<String>,
}
