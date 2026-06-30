//! Stray APEv2 detection and byte-level stripping (Phase 8c-iii, roadmap
//! "Phase 8 / 8c").
//!
//! A stray APEv2 tag on an MP3 shadows ID3 in foobar2000 / DeaDBeeF and
//! silently defeats tag edits. lofty reads APE on MPEG but cannot write or
//! remove it (the Phase 5b deferral), so removing one needs byte surgery: this
//! module is the hand-rolled port of Lattice's `apestrip` technique.
//!
//! This file holds the pure parser (locate / validate / excise). The mutating
//! `strip_file` / restore helpers and the `ape_strips` undo journal are the
//! second 8c-iii commit.
//!
//! ## APEv2 layout (trailing tag)
//!
//! optional 32-byte header, then items, then a mandatory 32-byte footer, then
//! optionally a 128-byte ID3v1 trailer. The 32-byte preamble (header and footer
//! share it): `"APETAGEX"` (8) + version u32 LE + **size** u32 LE (items +
//! footer, excludes the header) + item count u32 LE + flags u32 LE + 8 reserved
//! zero bytes. Flags bit 31 = "this is the header"; bit 29 = "tag has a header".

/// The APEv2 preamble magic.
const MAGIC: &[u8; 8] = b"APETAGEX";
/// Size of the header / footer preamble, in bytes.
const PREAMBLE: usize = 32;
/// Flag bit 31: the preamble is the header (clear on a footer).
const FLAG_IS_HEADER: u32 = 0x8000_0000;
/// Flag bit 29: the tag carries a header (before the items).
const FLAG_HAS_HEADER: u32 = 0x2000_0000;
/// Sanity ceiling on a tag's declared size (64 MiB); guards against a stray
/// `"APETAGEX"` in audio being read as a gigantic tag.
const MAX_TAG_SIZE: u32 = 64 * 1024 * 1024;

/// A parsed APE preamble (header or footer).
#[derive(Debug, Clone, Copy)]
struct Preamble {
    /// Declared bytes of items + footer (excludes the header).
    size: u32,
    count: u32,
    is_header: bool,
    has_header: bool,
}

/// Parse a 32-byte slice as an APE preamble. `None` unless the magic matches,
/// the 8 reserved bytes are zero (spec-mandated; the key guard against a random
/// match in audio), and the declared size is in `[PREAMBLE, MAX_TAG_SIZE]`.
fn parse_preamble(b: &[u8]) -> Option<Preamble> {
    if b.len() < PREAMBLE || &b[..8] != MAGIC {
        return None;
    }
    // bytes 24..32 are reserved and must be zero.
    if b[24..32].iter().any(|&x| x != 0) {
        return None;
    }
    let size = u32::from_le_bytes(b[12..16].try_into().ok()?);
    let count = u32::from_le_bytes(b[16..20].try_into().ok()?);
    let flags = u32::from_le_bytes(b[20..24].try_into().ok()?);
    if !(PREAMBLE as u32..=MAX_TAG_SIZE).contains(&size) {
        return None;
    }
    Some(Preamble {
        size,
        count,
        is_header: flags & FLAG_IS_HEADER != 0,
        has_header: flags & FLAG_HAS_HEADER != 0,
    })
}

/// Where a trailing ID3v1 tag begins (its 128-byte block starts with `"TAG"`),
/// or the end of the data if there is none. APE precedes ID3v1.
fn id3v1_start(data: &[u8]) -> usize {
    if data.len() >= 128 && &data[data.len() - 128..data.len() - 125] == b"TAG" {
        data.len() - 128
    } else {
        data.len()
    }
}

/// Cheap detection over a file *tail*: is there a well-formed trailing APE
/// footer? Reads the last 32 bytes before any ID3v1 trailer and validates the
/// footer (magic, reserved-zero, footer flag clear). Used by the `audit` ape
/// tier, which only reads the tail of each MP3, not the whole file.
pub fn has_ape(tail: &[u8]) -> bool {
    let end = id3v1_start(tail);
    if end < PREAMBLE {
        return false;
    }
    match parse_preamble(&tail[end - PREAMBLE..end]) {
        Some(f) => !f.is_header,
        None => false,
    }
}

/// The byte span of a trailing APE tag within a *full* file buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApeSpan {
    /// First byte of the APE tag (the header if present, else the items).
    pub tag_start: usize,
    /// One past the last APE byte (the footer's end; the ID3v1 start or EOF).
    pub tag_end: usize,
    pub item_count: u32,
    pub has_header: bool,
}

impl ApeSpan {
    pub fn len(&self) -> usize {
        self.tag_end - self.tag_start
    }

    pub fn is_empty(&self) -> bool {
        self.tag_end == self.tag_start
    }
}

