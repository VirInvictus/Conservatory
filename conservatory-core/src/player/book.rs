//! Audiobook segment planning (Phase 7c, spec §6.1).
//!
//! A book is ONE queue item, but its chapters live either inside a single M4B
//! (offsets in one file) or across a one-file-per-chapter folder. The engine
//! loads one file at a time and reads `time_pos` / `duration` per file, so a
//! multi-file book needs a map from an **absolute** book position (seconds
//! across the whole book) to a `(file, offset within that file)`. That map is
//! the ordered list of [`BookSegment`]s; the chapter marks are lifted to
//! absolute book time at the same point. Both are pure and unit-tested; the
//! engine consumes them and never touches the DB (the queue builder attaches
//! them to the [`PlayableItem`], the `ChapterMark` precedent).
//!
//! [`PlayableItem`]: crate::player::item::PlayableItem

use std::path::{Path, PathBuf};

use crate::db::MediaKind;
use crate::db::models::BookChapter;
use crate::player::chapters::ChapterMark;
use crate::player::item::PlayableItem;
use crate::player::profile::MusicProfile;

/// One contiguous audio file of a book, with the absolute book time at which it
/// begins. `duration` is the playable length of the file (the span its chapters
/// cover); it is `0.0` when no chapter in the file carried a duration, in which
/// case a later segment's `start` is under-counted (a best-effort degrade: the
/// engine still advances file to file on EOF, only an absolute *seek* into a
/// later file is imprecise).
#[derive(Debug, Clone, PartialEq)]
pub struct BookSegment {
    pub file: PathBuf,
    pub start: f64,
    pub duration: f64,
}

/// The engine-facing plan for a book: the ordered files to play and the chapter
/// marks lifted to absolute book time.
#[derive(Debug, Clone, PartialEq)]
pub struct BookPlan {
    pub segments: Vec<BookSegment>,
    pub marks: Vec<ChapterMark>,
}

impl BookPlan {
    /// Total playable length of the book (the end of the last segment), or `0.0`
    /// when no duration was known.
    pub fn total_duration(&self) -> f64 {
        self.segments.last().map_or(0.0, |s| s.start + s.duration)
    }
}

/// Group a book's chapters (ordered by `idx`) into per-file segments and absolute
/// chapter marks. Consecutive chapters sharing a `file_path` collapse into one
/// segment (the M4B case → a single segment; one-file-per-chapter → one segment
/// each). A chapter's absolute start is its segment's cumulative `start` plus the
/// chapter's in-file `file_offset`.
pub fn plan_book(chapters: &[BookChapter]) -> BookPlan {
    let mut segments: Vec<BookSegment> = Vec::new();
    let mut marks: Vec<ChapterMark> = Vec::with_capacity(chapters.len());

    for ch in chapters {
        let file = PathBuf::from(&ch.file_path);
        // Open a new segment when the file changes (the first chapter always
        // opens one). The previous segment's duration is complete by then
        // (chapters are file-contiguous by `idx`), so its end is the new
        // segment's cumulative start.
        if segments.last().is_none_or(|s| s.file != file) {
            let start = segments.last().map_or(0.0, |s| s.start + s.duration);
            segments.push(BookSegment {
                file,
                start,
                duration: 0.0,
            });
        }
        let seg = segments.last_mut().expect("a segment was just ensured");
        marks.push(ChapterMark {
            start_time: seg.start + ch.file_offset,
            title: ch.title.clone(),
        });
        // Grow the segment to cover this chapter (its offset plus its length).
        if let Some(d) = ch.duration {
            seg.duration = seg.duration.max(ch.file_offset + d);
        }
    }

    BookPlan { segments, marks }
}

/// Resolve an absolute book position to `(segment index, offset within that
/// segment's file)`: the last segment whose `start` is at or before `abs`
/// (mirrors [`current_chapter_at`]). Used by the engine to load the right file
/// and seek into it on resume / a slider seek / a cross-file chapter skip.
/// `None` when there are no segments.
///
/// [`current_chapter_at`]: crate::player::chapters::current_chapter_at
pub fn locate(segments: &[BookSegment], abs: f64) -> Option<(usize, f64)> {
    if segments.is_empty() {
        return None;
    }
    let idx = segments.iter().rposition(|s| s.start <= abs).unwrap_or(0);
    let offset = (abs - segments[idx].start).max(0.0);
    Some((idx, offset))
}

