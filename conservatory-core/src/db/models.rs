//! Domain types for the music data model (spec §4.1, docs/schema.md).
//!
//! Rust idiom over the SQL: `Option` for nullable columns, `bool` for INTEGER
//! 0/1, `chrono::DateTime<Utc>` for unix-epoch INTEGER timestamps, packed RGB as
//! `u32`. `id == 0` on a value not yet inserted; the writer returns the real id.
//! The shape mirrors `belfry-core`'s `domain.rs`.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::errors::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub sort_name: String, // "Beatles, The"; drives path + sort (Calibre author_sort)
    pub musicbrainz_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub album_artist_id: Option<i64>, // None => Various Artists bucket
    pub shelf_genre: Option<String>,  // the only genre input to the path (spec §5.2)
    pub year: Option<i32>,
    pub release_date: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>, // packed RGB, median-cut from cover (spec §7.4)
    pub folder_path: String,     // managed; rendered from the template (spec §5.1)
    pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>, // track artist (may differ from album artist)
    pub title: String,
    pub track_no: Option<i32>,
    pub disc_no: Option<i32>,
    pub duration: Option<f64>, // seconds
    pub file_path: String,     // managed; under the album folder
    pub format: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub replaygain_track: Option<f64>,
    pub replaygain_album: Option<f64>,
    pub rating: u8, // 0–5
    pub play_count: u32,
    pub last_played: Option<DateTime<Utc>>,
    pub starred: bool,
    pub musicbrainz_recording_id: Option<String>,
    pub added_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

/// A named saved search (spec §3.4). `expression` is the raw filter text, stored
/// verbatim and re-parsed on load so it inherits later grammar additions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Perspective {
    pub id: i64,
    pub name: String,
    pub expression: String,
    pub scope: String, // "tracks" today; albums/episodes/books reuse the table later
}

/// The media kind of a unified-queue entry (spec §4.3). Only `Track` is real in
/// Phase 4b; `Episode` and `Audiobook` rows arrive with Phases 6 and 7, but the
/// kind exists from the start because the queue is one core-owned table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Track,
    Episode,
    Audiobook,
}

impl MediaKind {
    /// The `kind` TEXT value stored in the `queue` table.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Track => "track",
            MediaKind::Episode => "episode",
            MediaKind::Audiobook => "audiobook",
        }
    }
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MediaKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "track" => Ok(MediaKind::Track),
            "episode" => Ok(MediaKind::Episode),
            "audiobook" => Ok(MediaKind::Audiobook),
            other => Err(Error::InvalidEnum {
                field: "queue.kind",
                value: other.to_string(),
            }),
        }
    }
}

/// One ordered entry in the unified queue (spec §4.3, §6.1). Exactly one of the
/// id columns is populated, matched to `kind`; `position` is contiguous and
/// drag-reorderable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub position: i64,
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub book_id: Option<i64>,
}

/// The singleton transport cursor as written (spec §6.4, Phase 6b-ii-c-2): what
/// was last playing and where, so a restart resumes. `kind` records the media
/// kind; `track_id` is set for a track, `episode_id` for an episode (the read
/// side is [`crate::db::PlaybackStateRow`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackCursor {
    pub kind: MediaKind,
    pub track_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub position: f64,
    pub paused: bool,
    pub volume: i64,
    pub updated_at: i64,
}

// --- Podcast domain (spec §4.2). Ported in shape from `belfry-core`'s
// `domain.rs`. These are core-owned types (the schema lives in core's migration
// ledger, the §2.2 boundary rule); the `conservatory-podcasts` plugin consumes
// them. A music-only build compiles them but leaves the tables empty.

/// A podcast subscription (spec §4.2). The conditional-GET bookkeeping
/// (`last_fetched` / `last_modified` / `etag`) is written by the fetch loop
/// (Phase 6a-ii); `auth_pass_ref` is a libsecret reference, never an inline
/// secret (oo7, Phase 6a-iii).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Show {
    pub id: i64,
    pub slug: String,
    pub feed_url: String,
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub cover_path: Option<String>,
    pub accent_rgb: Option<u32>,
    pub apple_podcasts_id: Option<String>,
    pub last_fetched: Option<DateTime<Utc>>,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub fetch_interval: u32,
    pub auth_user: Option<String>,
    pub auth_pass_ref: Option<String>,
    pub auto_download: bool,
    pub keep_count: u32,
    pub priority: i32,
    pub folder_path: String,
}

/// One feed episode. Identity is `(show_id, guid)` (spec §8). `episode_type` is
/// kept as the feed's raw string (commonly full / trailer / bonus) rather than a
/// closed enum, so an unexpected value never fails a read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub show_id: i64,
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub pub_date: Option<DateTime<Utc>>,
    pub duration: Option<u32>, // seconds
    pub file_size: Option<u64>,
    pub audio_url: Option<String>,
    pub audio_path: Option<String>, // None until downloaded (spec §5.3)
    pub folder_path: String,
    pub mime_type: Option<String>,
    pub season: Option<u32>,
    pub episode_number: Option<u32>,
    pub episode_type: Option<String>,
}