/// Locate a well-formed trailing APE tag in a full file buffer, for stripping.
/// Anchors on the footer (the 32 bytes before any ID3v1 trailer), derives the
/// tag start from the footer's declared size, and (when the footer claims a
/// header) verifies the header preamble actually sits at the derived start.
/// That consistency check is the safety crux: it proves `tag_start` is a real
/// tag boundary, not a stray `"APETAGEX"` in the audio.
pub fn locate_ape(data: &[u8]) -> Option<ApeSpan> {
    let end = id3v1_start(data);
    if end < PREAMBLE {
        return None;
    }
    let footer = parse_preamble(&data[end - PREAMBLE..end])?;
    if footer.is_header {
        return None; // a footer must not have the header flag set
    }
    // items + footer = footer.size; the body (items) starts size bytes back.
    let body_start = end.checked_sub(footer.size as usize)?;
    let tag_start = if footer.has_header {
        body_start.checked_sub(PREAMBLE)?
    } else {
        body_start
    };
    if footer.has_header {
        let header = parse_preamble(data.get(tag_start..tag_start + PREAMBLE)?)?;
        if !header.is_header || header.size != footer.size {
            return None;
        }
    }
    Some(ApeSpan {
        tag_start,
        tag_end: end,
        item_count: footer.count,
        has_header: footer.has_header,
    })
}

/// The file with the APE tag excised: `data[..tag_start] + data[tag_end..]`
/// (keeps the ID3v2 + audio prefix and any ID3v1 trailer).
pub fn strip_bytes(data: &[u8], span: &ApeSpan) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() - span.len());
    out.extend_from_slice(&data[..span.tag_start]);
    out.extend_from_slice(&data[span.tag_end..]);
    out
}

// --- Mutating helpers (Phase 8c-iii strip, commit 2) ---

use std::io;
use std::path::{Path, PathBuf};

/// Everything the `apestrip` verb needs for one file: the bytes to write, the
/// excised tag (for the undo journal), and the pre-strip identity.
#[derive(Debug, Clone)]
pub struct StripPlan {
    /// The new file content, with the APE tag removed.
    pub stripped: Vec<u8>,
    /// The excised APE tag bytes, stored for an exact undo.
    pub ape_bytes: Vec<u8>,
    pub tag_start: usize,
    pub item_count: u32,
    pub orig_size: u64,
    pub orig_mtime: i64,
}

/// Read a file and plan its strip. `Ok(None)` when the file carries no valid
/// trailing APE (nothing to do). Pure read; writes nothing.
pub fn plan_strip(abs: &Path) -> io::Result<Option<StripPlan>> {
    let data = std::fs::read(abs)?;
    let meta = std::fs::metadata(abs)?;
    let Some(span) = locate_ape(&data) else {
        return Ok(None);
    };
    Ok(Some(StripPlan {
        stripped: strip_bytes(&data, &span),
        ape_bytes: data[span.tag_start..span.tag_end].to_vec(),
        tag_start: span.tag_start,
        item_count: span.item_count,
        orig_size: data.len() as u64,
        orig_mtime: mtime_secs(&meta),
    }))
}

/// Write `stripped` over `abs`, crash-safe: a sibling temp file, fsync, a
/// decode check (lofty must still read it as a valid MPEG), then an atomic
/// rename. Refuses (and leaves the original untouched) if the new bytes still
/// contain an APE or fail to decode.
pub fn commit_strip(abs: &Path, stripped: &[u8]) -> io::Result<()> {
    if locate_ape(stripped).is_some() {
        return Err(io::Error::other("strip left an APE tag in place"));
    }
    write_atomic_verified(abs, stripped)
}

/// Re-insert `ape_bytes` at `tag_start` of the current file content (the undo
/// inverse of `strip_bytes`).
pub fn restore_bytes(current: &[u8], ape_bytes: &[u8], tag_start: usize) -> Option<Vec<u8>> {
    if tag_start > current.len() {
        return None;
    }
    let mut out = Vec::with_capacity(current.len() + ape_bytes.len());
    out.extend_from_slice(&current[..tag_start]);
    out.extend_from_slice(ape_bytes);
    out.extend_from_slice(&current[tag_start..]);
    Some(out)
}

/// Write `bytes` over `abs` via a sibling temp file + fsync + lofty decode
/// check + atomic rename + size verify. The original is replaced only if the
/// new file is fully written and decodes; on any error the temp is removed and
/// the original is untouched.
pub fn write_atomic_verified(abs: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(abs, bytes, true)
}

/// Atomic in-place rewrite without the decode check, for restoring the original
/// (possibly malformed) APE tag on undo: the bytes are exactly what was there
/// before, so requiring them to decode would be wrong (the malformed tag is
/// often why the file was stripped).
pub fn write_atomic_plain(abs: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(abs, bytes, false)
}

