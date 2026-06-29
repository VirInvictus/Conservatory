//! Library health audits (Phase 8c, roadmap "Phase 8 / 8c").
//!
//! Five read-only checks ported from Lattice's `--auditTags` / `--auditBitrate`
//! / `--auditReplayGain` / `--missingArt` / `--auditArtQuality`
//! (`~/.gitrepos/Lattice/src/lattice/modes/audit.py`), run DB-canonical over the
//! managed DB and the cover files. Report-only: any fix goes through a separate
//! verb (the ReplayGain rescan, the cover setter, the Phase 2c mover), never here.
//!
//! The tag/bitrate/ReplayGain logic is pure over the [`AuditTrackRow`] /
//! [`AuditAlbumRow`] reads and unit-tested; the cover-art checks and the Opus
//! R128 fallback touch the filesystem and are isolated behind an `Option<&Path>`
//! root so the rest stays testable without IO.

use std::collections::BTreeMap;
use std::path::Path;

use crate::db::{AuditAlbumRow, AuditTrackRow};

/// Formats whose bitrate floor is meaningless (a lossless file is by definition
/// fine), excluded from the bitrate audit so they are never false-flagged.
pub const LOSSLESS_FORMATS: &[&str] = &["flac", "alac", "wav", "aiff", "ape", "wavpack", "wv"];

/// Default bitrate floor, in kbps (`tracks.bitrate` is stored in kbps).
pub const DEFAULT_BITRATE_FLOOR: u32 = 192;

/// Default cover-art pixel floor (width, height).
pub const DEFAULT_MIN_ART_PX: (u32, u32) = (500, 500);

/// Which critical tags a track is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagFlags {
    pub title: bool,
    pub artist: bool,
    pub track_no: bool,
    pub genre: bool,
}

impl TagFlags {
    pub fn any(&self) -> bool {
        self.title || self.artist || self.track_no || self.genre
    }

