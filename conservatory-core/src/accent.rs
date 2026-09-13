//! Cover-art accent extraction (spec §7.4, roadmap Phase 1c).
//!
//! A faithful port of Hermitage's `colors.py`: resize the cover to 64x64, run a
//! median-cut quantizer down to a small palette, then pick the **most vibrant**
//! colour (max saturation x value in HSV), not the most populous. The result is
//! a packed `0x00RRGGBB` accent for the browse and Now Playing surfaces.
//!
//! This is the same algorithm *family* as Pillow's quantizer, not byte-identical
//! (Pillow quantizes via libimagequant internally); fidelity is to the approach,
//! and the tests use unambiguous covers. See `docs/accent.md`.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;

use crate::errors::Result;
use crate::tags::TrackDraft;

const SAMPLE_SIZE: u32 = 64; // downsample edge before analysis (Hermitage SAMPLE_SIZE)
const NUM_COLORS: usize = 5; // palette size to quantize down to (Hermitage NUM_COLORS)

/// Compute the packed `0x00RRGGBB` accent for an encoded cover image.
pub fn compute_accent(bytes: &[u8]) -> Result<u32> {
    let img = image::load_from_memory(bytes)?.to_rgb8();
    let small = image::imageops::resize(&img, SAMPLE_SIZE, SAMPLE_SIZE, FilterType::Lanczos3);

    let pixels: Vec<[u8; 3]> = small.pixels().map(|p| p.0).collect();
    let palette = median_cut(pixels, NUM_COLORS);

    // Most vibrant wins (Hermitage `_sort_by_vibrancy`); first on ties for
    // determinism. `palette` is always non-empty for a decodable image.
    let mut best = palette[0];
    let mut best_score = vibrancy(best);
    for &c in &palette[1..] {
        let score = vibrancy(c);
        if score > best_score {
            best_score = score;
            best = c;
        }
    }

    Ok(pack(best))
}

/// Where an album's cover bytes came from. A sidecar carries its source path
/// so a move-mode import can journal the file itself into the managed tree
/// (and undo restores it); embedded art has no source file to consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverSource {
    /// Art embedded in the audio file's tags.
    Embedded(Vec<u8>),
    /// A sibling cover file in the source directory (e.g. `cover.jpg`).
    Sidecar { path: PathBuf, bytes: Vec<u8> },
}

impl CoverSource {
    /// The cover bytes, whichever kind this is.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Embedded(bytes) | Self::Sidecar { bytes, .. } => bytes,
        }
    }
}

/// Locate the cover source for a draft: the embedded picture if present, else a
/// sibling cover file in the source directory. Feeds the accent, the on-disk
/// cover write, and (for a sidecar) the move journal.
pub fn find_cover_source(source: &Path, draft: &TrackDraft) -> Option<CoverSource> {
    if let Some(cover) = &draft.cover {
        return Some(CoverSource::Embedded(cover.data.clone()));
    }
    let dir = source.parent()?;
    const CANDIDATES: &[&str] = &[
        "cover.jpg",
        "cover.jpeg",
        "cover.png",
        "folder.jpg",
        "folder.png",
        "front.jpg",
        "front.png",
    ];
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_lowercase();
        if CANDIDATES.contains(&name.as_str())
            && let Ok(bytes) = std::fs::read(entry.path())
        {
            return Some(CoverSource::Sidecar {
                path: entry.path(),
                bytes,
            });
        }
    }
    None
}

/// Locate the cover bytes for a draft: the embedded picture if present, else a
/// sibling cover file in the source directory. Storing a canonical `cover.jpg`
/// into the managed tree is Phase 2's job; this only finds the bytes to analyze.
pub fn find_cover_bytes(source: &Path, draft: &TrackDraft) -> Option<Vec<u8>> {
    find_cover_source(source, draft).map(|src| src.bytes().to_vec())
}

