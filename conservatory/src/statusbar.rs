//! Pure helpers for the status bar footer and the leaf play-status glyph (Phase
//! 11b), kept out of the GTK widgets so they stay headless-testable (the
//! `query.rs` / `playqueue.rs` precedent).
//!
//! The footer's left side reports the *playing* track's technical line; its
//! right side reports the active view's aggregate (count + total playtime), or
//! the selection's when two or more rows are selected. The glyph column marks
//! the leaf row that is the currently playing track.

use conservatory_core::db::TrackBrief;

/// Count and total duration (seconds) of a track set; the right-hand status
/// readout. Tracks with no known duration contribute 0 to the total.
pub fn view_aggregate(rows: &[TrackBrief]) -> (usize, f64) {
    let total = rows.iter().filter_map(|t| t.duration).sum();
    (rows.len(), total)
}

/// Group an integer with thousands separators: `1203` → `"1,203"`.
pub fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in s.chars().enumerate() {
        if i != 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A coarse playtime: `m:ss`, `h:mm:ss`, or `Nd h:mm:ss` past a day. The right
/// side of the footer ("N tracks · D total playtime").
pub fn format_playtime(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (d, h, m, s) = (
        total / 86_400,
        (total % 86_400) / 3_600,
        (total % 3_600) / 60,
        total % 60,
    );
    if d > 0 {
        format!("{d}d {h}:{m:02}:{s:02}")
    } else if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The right-hand footer label. With `selected` rows (2+), it reports the
/// selection; otherwise the whole view.
pub fn aggregate_label(count: usize, total_secs: f64, selected: bool) -> String {
    let noun = if selected {
        "selected"
    } else if count == 1 {
        "track"
    } else {
        "tracks"
    };
    format!(
        "{} {noun} · {}",
        group_thousands(count),
        format_playtime(total_secs)
    )
}

/// The left-hand footer line: the playing track's technical readout, skipping
/// any field that is unknown. Empty when nothing is known (nothing playing).
pub fn tech_line(
    format: Option<&str>,
    sample_rate: Option<i32>,
    channels: Option<i64>,
    bitrate: Option<i32>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = format.filter(|f| !f.is_empty()) {
        parts.push(f.to_uppercase());
    }
    if let Some(sr) = sample_rate.filter(|sr| *sr > 0) {
        parts.push(format!("{sr} Hz"));
    }
    if let Some(ch) = channels.filter(|ch| *ch > 0) {
        parts.push(format!("{ch}ch"));
    }
    if let Some(br) = bitrate.filter(|br| *br > 0) {
        parts.push(format!("{br} kbps"));
    }
    parts.join(" · ")
}

/// The play-status state for a leaf row: `0` none, `1` playing, `2` paused. A
/// row is "the current track" only when the playing item is a track (not an
/// episode / book) and its id matches.
pub fn play_state(row_id: i64, playing_id: Option<i64>, is_track: bool, paused: bool) -> u8 {
    if is_track && playing_id == Some(row_id) {
        if paused {
            2
        } else {
            1
        }
    } else {
        0
    }
}

/// The symbolic icon for a play-status state, or `None` (no glyph) for an
/// inactive row. Symbolic names come from the icon theme (font-independent).
pub fn play_glyph(state: u8) -> Option<&'static str> {
    match state {
        1 => Some("media-playback-start-symbolic"),
        2 => Some("media-playback-pause-symbolic"),
        _ => None,
    }
}

/// The leaf-list position of the playing track (Phase 11d jump-to-current): the
/// first row whose id matches, in the model's display order. `None` when the
/// playing track is not in the current view. Pure.
pub fn current_row_index(model_ids: &[i64], playing_id: i64) -> Option<u32> {
    model_ids
        .iter()
        .position(|id| *id == playing_id)
        .map(|p| p as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(id: i64, duration: Option<f64>) -> TrackBrief {
        TrackBrief {
            id,
            title: String::new(),
            artist: None,
            album: None,
            genres: String::new(),
            duration,
            rating: 0,
            cover_path: None,
            accent_rgb: None,
        }
    }

    #[test]
    fn aggregate_sums_known_durations() {
        let rows = [brief(1, Some(120.0)), brief(2, None), brief(3, Some(60.5))];
        let (count, total) = view_aggregate(&rows);
        assert_eq!(count, 3);
        assert_eq!(total, 180.5);
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(42), "42");
        assert_eq!(group_thousands(1_203), "1,203");
        assert_eq!(group_thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn playtime_rolls_over_at_hour_and_day() {
        assert_eq!(format_playtime(0.0), "0:00");
        assert_eq!(format_playtime(7.0), "0:07");
        assert_eq!(format_playtime(187.0), "3:07");
        assert_eq!(format_playtime(3_661.0), "1:01:01");
        // 3 days, 14:22:09 = 3*86400 + 14*3600 + 22*60 + 9
        assert_eq!(format_playtime(310_929.0), "3d 14:22:09");
        assert_eq!(format_playtime(-5.0), "0:00");
    }

    #[test]
    fn aggregate_label_switches_noun() {
        assert_eq!(aggregate_label(1, 60.0, false), "1 track · 1:00");
        assert_eq!(
            aggregate_label(1_203, 310_929.0, false),
            "1,203 tracks · 3d 14:22:09"
        );
        assert_eq!(aggregate_label(5, 600.0, true), "5 selected · 10:00");
    }

    #[test]
    fn tech_line_skips_unknowns() {
        assert_eq!(
            tech_line(Some("flac"), Some(44_100), Some(2), Some(1006)),
            "FLAC · 44100 Hz · 2ch · 1006 kbps"
        );
        assert_eq!(
            tech_line(Some("opus"), Some(48_000), Some(2), None),
            "OPUS · 48000 Hz · 2ch"
        );
        assert_eq!(tech_line(None, None, None, None), "");
        assert_eq!(tech_line(Some(""), Some(0), Some(0), Some(0)), "");
    }

    #[test]
    fn play_state_and_glyph() {
        // Playing track matches.
        assert_eq!(play_state(7, Some(7), true, false), 1);
        assert_eq!(play_state(7, Some(7), true, true), 2);
        // Different id, or the playing item is not a track.
        assert_eq!(play_state(7, Some(8), true, false), 0);
        assert_eq!(play_state(7, Some(7), false, false), 0);
        assert_eq!(play_state(7, None, true, false), 0);

        assert_eq!(play_glyph(1), Some("media-playback-start-symbolic"));
        assert_eq!(play_glyph(2), Some("media-playback-pause-symbolic"));
        assert_eq!(play_glyph(0), None);
    }

    #[test]
    fn current_row_index_finds_the_playing_row() {
        let ids = [10, 20, 30, 40];
        assert_eq!(current_row_index(&ids, 30), Some(2));
        assert_eq!(current_row_index(&ids, 10), Some(0));
        // Not in the current view (e.g. filtered out).
        assert_eq!(current_row_index(&ids, 99), None);
        assert_eq!(current_row_index(&[], 10), None);
    }
}