/// Sibling temp file + fsync + (optional) decode check + atomic rename + size
/// verify. The original is replaced only on full success; any error removes the
/// temp and leaves the original untouched.
fn write_atomic(abs: &Path, bytes: &[u8], verify_decode: bool) -> io::Result<()> {
    let temp = temp_sibling(abs);
    let result = (|| -> io::Result<()> {
        std::fs::write(&temp, bytes)?;
        let mut f = std::fs::File::open(&temp)?;
        f.sync_all()?;
        // Content-based decode check (lofty reads from the handle, so it is
        // robust to the temp file's non-audio extension).
        if verify_decode && lofty::read_from(&mut f).is_err() {
            return Err(io::Error::other(
                "decode check failed; refusing to replace the original",
            ));
        }
        drop(f);
        std::fs::rename(&temp, abs)?;
        tracing::debug!(target: "conservatory::io", path = %abs.display(), bytes = bytes.len(), "ape: strip + rename");
        let len = std::fs::metadata(abs)?.len();
        if len != bytes.len() as u64 {
            return Err(io::Error::other(format!(
                "size mismatch after write: {len} != {}",
                bytes.len()
            )));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// A temp path beside the target, on the same filesystem so the rename is atomic.
fn temp_sibling(abs: &Path) -> PathBuf {
    let mut s = abs.as_os_str().to_owned();
    s.push(".conservatory-apestrip");
    PathBuf::from(s)
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preamble(size: u32, count: u32, is_header: bool, has_header: bool) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[..8].copy_from_slice(MAGIC);
        b[8..12].copy_from_slice(&2000u32.to_le_bytes());
        b[12..16].copy_from_slice(&size.to_le_bytes());
        b[16..20].copy_from_slice(&count.to_le_bytes());
        let mut flags = 0u32;
        if is_header {
            flags |= FLAG_IS_HEADER;
        }
        if has_header {
            flags |= FLAG_HAS_HEADER;
        }
        b[20..24].copy_from_slice(&flags.to_le_bytes());
        b
    }

    /// items + footer, optionally with a leading header.
    fn build_ape(items: &[u8], with_header: bool) -> Vec<u8> {
        let size = (items.len() + PREAMBLE) as u32;
        let mut out = Vec::new();
        if with_header {
            out.extend_from_slice(&preamble(size, 3, true, true));
        }
        out.extend_from_slice(items);
        out.extend_from_slice(&preamble(size, 3, false, with_header));
        out
    }

    fn prefix() -> Vec<u8> {
        // A fake ID3v2 + "audio" prefix that the strip must preserve.
        let mut p = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
        p.extend_from_slice(&[0xAB; 200]);
        p
    }

    #[test]
    fn locate_and_strip_footer_only() {
        let pre = prefix();
        let mut data = pre.clone();
        data.extend_from_slice(&build_ape(b"item-bytes", false));

        let span = locate_ape(&data).expect("ape present");
        assert_eq!(span.tag_start, pre.len());
        assert_eq!(span.tag_end, data.len());
        assert!(!span.has_header);
        assert_eq!(strip_bytes(&data, &span), pre);
        assert!(has_ape(&data));
    }

    #[test]
    fn locate_with_header() {
        let pre = prefix();
        let mut data = pre.clone();
        data.extend_from_slice(&build_ape(b"more-items", true));

        let span = locate_ape(&data).expect("ape present");
        assert_eq!(span.tag_start, pre.len());
        assert!(span.has_header);
        assert_eq!(strip_bytes(&data, &span), pre);
    }

    #[test]
    fn preserves_trailing_id3v1() {
        let pre = prefix();
        let mut data = pre.clone();
        data.extend_from_slice(&build_ape(b"items", false));
        let mut id3v1 = b"TAG".to_vec();
        id3v1.extend_from_slice(&[0u8; 125]); // 128 total
        data.extend_from_slice(&id3v1);

        let span = locate_ape(&data).expect("ape present");
        assert_eq!(span.tag_end, data.len() - 128);
        let stripped = strip_bytes(&data, &span);
        let mut expected = pre.clone();
        expected.extend_from_slice(&id3v1);
        assert_eq!(stripped, expected, "ID3v1 trailer is kept");
    }

    #[test]
    fn no_ape_when_signature_only_in_audio() {
        // "APETAGEX" buried in the audio, with no valid trailing footer.
        let mut data = prefix();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&[0x11; 64]);
        assert!(!has_ape(&data));
        assert!(locate_ape(&data).is_none());
    }

    #[test]
    fn header_consistency_check_rejects_garbage() {
        // A footer that claims a header, but the derived start is not a header.
        let pre = prefix();
        let mut data = pre.clone();
        let items = vec![0x22u8; 40];
        data.extend_from_slice(&items);
        // footer says has_header, size = items + footer (no real header present).
        data.extend_from_slice(&preamble((items.len() + PREAMBLE) as u32, 3, false, true));
        assert!(locate_ape(&data).is_none(), "no real header => refused");
    }
}
