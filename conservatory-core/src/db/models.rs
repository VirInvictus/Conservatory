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