/// Median-cut quantization to at most `n` representative colours.
///
/// Repeatedly split the box with the widest single-channel spread at its median
/// along that channel, until `n` boxes exist or no box can be split further (a
/// box of identical pixels, e.g. a solid-colour cover, yields one colour). Each
/// box's representative is its per-channel mean.
fn median_cut(pixels: Vec<[u8; 3]>, n: usize) -> Vec<[u8; 3]> {
    if pixels.is_empty() {
        return Vec::new();
    }
    let mut boxes = vec![pixels];

    while boxes.len() < n {
        // Pick the box with the widest channel range; stop if none is splittable.
        let mut target = None;
        let mut widest = 0u16;
        for (i, b) in boxes.iter().enumerate() {
            let (channel, range) = widest_channel(b);
            if range as u16 > widest {
                widest = range as u16;
                target = Some((i, channel));
            }
        }
        let Some((idx, channel)) = target else { break };

        let mut b = boxes.swap_remove(idx);
        // Stable sort keeps a deterministic order for equal channel values.
        b.sort_by_key(|px| px[channel]);
        let mid = b.len() / 2;
        let hi = b.split_off(mid);
        boxes.push(b);
        boxes.push(hi);
    }

    boxes.iter().map(|b| mean(b)).collect()
}

/// The channel (0=R,1=G,2=B) with the largest max-min spread, and that spread.
fn widest_channel(pixels: &[[u8; 3]]) -> (usize, u8) {
    let mut best_channel = 0;
    let mut best_range = 0u8;
    for channel in 0..3 {
        let mut min = u8::MAX;
        let mut max = u8::MIN;
        for px in pixels {
            min = min.min(px[channel]);
            max = max.max(px[channel]);
        }
        let range = max - min;
        if range > best_range {
            best_range = range;
            best_channel = channel;
        }
    }
    (best_channel, best_range)
}

fn mean(pixels: &[[u8; 3]]) -> [u8; 3] {
    let n = pixels.len() as u64;
    let mut sum = [0u64; 3];
    for px in pixels {
        for c in 0..3 {
            sum[c] += px[c] as u64;
        }
    }
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// Vibrancy = saturation x value (HSV), Hermitage's ranking key.
fn vibrancy(rgb: [u8; 3]) -> f32 {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let value = max;
    let saturation = if max == 0.0 { 0.0 } else { (max - min) / max };
    saturation * value
}

fn pack(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | (rgb[2] as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    /// Encode an `RgbImage` to in-memory PNG bytes (no committed fixture needed).
    fn png_bytes(img: &RgbImage) -> Vec<u8> {
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn solid_colour_yields_that_colour() {
        let img = RgbImage::from_pixel(32, 32, Rgb([0x20, 0xC0, 0x40]));
        assert_eq!(compute_accent(&png_bytes(&img)).unwrap(), 0x20C040);
    }

    #[test]
    fn vibrant_region_beats_dominant_grey() {
        // Mostly mid-grey with a small pure-red block: population favours grey,
        // vibrancy favours red. Hermitage picks red.
        let mut img = RgbImage::from_pixel(64, 64, Rgb([0x80, 0x80, 0x80]));
        for y in 0..16 {
            for x in 0..16 {
                img.put_pixel(x, y, Rgb([0xFF, 0x00, 0x00]));
            }
        }
        assert_eq!(compute_accent(&png_bytes(&img)).unwrap(), 0xFF0000);
    }

    #[test]
    fn extraction_is_deterministic() {
        let mut img = RgbImage::from_pixel(48, 48, Rgb([0x10, 0x30, 0x90]));
        for y in 0..48 {
            img.put_pixel(y % 48, y, Rgb([0xE0, 0x10, 0x70]));
        }
        let bytes = png_bytes(&img);
        assert_eq!(
            compute_accent(&bytes).unwrap(),
            compute_accent(&bytes).unwrap()
        );
    }
}
