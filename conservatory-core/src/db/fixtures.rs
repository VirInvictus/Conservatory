//! Synthetic library builder for tests and the `debug-fixture` CLI verb.
//!
//! Deterministic, uniform data: good for exercising the schema, worker, read
//! pool, and FTS triggers, not for realistic UI screenshots. Mirrors the shape
//! of `belfry-core`'s fixture generator. No real audio is touched (the DB layer
//! does not read files yet; that arrives with the tag reader at Phase 1c).

use std::str::FromStr;

use chrono::Utc;

use crate::db::WorkerHandle;
use crate::db::models::{Album, Artist, Track};
use crate::errors::{Error, Result};

const GENRES: &[&str] = &["Electronic", "Ambient", "Jazz", "Rock", "Hip Hop"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureScale {
    /// 5 artists × 2 albums × 8 tracks = 80 tracks.
    Small,
    /// 50 artists × 4 albums × 10 tracks = 2,000 tracks.
    Medium,
    /// 100 artists × 10 albums × 12 tracks = 12,000 tracks.
    Large,
}

impl FixtureScale {
    fn shape(self) -> (usize, usize, usize) {
        match self {
            Self::Small => (5, 2, 8),
            Self::Medium => (50, 4, 10),
            Self::Large => (100, 10, 12),
        }
    }
}

impl FromStr for FixtureScale {
    type Err = Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            other => Err(Error::InvalidEnum {
                field: "fixture_scale",
                value: other.to_string(),
            }),
        }
    }
}

/// Synthesize a library at the given scale, inserting through the worker.
pub async fn generate(handle: &WorkerHandle, scale: FixtureScale) -> Result<()> {
    let (artists, albums_per_artist, tracks_per_album) = scale.shape();
    let now = Utc::now();

    for a in 0..artists {
        let artist_id = handle
            .insert_artist(Artist {
                id: 0,
                name: format!("Artist {a}"),
                sort_name: format!("Artist {a:04}"),
                musicbrainz_id: None,
            })
            .await?;

        for b in 0..albums_per_artist {
            let genre = GENRES[(a + b) % GENRES.len()];
            let genre_id = handle.get_or_create_genre(genre).await?;
            let folder = format!("{genre}/Artist {a:04}/Album {a}-{b} (2000)");

            let album_id = handle
                .insert_album(Album {
                    id: 0,
                    title: format!("Album {a}-{b}"),
                    album_artist_id: Some(artist_id),
                    shelf_genre: Some(genre.to_string()),
                    year: Some(1990 + (b as i32 % 30)),
                    release_date: None,
                    musicbrainz_release_id: None,
                    cover_path: None,
                    accent_rgb: None,
                    folder_path: folder.clone(),
                    added_at: Some(now),
                })
                .await?;

            for t in 0..tracks_per_album {
                let track_id = handle
                    .insert_track(Track {
                        id: 0,
                        album_id: Some(album_id),
                        artist_id: Some(artist_id),
                        title: format!("Track {t}"),
                        track_no: Some(t as i32 + 1),
                        disc_no: Some(1),
                        duration: Some(180.0 + t as f64),
                        file_path: format!("{folder}/{:02} - Track {t}.flac", t + 1),
                        format: Some("flac".to_string()),
                        bitrate: Some(1024),
                        sample_rate: Some(44100),
                        replaygain_track: None,
                        replaygain_album: None,
                        rating: 0,
                        play_count: 0,
                        last_played: None,
                        starred: false,
                        musicbrainz_recording_id: None,
                        added_at: Some(now),
                    })
                    .await?;
                handle.link_track_genre(track_id, genre_id).await?;
            }
        }
    }

    Ok(())
}
