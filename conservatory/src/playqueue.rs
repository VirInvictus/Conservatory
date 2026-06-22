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

/// A queued episode's playable source (Phase 6b-ii-c). Either a downloaded file
/// (relative to the library root) or a stream URL; `build_episode_queue` picks
/// the local file when present, else the URL. Podcast-only.
#[cfg(feature = "podcasts")]
#[derive(Debug, Clone)]
pub struct EpisodeSource {
    pub id: i64,
    pub audio_path: Option<String>,
    pub audio_url: Option<String>,
}

/// Build a play queue from a list of episodes (the deadbeef idiom, episode
/// flavour). Each episode's `source` is its downloaded file (`root` + the
/// relative `audio_path`) when present, else the stream URL (libmpv's
/// `loadfile` takes a URL as-is). Episodes with neither are skipped; `start`
/// re-indexes past any skip, pointing at the activated episode (or 0).
#[cfg(feature = "podcasts")]
pub fn build_episode_queue(
    ordered: &[EpisodeSource],
    activated: usize,
    root: &Path,
) -> (Vec<PlayableItem>, usize) {
    let activated_id = ordered.get(activated).map(|e| e.id);

    let items: Vec<PlayableItem> = ordered
        .iter()
        .filter_map(|e| {
            let source = match (&e.audio_path, &e.audio_url) {
                (Some(path), _) => root.join(path),
                (None, Some(url)) => std::path::PathBuf::from(url),
                (None, None) => return None,
            };
            Some(PlayableItem {
                track_id: e.id,
                source,
                profile: conservatory_core::resolve_episode_profile(),
                album_id: None,
                kind: MediaKind::Episode,
            })
        })
        .collect();

    let start = activated_id
        .and_then(|id| items.iter().position(|i| i.track_id == id))
        .unwrap_or(0);

    (items, start)
}

/// Which side of the drop-target row the dragged row lands on (from the cursor
/// Y vs the row's mid-height, the GNOME/macOS reorder idiom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropBias {
    Above,
    Below,
}

/// The final queue position for a drag-and-drop reorder: the item at `from`
/// dropped onto the row at `dest` with `bias`, in a queue of `count` items. The
/// result is the `to` index for `reorder_queue`/`move_item` (which both apply
/// `remove(from)` then `insert(to)`), clamped into range. Pure.
pub fn drop_target_position(from: usize, dest: usize, bias: DropBias, count: usize) -> usize {
    // After removing `from`, the dest row sits one slot earlier iff it was after
    // `from`; insert above it at that slot, below it one past.
    let dest_prime = if from < dest { dest - 1 } else { dest };
    let to = match bias {
        DropBias::Above => dest_prime,
        DropBias::Below => dest_prime + 1,
    };
    to.min(count.saturating_sub(1))
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
    fn drop_position_dragging_down() {
        // Drag item 0 onto item 3, below it: ends at index 3.
        assert_eq!(drop_target_position(0, 3, DropBias::Below, 5), 3);
        // Above item 3: ends at index 2.
        assert_eq!(drop_target_position(0, 3, DropBias::Above, 5), 2);
    }

    #[test]
    fn drop_position_dragging_up() {
        // Drag item 4 onto item 1, above it: ends at index 1.
        assert_eq!(drop_target_position(4, 1, DropBias::Above, 5), 1);
        // Below item 1: ends at index 2.
        assert_eq!(drop_target_position(4, 1, DropBias::Below, 5), 2);
    }

    #[test]
    fn drop_position_clamps_at_the_end() {
        // Below the last row never exceeds the final index.
        assert_eq!(drop_target_position(0, 4, DropBias::Below, 5), 4);
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

    #[cfg(feature = "podcasts")]
    #[test]
    fn build_episode_queue_prefers_local_streams_else_and_skips_sourceless() {
        use std::path::PathBuf;
        let root = Path::new("/lib");
        let episodes = vec![
            // Downloaded: the local file wins even with a URL present.
            EpisodeSource {
                id: 1,
                audio_path: Some("Podcasts/s/2024-01-01--e/a.mp3".to_string()),
                audio_url: Some("https://cdn/a.mp3".to_string()),
            },
            // Not downloaded: stream the URL.
            EpisodeSource {
                id: 2,
                audio_path: None,
                audio_url: Some("https://cdn/b.mp3".to_string()),
            },
            // Neither: skipped.
            EpisodeSource {
                id: 3,
                audio_path: None,
                audio_url: None,
            },
        ];
        let (items, start) = build_episode_queue(&episodes, 1, root);
        assert_eq!(items.len(), 2, "the source-less episode is skipped");
        assert_eq!(
            items[0].source,
            PathBuf::from("/lib/Podcasts/s/2024-01-01--e/a.mp3")
        );
        assert_eq!(items[1].source, PathBuf::from("https://cdn/b.mp3"));
        assert_eq!(items[0].kind, MediaKind::Episode);
        assert_eq!(start, 1, "activated episode id 2 is item index 1");
    }
}