/// Build the single [`PlayableItem`] for a book (Phase 7c). The book's chapters
/// (ordered by `idx`) are planned into per-file segments, each segment file
/// resolved under the library `root`; `source` is the first segment's file (what
/// the engine loads first) and the rest travel in `segments` for the engine's
/// internal file advance. `chapters` are the absolute book-time marks. `None`
/// when the book has no chapters / no playable file. `book_id` rides in
/// `track_id`, the queue's per-kind id field (the episode precedent).
pub fn build_book_item(
    book_id: i64,
    chapters: &[BookChapter],
    root: &Path,
    profile: MusicProfile,
) -> Option<PlayableItem> {
    let mut plan = plan_book(chapters);
    if plan.segments.is_empty() {
        return None;
    }
    for seg in &mut plan.segments {
        seg.file = root.join(&seg.file);
    }
    Some(PlayableItem {
        track_id: book_id,
        source: plan.segments[0].file.clone(),
        profile,
        album_id: None,
        kind: MediaKind::Audiobook,
        streaming: false,
        chapters: plan.marks.into(),
        segments: plan.segments.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(idx: i64, file: &str, offset: f64, duration: Option<f64>) -> BookChapter {
        BookChapter {
            id: idx + 1,
            book_id: 1,
            idx,
            title: Some(format!("Chapter {}", idx + 1)),
            file_path: file.to_string(),
            file_offset: offset,
            duration,
        }
    }

    #[test]
    fn m4b_is_one_segment_with_all_marks() {
        // One file, three chapters at offsets inside it.
        let chs = vec![
            chapter(0, "Book/book.m4b", 0.0, Some(600.0)),
            chapter(1, "Book/book.m4b", 600.0, Some(600.0)),
            chapter(2, "Book/book.m4b", 1200.0, Some(300.0)),
        ];
        let plan = plan_book(&chs);
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(plan.segments[0].start, 0.0);
        assert_eq!(plan.segments[0].duration, 1500.0); // 1200 + 300
        // Marks are at the in-file offsets (one file → absolute == offset).
        let starts: Vec<f64> = plan.marks.iter().map(|m| m.start_time).collect();
        assert_eq!(starts, vec![0.0, 600.0, 1200.0]);
        assert_eq!(plan.total_duration(), 1500.0);
    }

    #[test]
    fn multi_file_is_one_segment_each_with_cumulative_starts() {
        // Three files, one chapter each (offset 0): the classic mp3-per-chapter.
        let chs = vec![
            chapter(0, "Book/01.mp3", 0.0, Some(300.0)),
            chapter(1, "Book/02.mp3", 0.0, Some(420.0)),
            chapter(2, "Book/03.mp3", 0.0, Some(180.0)),
        ];
        let plan = plan_book(&chs);
        assert_eq!(plan.segments.len(), 3);
        let starts: Vec<f64> = plan.segments.iter().map(|s| s.start).collect();
        assert_eq!(starts, vec![0.0, 300.0, 720.0]); // cumulative
        // Chapter marks sit at the cumulative file boundaries.
        let marks: Vec<f64> = plan.marks.iter().map(|m| m.start_time).collect();
        assert_eq!(marks, vec![0.0, 300.0, 720.0]);
        assert_eq!(plan.total_duration(), 900.0);
    }

    #[test]
    fn multi_file_with_internal_chapters() {
        // Two files, each with two chapters (a less common but valid shape).
        let chs = vec![
            chapter(0, "Book/01.mp3", 0.0, Some(300.0)),
            chapter(1, "Book/01.mp3", 300.0, Some(300.0)),
            chapter(2, "Book/02.mp3", 0.0, Some(200.0)),
            chapter(3, "Book/02.mp3", 200.0, Some(200.0)),
        ];
        let plan = plan_book(&chs);
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].duration, 600.0);
        assert_eq!(plan.segments[1].start, 600.0);
        let marks: Vec<f64> = plan.marks.iter().map(|m| m.start_time).collect();
        assert_eq!(marks, vec![0.0, 300.0, 600.0, 800.0]);
    }

    #[test]
    fn missing_duration_degrades_without_panicking() {
        // No durations: segments still split per file, starts collapse to 0 (the
        // engine advances on EOF; only absolute seeking into a later file suffers).
        let chs = vec![
            chapter(0, "Book/01.mp3", 0.0, None),
            chapter(1, "Book/02.mp3", 0.0, None),
        ];
        let plan = plan_book(&chs);
        assert_eq!(plan.segments.len(), 2);
        assert_eq!(plan.segments[0].duration, 0.0);
        assert_eq!(plan.segments[1].start, 0.0);
        assert_eq!(plan.total_duration(), 0.0);
    }

    #[test]
    fn locate_finds_the_segment_and_offset() {
        let chs = vec![
            chapter(0, "Book/01.mp3", 0.0, Some(300.0)),
            chapter(1, "Book/02.mp3", 0.0, Some(420.0)),
            chapter(2, "Book/03.mp3", 0.0, Some(180.0)),
        ];
        let segs = plan_book(&chs).segments;
        assert_eq!(locate(&segs, 0.0), Some((0, 0.0)));
        assert_eq!(locate(&segs, 150.0), Some((0, 150.0)));
        assert_eq!(locate(&segs, 300.0), Some((1, 0.0)));
        assert_eq!(locate(&segs, 500.0), Some((1, 200.0)));
        assert_eq!(locate(&segs, 720.0), Some((2, 0.0)));
        // Past the end clamps into the last segment.
        assert_eq!(locate(&segs, 9999.0), Some((2, 9279.0)));
        assert_eq!(locate(&[], 5.0), None);
    }
}