    /// The missing-field names, for the report.
    pub fn labels(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.title {
            v.push("title");
        }
        if self.artist {
            v.push("artist");
        }
        if self.track_no {
            v.push("track#");
        }
        if self.genre {
            v.push("genre");
        }
        v
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagDeficiency {
    pub track_id: i64,
    pub file_path: String,
    pub missing: TagFlags,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitrateDeficiency {
    pub track_id: i64,
    pub file_path: String,
    pub format: Option<String>,
    pub bitrate: Option<u32>,
}

/// Per-album ReplayGain coverage (Lattice's four buckets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgBucket {
    /// No track carries track gain.
    Missing,
    /// Some tracks tagged, some bare (the worst case for playback).
    Partial,
    /// Every track has track gain, but not every track has album gain.
    NoAlbumGain,
    /// Every track has both track and album gain.
    Ok,
}

impl RgBucket {
    pub fn as_str(&self) -> &'static str {
        match self {
            RgBucket::Missing => "missing",
            RgBucket::Partial => "partial",
            RgBucket::NoAlbumGain => "no-album-gain",
            RgBucket::Ok => "ok",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgCoverage {
    pub album_id: i64,
    pub artist: String,
    pub title: String,
    pub bucket: RgBucket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtDeficiency {
    pub album_id: i64,
    pub artist: String,
    pub title: String,
    pub folder_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtResDeficiency {
    pub album_id: i64,
    pub artist: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
}

/// The full audit report; non-OK findings only.
#[derive(Debug, Clone, Default)]
pub struct AuditReport {
    pub missing_tags: Vec<TagDeficiency>,
    pub low_bitrate: Vec<BitrateDeficiency>,
    pub replaygain: Vec<RgCoverage>,
    pub missing_art: Vec<ArtDeficiency>,
    pub low_res_art: Vec<ArtResDeficiency>,
}

impl AuditReport {
    pub fn is_empty(&self) -> bool {
        self.missing_tags.is_empty()
            && self.low_bitrate.is_empty()
            && self.replaygain.is_empty()
            && self.missing_art.is_empty()
            && self.low_res_art.is_empty()
    }
}

/// Which tiers to run, and the two tunables.
#[derive(Debug, Clone)]
pub struct AuditOptions {
    pub tags: bool,
    pub bitrate: bool,
    pub replaygain: bool,
    pub art: bool,
    pub artres: bool,
    pub bitrate_floor: u32,
    pub min_art_px: (u32, u32),
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self {
            tags: true,
            bitrate: true,
            replaygain: true,
            art: true,
            artres: true,
            bitrate_floor: DEFAULT_BITRATE_FLOOR,
            min_art_px: DEFAULT_MIN_ART_PX,
        }
    }
}

fn is_lossless(format: Option<&str>) -> bool {
    format
        .map(|f| LOSSLESS_FORMATS.contains(&f.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Tier 1: tracks missing any critical tag.
pub fn audit_tags(rows: &[AuditTrackRow]) -> Vec<TagDeficiency> {
    rows.iter()
        .filter_map(|r| {
            let missing = TagFlags {
                title: r.title.trim().is_empty(),
                artist: r.artist.as_deref().map(str::trim).unwrap_or("").is_empty(),
                track_no: r.track_no.is_none(),
                genre: r.genre_count == 0,
            };
            missing.any().then(|| TagDeficiency {
                track_id: r.track_id,
                file_path: r.file_path.clone(),
                missing,
            })
        })
        .collect()
}

/// Tier 2: lossy tracks below the bitrate floor (lossless formats skipped, and
/// an unknown bitrate is not treated as below-floor).
pub fn audit_bitrate(rows: &[AuditTrackRow], floor: u32) -> Vec<BitrateDeficiency> {
    rows.iter()
        .filter(|r| !is_lossless(r.format.as_deref()))
        .filter(|r| matches!(r.bitrate, Some(b) if b < floor))
        .map(|r| BitrateDeficiency {
            track_id: r.track_id,
            file_path: r.file_path.clone(),
            format: r.format.clone(),
            bitrate: r.bitrate,
        })
        .collect()
}

/// Tier 3: per-album ReplayGain coverage (non-OK albums only). Run
/// [`resolve_r128`] first when a root is available so R128-only Opus is counted.
pub fn audit_replaygain(rows: &[AuditTrackRow], albums: &[AuditAlbumRow]) -> Vec<RgCoverage> {
    let labels: BTreeMap<i64, (&str, &str)> = albums
        .iter()
        .map(|a| {
            (
                a.album_id,
                (a.artist.as_deref().unwrap_or(""), a.title.as_str()),
            )
        })
        .collect();

    // Group track rows by album.
    let mut by_album: BTreeMap<i64, Vec<&AuditTrackRow>> = BTreeMap::new();
    for r in rows {
        if let Some(id) = r.album_id {
            by_album.entry(id).or_default().push(r);
        }
    }

    let mut out = Vec::new();
    for (album_id, tracks) in &by_album {
        let n = tracks.len();
        let t = tracks
            .iter()
            .filter(|r| r.replaygain_track.is_some())
            .count();
        let a = tracks
            .iter()
            .filter(|r| r.replaygain_album.is_some())
            .count();
        let bucket = if t == 0 {
            RgBucket::Missing
        } else if t < n {
            RgBucket::Partial
        } else if a < n {
            RgBucket::NoAlbumGain
        } else {
            RgBucket::Ok
        };
        if bucket != RgBucket::Ok {
            let (artist, title) = labels.get(album_id).copied().unwrap_or(("", ""));
            out.push(RgCoverage {
                album_id: *album_id,
                artist: artist.to_string(),
                title: title.to_string(),
                bucket,
            });
        }
    }
    out
}

/// Fill the ReplayGain presence of Opus tracks whose DB gain is NULL from the
/// file's raw `R128_*` tags (lofty does not surface them; see
/// [`crate::tags::read_r128_presence`]). A found R128 tag sets the column to a
/// presence sentinel (`Some(0.0)`) since the audit only checks presence, not
/// value. Mutates in place; best-effort (unreadable files are left as-is).
pub fn resolve_r128(rows: &mut [AuditTrackRow], root: &Path) {
    for r in rows.iter_mut() {
        let is_opus = r
            .format
            .as_deref()
            .map(|f| f.eq_ignore_ascii_case("opus"))
            .unwrap_or(false);
        if !is_opus || (r.replaygain_track.is_some() && r.replaygain_album.is_some()) {
            continue;
        }
        let abs = root.join(&r.file_path);
        let (has_track, has_album) = crate::tags::read_r128_presence(&abs);
        if r.replaygain_track.is_none() && has_track {
            r.replaygain_track = Some(0.0);
        }
        if r.replaygain_album.is_none() && has_album {
            r.replaygain_album = Some(0.0);
        }
    }
}

/// Tiers 4 + 5: missing and low-resolution cover art. Missing flags a NULL
/// `cover_path` always, and (when `root` is given) a recorded cover whose file
/// is absent. Low-res needs `root` and `check_res`: it decodes each present
/// cover's header dimensions and flags those below `min_px`.
pub fn audit_art(
    albums: &[AuditAlbumRow],
    root: Option<&Path>,
    min_px: (u32, u32),
    check_res: bool,
) -> (Vec<ArtDeficiency>, Vec<ArtResDeficiency>) {
    let mut missing = Vec::new();
    let mut low_res = Vec::new();
    for al in albums {
        let abs = root.and_then(|r| al.cover_path.as_deref().map(|c| r.join(c)));
        let present = match (&al.cover_path, &abs) {
            (None, _) => false,
            (Some(_), Some(p)) => p.exists(),
            (Some(_), None) => true, // recorded but no root to verify; assume present
        };
        if !present {
            missing.push(ArtDeficiency {
                album_id: al.album_id,
                artist: al.artist.clone().unwrap_or_default(),
                title: al.title.clone(),
                folder_path: al.folder_path.clone(),
            });
            continue;
        }
        if check_res
            && let Some(p) = &abs
            && let Some((w, h)) = cover_dimensions(p)
            && (w < min_px.0 || h < min_px.1)
        {
            low_res.push(ArtResDeficiency {
                album_id: al.album_id,
                artist: al.artist.clone().unwrap_or_default(),
                title: al.title.clone(),
                width: w,
                height: h,
            });
        }
    }
    (missing, low_res)
}

/// Read just the header dimensions of an image file (no full decode).
fn cover_dimensions(abs: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(abs)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Run the requested audit tiers. Takes the track rows by value so the Opus
/// R128 fallback can augment them before the ReplayGain bucketing.
pub fn run_audit(
    mut tracks: Vec<AuditTrackRow>,
    albums: &[AuditAlbumRow],
    opts: &AuditOptions,
    root: Option<&Path>,
) -> AuditReport {
    let mut report = AuditReport::default();
    if opts.tags {
        report.missing_tags = audit_tags(&tracks);
    }
    if opts.bitrate {
        report.low_bitrate = audit_bitrate(&tracks, opts.bitrate_floor);
    }
    if opts.replaygain {
        if let Some(root) = root {
            resolve_r128(&mut tracks, root);
        }
        report.replaygain = audit_replaygain(&tracks, albums);
    }
    if opts.art || opts.artres {
        let (missing, low_res) = audit_art(albums, root, opts.min_art_px, opts.artres);
        if opts.art {
            report.missing_art = missing;
        }
        if opts.artres {
            report.low_res_art = low_res;
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn trow(
        track_id: i64,
        album_id: i64,
        title: &str,
        artist: Option<&str>,
        track_no: Option<u32>,
        genre_count: i64,
        format: &str,
        bitrate: Option<u32>,
        rg_track: Option<f64>,
        rg_album: Option<f64>,
    ) -> AuditTrackRow {
        AuditTrackRow {
            track_id,
            album_id: Some(album_id),
            title: title.to_string(),
            artist: artist.map(str::to_string),
            track_no,
            genre_count,
            format: Some(format.to_string()),
            bitrate,
            replaygain_track: rg_track,
            replaygain_album: rg_album,
            file_path: format!("path/{track_id}.{format}"),
        }
    }

    fn album(album_id: i64, title: &str) -> AuditAlbumRow {
        AuditAlbumRow {
            album_id,
            artist: Some("Artist".into()),
            title: title.to_string(),
            cover_path: None,
            folder_path: format!("Music/Artist/{title}"),
        }
    }

    #[test]
    fn tags_flags_each_missing_field() {
        let rows = vec![
            trow(
                1,
                1,
                "Has all",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                2,
                1,
                "",
                Some("A"),
                Some(2),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                3,
                1,
                "No artist",
                None,
                Some(3),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                4,
                1,
                "No track#",
                Some("A"),
                None,
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                5,
                1,
                "No genre",
                Some("A"),
                Some(5),
                0,
                "mp3",
                Some(320),
                None,
                None,
            ),
        ];
        let def = audit_tags(&rows);
        assert_eq!(def.len(), 4, "the fully-tagged track is clean");
        assert!(def.iter().any(|d| d.track_id == 2 && d.missing.title));
        assert!(def.iter().any(|d| d.track_id == 3 && d.missing.artist));
        assert!(def.iter().any(|d| d.track_id == 4 && d.missing.track_no));
        assert!(def.iter().any(|d| d.track_id == 5 && d.missing.genre));
    }

    #[test]
    fn bitrate_skips_lossless_and_unknown() {
        let rows = vec![
            trow(
                1,
                1,
                "Low mp3",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(128),
                None,
                None,
            ),
            trow(
                2,
                1,
                "Fine mp3",
                Some("A"),
                Some(2),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                3,
                1,
                "FLAC",
                Some("A"),
                Some(3),
                1,
                "flac",
                Some(50),
                None,
                None,
            ),
            trow(
                4,
                1,
                "Unknown",
                Some("A"),
                Some(4),
                1,
                "mp3",
                None,
                None,
                None,
            ),
        ];
        let def = audit_bitrate(&rows, 192);
        assert_eq!(def.len(), 1);
        assert_eq!(def[0].track_id, 1, "only the low lossy track flags");
    }

    #[test]
    fn replaygain_buckets() {
        let albums = vec![
            album(1, "Missing"),
            album(2, "Partial"),
            album(3, "NoAlbumGain"),
            album(4, "Ok"),
        ];
        let rows = vec![
            // album 1: no track gain anywhere -> Missing
            trow(
                1,
                1,
                "a",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            trow(
                2,
                1,
                "b",
                Some("A"),
                Some(2),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            // album 2: one tagged, one bare -> Partial
            trow(
                3,
                2,
                "a",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(320),
                Some(-6.0),
                Some(-7.0),
            ),
            trow(
                4,
                2,
                "b",
                Some("A"),
                Some(2),
                1,
                "mp3",
                Some(320),
                None,
                None,
            ),
            // album 3: both have track gain, one lacks album gain -> NoAlbumGain
            trow(
                5,
                3,
                "a",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(320),
                Some(-6.0),
                Some(-7.0),
            ),
            trow(
                6,
                3,
                "b",
                Some("A"),
                Some(2),
                1,
                "mp3",
                Some(320),
                Some(-6.0),
                None,
            ),
            // album 4: complete -> Ok (not reported)
            trow(
                7,
                4,
                "a",
                Some("A"),
                Some(1),
                1,
                "mp3",
                Some(320),
                Some(-6.0),
                Some(-7.0),
            ),
        ];
        let cov = audit_replaygain(&rows, &albums);
        assert_eq!(cov.len(), 3, "the OK album is not reported");
        let bucket = |id: i64| cov.iter().find(|c| c.album_id == id).map(|c| c.bucket);
        assert_eq!(bucket(1), Some(RgBucket::Missing));
        assert_eq!(bucket(2), Some(RgBucket::Partial));
        assert_eq!(bucket(3), Some(RgBucket::NoAlbumGain));
        assert_eq!(bucket(4), None);
    }

    #[test]
    fn art_missing_when_cover_path_null() {
        let albums = vec![album(1, "NoCover")];
        let (missing, low_res) = audit_art(&albums, None, (500, 500), false);
        assert_eq!(missing.len(), 1);
        assert!(low_res.is_empty());
    }
}
