//! Chapter marks and the pure skip-navigation logic (Phase 6c-iii-b, spec §6.1, §8).
//!
//! A [`ChapterMark`] is the lightweight, engine-facing view of a chapter: just the
//! `start_time` and an optional title (the host seeks to a time; it never needs the
//! url / image the `chapters` table also stores). The consumer resolves a queue
//! item's marks at build time (`list_chapters` for an episode, `book_chapters` for
//! an audiobook at Phase 7c) and attaches them to the [`PlayableItem`], so the
//! engine does no DB reads.
//!
//! The two helpers are pure and unit-tested headless. They are the **shared**
//! chapter-skip mechanism: podcasts feed them `chapters.start_time`, audiobooks
//! feed them `book_chapters` offsets at 7c (roadmap), so the navigation lives here
//! in core, not in a podcast-only path.
//!
//! [`PlayableItem`]: crate::player::item::PlayableItem

/// How far past a chapter's start a "skip back" still restarts that chapter rather
/// than stepping to the previous one (the convention the item-level
/// `PREVIOUS_RESTART_THRESHOLD` uses one level up): tap back near the start to go
/// to the previous chapter, tap back mid-chapter to return to this chapter's head.
const BACK_RESTART_SECS: f64 = 3.0;

/// One chapter boundary, as the engine needs it: where it starts and what it is
/// called. Sorted by `start_time` by construction (`list_chapters` orders by it).
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterMark {
    pub start_time: f64,
    pub title: Option<String>,
}

/// The index of the chapter containing `pos`: the last mark whose `start_time` is
/// at or before `pos`. `None` when there are no marks, or `pos` precedes the first
/// chapter's start (a rare lead-in before chapter one).
pub fn current_chapter_at(marks: &[ChapterMark], pos: f64) -> Option<usize> {
    marks.iter().rposition(|m| m.start_time <= pos)
}

/// The absolute time to seek to for a chapter skip from `pos`, or `None` for a
/// no-op (forward past the last chapter). `dir > 0` skips forward, `dir <= 0` back:
///
/// - **Forward:** the start of the first chapter beginning after `pos`; `None` at
///   the end (clamped, a no-op).
/// - **Back:** the start of the *current* chapter if `pos` is more than
///   [`BACK_RESTART_SECS`] into it (restart this chapter), else the previous
///   chapter's start; clamped to `0.0` at the first chapter.
///
/// Empty `marks` yields `None` (the engine then leaves the playhead alone).
pub fn neighbour_chapter(marks: &[ChapterMark], pos: f64, dir: i32) -> Option<f64> {
    if marks.is_empty() {
        return None;
    }
    if dir > 0 {
        // First chapter strictly after the playhead (so sitting exactly on a
        // boundary still advances, never sticks).
        marks
            .iter()
            .find(|m| m.start_time > pos)
            .map(|m| m.start_time)
    } else {
        match current_chapter_at(marks, pos) {
            // Before the first chapter: back is the very start.
            None => Some(0.0),
            Some(cur) => {
                let cur_start = marks[cur].start_time;
                if pos - cur_start > BACK_RESTART_SECS {
                    Some(cur_start) // restart the current chapter
                } else if cur == 0 {
                    Some(0.0) // already in the first chapter: clamp to the start
                } else {
                    Some(marks[cur - 1].start_time) // step to the previous chapter
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(starts: &[f64]) -> Vec<ChapterMark> {
        starts
            .iter()
            .map(|&start_time| ChapterMark {
                start_time,
                title: None,
            })
            .collect()
    }

    #[test]
    fn current_chapter_is_the_last_started() {
        let m = marks(&[0.0, 60.0, 120.0]);
        assert_eq!(current_chapter_at(&m, 0.0), Some(0));
        assert_eq!(current_chapter_at(&m, 30.0), Some(0));
        assert_eq!(current_chapter_at(&m, 60.0), Some(1));
        assert_eq!(current_chapter_at(&m, 130.0), Some(2));
        assert_eq!(current_chapter_at(&[], 5.0), None);
        // A lead-in before chapter one (first start > 0).
        assert_eq!(current_chapter_at(&marks(&[10.0, 60.0]), 5.0), None);
    }

    #[test]
    fn forward_lands_on_the_next_boundary_and_clamps_at_the_end() {
        let m = marks(&[0.0, 60.0, 120.0]);
        assert_eq!(neighbour_chapter(&m, 30.0, 1), Some(60.0));
        assert_eq!(neighbour_chapter(&m, 59.0, 1), Some(60.0));
        // Exactly on a boundary advances to the next, never sticks.
        assert_eq!(neighbour_chapter(&m, 60.0, 1), Some(120.0));
        // Past the last chapter: no-op.
        assert_eq!(neighbour_chapter(&m, 130.0, 1), None);
        assert_eq!(neighbour_chapter(&[], 5.0, 1), None);
    }

    #[test]
    fn back_restarts_then_steps_then_clamps() {
        let m = marks(&[0.0, 60.0, 120.0]);
        // Deep into chapter three: restart it.
        assert_eq!(neighbour_chapter(&m, 130.0, -1), Some(120.0));
        // Just into chapter three: step to chapter two.
        assert_eq!(neighbour_chapter(&m, 121.0, -1), Some(60.0));
        // Deep into chapter one: restart it.
        assert_eq!(neighbour_chapter(&m, 30.0, -1), Some(0.0));
        // Just into chapter one: clamp to the start.
        assert_eq!(neighbour_chapter(&m, 1.0, -1), Some(0.0));
        // Before any chapter: clamp to the start.
        assert_eq!(neighbour_chapter(&marks(&[10.0, 60.0]), 5.0, -1), Some(0.0));
        assert_eq!(neighbour_chapter(&[], 5.0, -1), None);
    }
}