/// Per-episode triage + playback state (spec §4.2). Queue membership is *not*
/// here: it lives in the unified `queue` table (the §4.2 change from Belfry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayedState {
    Unplayed = 0,
    InProgress = 1,
    PlayedFully = 2,
    ArchivedUnlistened = 3,
}

impl PlayedState {
    /// The INTEGER value stored in `playback.played`.
    pub fn as_i64(self) -> i64 {
        self as i64
    }

    /// Map a stored INTEGER back to a state.
    pub fn from_i64(value: i64) -> Result<Self, Error> {
        match value {
            0 => Ok(PlayedState::Unplayed),
            1 => Ok(PlayedState::InProgress),
            2 => Ok(PlayedState::PlayedFully),
            3 => Ok(PlayedState::ArchivedUnlistened),
            other => Err(Error::InvalidEnum {
                field: "playback.played",
                value: other.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    pub episode_id: i64,
    pub position: f64,
    pub played: PlayedState,
    pub last_played: Option<DateTime<Utc>>,
    pub play_count: u32,
    pub starred: bool,
}

/// What happens to a new episode of this show (Castro inbox policy, spec §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxPolicy {
    Inbox,
    AlwaysQueue,
    AlwaysArchive,
}

impl InboxPolicy {
    /// The TEXT value stored in `show_settings.inbox_policy`.
    pub fn as_str(self) -> &'static str {
        match self {
            InboxPolicy::Inbox => "inbox",
            InboxPolicy::AlwaysQueue => "always_queue",
            InboxPolicy::AlwaysArchive => "always_archive",
        }
    }
}

impl fmt::Display for InboxPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InboxPolicy {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inbox" => Ok(InboxPolicy::Inbox),
            "always_queue" => Ok(InboxPolicy::AlwaysQueue),
            "always_archive" => Ok(InboxPolicy::AlwaysArchive),
            other => Err(Error::InvalidEnum {
                field: "show_settings.inbox_policy",
                value: other.to_string(),
            }),
        }
    }
}

/// Per-show overrides (Overcast pattern, spec §3.7). `skip_forward` / `skip_back`
/// of `None` inherit the global skip amounts.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowSettings {
    pub show_id: i64,
    pub playback_speed: f64,
    pub smart_speed: bool,
    pub voice_boost: bool,
    pub skip_intro: u32,
    pub skip_outro: u32,
    pub skip_forward: Option<u32>,
    pub skip_back: Option<u32>,
    pub inbox_policy: InboxPolicy,
}

/// One playback session (spec §6.3). Append-only; drives Smart Speed time-saved
/// accounting and the history view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListeningSession {
    pub id: i64,
    pub episode_id: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub real_seconds: f64,
    pub audio_seconds: f64,
    pub smart_speed_saved: f64,
}

/// An episode chapter (spec §8). Source is `podcast:chapters` JSON or ID3 CHAP;
/// `start_time` / `end_time` are seconds into the episode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub id: i64,
    pub episode_id: i64,
    pub start_time: f64,
    pub end_time: Option<f64>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub image_path: Option<String>,
}

/// A show tag (Calibre loanword; secondary organization, preserved on OPML
/// round-trip, spec §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_kind_round_trips_through_text() {
        for kind in [MediaKind::Track, MediaKind::Episode, MediaKind::Audiobook] {
            assert_eq!(kind.as_str().parse::<MediaKind>().unwrap(), kind);
        }
        assert!("bogus".parse::<MediaKind>().is_err());
    }

    #[test]
    fn played_state_round_trips_through_i64() {
        for state in [
            PlayedState::Unplayed,
            PlayedState::InProgress,
            PlayedState::PlayedFully,
            PlayedState::ArchivedUnlistened,
        ] {
            assert_eq!(PlayedState::from_i64(state.as_i64()).unwrap(), state);
        }
        assert!(PlayedState::from_i64(9).is_err());
    }

    #[test]
    fn inbox_policy_round_trips_through_text() {
        for policy in [
            InboxPolicy::Inbox,
            InboxPolicy::AlwaysQueue,
            InboxPolicy::AlwaysArchive,
        ] {
            assert_eq!(policy.as_str().parse::<InboxPolicy>().unwrap(), policy);
        }
        assert!("bogus".parse::<InboxPolicy>().is_err());
    }

    #[test]
    fn track_serde_round_trip() {
        let track = Track {
            id: 7,
            album_id: Some(1),
            artist_id: Some(2),
            title: "Roygbiv".to_string(),
            track_no: Some(3),
            disc_no: Some(1),
            duration: Some(151.0),
            file_path: "Electronic/Boards of Canada/Music Has the Right (1998)/03 - Roygbiv.flac"
                .to_string(),
            format: Some("flac".to_string()),
            bitrate: Some(1024),
            sample_rate: Some(44100),
            replaygain_track: Some(-7.5),
            replaygain_album: Some(-7.2),
            rating: 5,
            play_count: 42,
            last_played: Some(Utc::now()),
            starred: true,
            musicbrainz_recording_id: None,
            added_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&track).unwrap();
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(track, back);
    }
}
