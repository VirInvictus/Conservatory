//! Building a play queue from the browse list (Phase 4b-ii-a), kept out of the
//! GTK widgets so it stays headless-testable (the `query.rs` precedent).
//!
//! Double-clicking a track plays the *visible* leaf list from that row (the
//! deadbeef/foobar idiom): the GUI hands the ordered track ids, the activated
//! index, and the `Track` rows it batch-read, and gets back the resolved
//! `PlayableItem`s plus the start index, adjusted for any track that vanished
//! between the read and the build.

use std::collections::HashMap;
use std::path::Path;

use conservatory_core::db::{MediaKind, Track};
use conservatory_core::{PlayableItem, PlaybackConfig, resolve_music_profile};

/// Build the play queue and start index from the visible list.
///
/// `ordered_ids` is the leaf in display order; `activated` is the row the user
/// double-clicked; `tracks` are the rows fetched for those ids (any order, may
/// be missing some). Items come back in display order with their profile
/// resolved and an absolute `source` (`root` + the relative `file_path`); the
/// returned start points at the activated track's item, or 0 if it vanished.
pub fn build_play_queue(
    ordered_ids: &[i64],
    activated: usize,
    tracks: &[Track],
    root: &Path,
    cfg: &PlaybackConfig,
) -> (Vec<PlayableItem>, usize) {
    let by_id: HashMap<i64, &Track> = tracks.iter().map(|t| (t.id, t)).collect();
    let activated_id = ordered_ids.get(activated).copied();

    let items: Vec<PlayableItem> = ordered_ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|track| PlayableItem {
            track_id: track.id,
            source: root.join(&track.file_path),
            profile: resolve_music_profile(track, cfg),
            album_id: track.album_id,
            kind: MediaKind::Track,
        })
        .collect();

    let start = activated_id
        .and_then(|id| items.iter().position(|i| i.track_id == id))
        .unwrap_or(0);

    (items, start)
}

/// Format a number of seconds as `m:ss` (e.g. `3:07`); negatives clamp to 0.
pub fn fmt_secs(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    format!("{}:{:02}", total / 60, total % 60)
}

/// `position / duration` for the Now-bar, e.g. `1:12 / 3:40`; an unknown
/// duration shows just the position.
pub fn fmt_position(position: f64, duration: Option<f64>) -> String {
    match duration {
        Some(d) => format!("{} / {}", fmt_secs(position), fmt_secs(d)),
        None => fmt_secs(position),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn track(id: i64, path: &str) -> Track {
        Track {
            id,
            album_id: Some(1),
            artist_id: Some(1),
            title: format!("t{id}"),
            track_no: Some(1),
            disc_no: Some(1),
            duration: Some(120.0),
            file_path: path.to_string(),
            format: Some("flac".into()),
            bitrate: Some(1000),
            sample_rate: Some(44100),
            replaygain_track: None,
            replaygain_album: None,
            rating: 0,
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: Some(Utc::now()),
        }
    }

    #[test]
    fn preserves_display_order_and_joins_root() {
        let ids = vec![3, 1, 2];
        let tracks = vec![track(1, "a.flac"), track(2, "b.flac"), track(3, "c.flac")];
        let (items, start) = build_play_queue(
            &ids,
            0,
            &tracks,
            Path::new("/lib"),
            &PlaybackConfig::default(),
        );
        assert_eq!(
            items.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        assert_eq!(items[0].source, Path::new("/lib/c.flac"));
        assert_eq!(start, 0); // activated id 3 is first
    }

    #[test]
    fn start_points_at_the_activated_track() {
        let ids = vec![10, 20, 30];
        let tracks = vec![track(10, "a"), track(20, "b"), track(30, "c")];
        let (_items, start) = build_play_queue(
            &ids,
            2,
            &tracks,
            Path::new("/m"),
            &PlaybackConfig::default(),
        );
        assert_eq!(start, 2);
    }

    #[test]
    fn missing_track_is_skipped_and_start_reindexes() {
        // id 20 vanished between the read and the build; the activated row was 30.
        let ids = vec![10, 20, 30];
        let tracks = vec![track(10, "a"), track(30, "c")];
        let (items, start) = build_play_queue(
            &ids,
            2,
            &tracks,
            Path::new("/m"),
            &PlaybackConfig::default(),
        );
        assert_eq!(
            items.iter().map(|i| i.track_id).collect::<Vec<_>>(),
            vec![10, 30]
        );
        assert_eq!(start, 1); // 30 is now at index 1
    }

    #[test]
    fn vanished_activated_track_falls_back_to_start() {
        let ids = vec![10, 20, 30];
        let tracks = vec![track(10, "a"), track(30, "c")];
        let (_items, start) = build_play_queue(
            &ids,
            1,
            &tracks,
            Path::new("/m"),
            &PlaybackConfig::default(),
        );
        assert_eq!(start, 0); // activated id 20 is gone
    }

    #[test]
    fn formats_times() {
        assert_eq!(fmt_secs(0.0), "0:00");
        assert_eq!(fmt_secs(7.0), "0:07");
        assert_eq!(fmt_secs(187.0), "3:07");
        assert_eq!(fmt_secs(-5.0), "0:00");
        assert_eq!(fmt_position(72.0, Some(220.0)), "1:12 / 3:40");
        assert_eq!(fmt_position(5.0, None), "0:05");
    }
}
