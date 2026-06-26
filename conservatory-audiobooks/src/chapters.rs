//! Chapter resolver (Phase 7a-ii, spec §4.5, §6.1).
//!
//! Turns a book's audio file(s) into ordered [`ChapterDraft`]s, each addressing
//! either a standalone per-chapter file (`file_offset` 0) or a span inside one
//! M4B. Three cases (roadmap 7a):
//!
//! 1. one file with embedded chapters -> one draft per chapter (via `ffprobe`);
//! 2. one file with none -> a single whole-file chapter;
//! 3. a multi-file folder -> one draft per file, ordered by the part tag.
//!
//! The raw-chapter -> draft mapping and the multi-file ordering are pure (unit
//! tested with no audio); only the single-file path touches `ffprobe`.

use std::path::{Path, PathBuf};

use lofty::prelude::{Accessor, AudioFile, TaggedFileExt};

use crate::error::Result;
use crate::ffprobe::{self, RawChapter};

/// One resolved chapter, mapping 1:1 to a `book_chapters` row (spec §4.5).
/// `file_path` is the **source** file the reader saw; the importer (7a-iii)
/// rewrites it to the managed path after the move.
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterDraft {
    pub idx: i64,
    pub title: Option<String>,
    pub file_path: PathBuf,
    pub file_offset: f64,
    pub duration: Option<f64>,
}

/// Per-file metadata the resolver needs (read once via lofty).
struct FileMeta {
    part_no: Option<u32>,
    title: Option<String>,
    duration: Option<f64>,
}

fn file_meta(path: &Path) -> FileMeta {
    match lofty::read_from_path(path) {
        Ok(tagged) => {
            let duration = tagged.properties().duration().as_secs_f64();
            let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
            FileMeta {
                part_no: tag.and_then(|t| t.track()),
                title: tag.and_then(|t| t.title().map(|c| c.to_string())),
                duration: (duration > 0.0).then_some(duration),
            }
        }
        Err(_) => FileMeta {
            part_no: None,
            title: None,
            duration: None,
        },
    }
}

/// Resolve a book's chapters from its (already collected) audio files.
/// `book_title` lets the multi-file path drop a per-part title that merely
/// repeats the book title (a full-cast M4B tags every part with the book title)
/// in favour of a synthesized "Part N".
pub fn resolve_chapters(files: &[PathBuf], book_title: Option<&str>) -> Result<Vec<ChapterDraft>> {
    match files {
        [] => Ok(Vec::new()),
        [single] => Ok(resolve_single(single)),
        many => Ok(resolve_multi(many, book_title)),
    }
}

/// One file: embedded chapters if any, else the whole file as one chapter.
fn resolve_single(file: &Path) -> Vec<ChapterDraft> {
    let meta = file_meta(file);
    // ffprobe absent / chapterless / failing all collapse to "no chapters".
    let raw = ffprobe::probe_chapters(file).unwrap_or_default();
    if raw.is_empty() {
        return vec![ChapterDraft {
            idx: 0,
            title: None,
            file_path: file.to_path_buf(),
            file_offset: 0.0,
            duration: meta.duration,
        }];
    }
    chapters_from_raw(file, &raw)
}

/// Map ffprobe's chapter list to drafts inside a single file (pure).
fn chapters_from_raw(file: &Path, raw: &[RawChapter]) -> Vec<ChapterDraft> {
    raw.iter()
        .enumerate()
        .map(|(i, c)| ChapterDraft {
            idx: i as i64,
            title: c.title.clone(),
            file_path: file.to_path_buf(),
            file_offset: c.start,
            duration: Some((c.end - c.start).max(0.0)),
        })
        .collect()
}

/// Many files, one chapter each, ordered by the part tag (then the input order,
/// which the caller sorts by filename).
fn resolve_multi(files: &[PathBuf], book_title: Option<&str>) -> Vec<ChapterDraft> {
    let mut entries: Vec<(PathBuf, FileMeta)> =
        files.iter().map(|f| (f.clone(), file_meta(f))).collect();
    // A multi-part M4B carries `1/11 .. 11/11`, so a lexical filename sort
    // ("Part 10" before "Part 2") is wrong; order by the part tag when present,
    // falling back to the caller's filename order for ties / missing tags.
    entries.sort_by(|a, b| match (a.1.part_no, b.1.part_no) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.0.cmp(&b.0),
    });
    entries
        .into_iter()
        .enumerate()
        .map(|(i, (path, meta))| ChapterDraft {
            title: Some(chapter_title(&path, &meta, i, book_title)),
            idx: i as i64,
            file_path: path,
            file_offset: 0.0,
            duration: meta.duration,
        })
        .collect()
}

/// The per-file chapter title: the file's track title when it is meaningful
/// (present and not just the book title repeated), else a synthesized "Part N".
fn chapter_title(path: &Path, meta: &FileMeta, idx: usize, book_title: Option<&str>) -> String {
    let repeats_book = |t: &str| book_title.is_some_and(|b| b.eq_ignore_ascii_case(t.trim()));
    match &meta.title {
        Some(t) if !t.trim().is_empty() && !repeats_book(t) => t.clone(),
        _ => {
            let n = meta.part_no.map(|p| p as usize).unwrap_or(idx + 1);
            // Fall back to the filename stem only if there is no part number at all.
            if meta.part_no.is_none()
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                stem.to_string()
            } else {
                format!("Part {n}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_chapters_map_to_offsets_and_durations() {
        let raw = vec![
            RawChapter {
                start: 0.0,
                end: 120.0,
                title: Some("Intro".into()),
            },
            RawChapter {
                start: 120.0,
                end: 300.0,
                title: Some("Main".into()),
            },
        ];
        let out = chapters_from_raw(Path::new("book.m4b"), &raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].idx, 0);
        assert_eq!(out[0].file_offset, 0.0);
        assert_eq!(out[0].duration, Some(120.0));
        assert_eq!(out[1].idx, 1);
        assert_eq!(out[1].file_offset, 120.0);
        assert_eq!(out[1].duration, Some(180.0));
        assert_eq!(out[1].title.as_deref(), Some("Main"));
    }

    #[test]
    fn chapter_title_prefers_track_title_else_synthesizes_part() {
        // A distinct track title wins.
        let m = FileMeta {
            part_no: Some(1),
            title: Some("Prologue".into()),
            duration: None,
        };
        assert_eq!(
            chapter_title(Path::new("00.mp3"), &m, 0, Some("The Name of the Wind")),
            "Prologue"
        );
        // A title that just repeats the book title is dropped for "Part N".
        let m = FileMeta {
            part_no: Some(3),
            title: Some("The Graveyard Book".into()),
            duration: None,
        };
        assert_eq!(
            chapter_title(Path::new("p3.m4b"), &m, 2, Some("The Graveyard Book")),
            "Part 3"
        );
    }
}
