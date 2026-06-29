//! Library statistics (Phase 8c-ii, roadmap "Phase 8 / 8c").
//!
//! A one-command summary of the music library, ported from Lattice's `--stats`
//! (`~/.gitrepos/Lattice/src/lattice/modes/stats.py`): the totals, a format
//! breakdown with sizes, a bitrate summary, the rating distribution, the genre
//! distribution (with a per-genre rating tally), and the top artists.
//!
//! DB-canonical: everything aggregates from the [`StatsTrackRow`] /
//! [`StatsGenreRow`] reads and [`LibraryCounts`], except file size, which the
//! schema does not store. That one fact comes from a `stat()` pass over the
//! files and so needs a root; without it the sizes are `None` ("n/a").

use std::collections::HashMap;
use std::path::Path;

use crate::audit::DEFAULT_BITRATE_FLOOR;
use crate::db::{LibraryCounts, StatsGenreRow, StatsTrackRow};

/// A 0..5 rating tally; `stars[0]` is 1-star, `stars[4]` is 5-star. Rating 0 is
/// Conservatory's unrated default (it is never folded into 1-star the way
/// Lattice maps a literal 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RatingTally {
    pub unrated: usize,
    pub stars: [usize; 5],
}

impl RatingTally {
    fn add(&mut self, rating: i64) {
        match rating {
            1..=5 => self.stars[(rating - 1) as usize] += 1,
            _ => self.unrated += 1,
        }
    }

    pub fn rated(&self) -> usize {
        self.stars.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatStat {
    pub format: String,
    pub count: usize,
    /// `None` when no root was given to size the files.
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BitrateStat {
    pub avg: f64,
    pub min: u32,
    pub max: u32,
    pub below_floor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreStat {
    pub genre: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistStat {
    pub artist: String,
    pub count: usize,
}

/// The full statistics summary. `genres` and `top_artists` are returned fully
/// sorted (descending by count); the caller takes the top N.
#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub total_tracks: i64,
    pub total_albums: i64,
    pub total_artists: i64,
    pub total_size_bytes: Option<u64>,
    pub total_duration_secs: f64,
    pub fully_tagged: usize,
    pub formats: Vec<FormatStat>,
    pub bitrate: Option<BitrateStat>,
    pub ratings: RatingTally,
    pub genres: Vec<GenreStat>,
    pub genre_ratings: Vec<(String, RatingTally)>,
    pub top_artists: Vec<ArtistStat>,
}

/// A track is fully tagged when it carries title, artist, track number, and at
/// least one genre (the inverse of the Phase 8c-i missing-tags predicate).
fn is_fully_tagged(r: &StatsTrackRow) -> bool {
    let has_artist = !r.artist.as_deref().map(str::trim).unwrap_or("").is_empty();
    !r.title.trim().is_empty() && has_artist && r.track_no.is_some() && r.genre_count > 0
}

/// Aggregate the library statistics. The size pass `stat`s each track's file
/// under `root`; with `root == None` all sizes stay `None`.
pub fn compute_stats(
    tracks: &[StatsTrackRow],
    genre_rows: &[StatsGenreRow],
    counts: LibraryCounts,
    root: Option<&Path>,
) -> LibraryStats {
    let mut format_counts: HashMap<String, usize> = HashMap::new();
    let mut format_sizes: HashMap<String, u64> = HashMap::new();
    let mut artist_counts: HashMap<String, usize> = HashMap::new();
    let mut ratings = RatingTally::default();
    let mut total_size: u64 = 0;
    let mut sized_any = false;
    let mut total_duration = 0.0;
    let mut fully_tagged = 0;
    let mut bitrates: Vec<u32> = Vec::new();

    for r in tracks {
        let fmt = r.format.clone().unwrap_or_else(|| "unknown".to_string());
        *format_counts.entry(fmt.clone()).or_default() += 1;

        if let Some(root) = root
            && let Ok(meta) = std::fs::metadata(root.join(&r.file_path))
        {
            let len = meta.len();
            total_size += len;
            *format_sizes.entry(fmt).or_default() += len;
            sized_any = true;
        }

        if let Some(d) = r.duration {
            total_duration += d;
        }
        if let Some(b) = r.bitrate {
            bitrates.push(b);
        }
        ratings.add(r.rating);
        if is_fully_tagged(r) {
            fully_tagged += 1;
        }
        if let Some(name) = r.artist.as_deref().filter(|s| !s.trim().is_empty()) {
            *artist_counts.entry(name.to_string()).or_default() += 1;
        }
    }

    // Format breakdown, sorted by count then name.
    let mut formats: Vec<FormatStat> = format_counts
        .into_iter()
        .map(|(format, count)| {
            let size_bytes = sized_any.then(|| format_sizes.get(&format).copied().unwrap_or(0));
            FormatStat {
                format,
                count,
                size_bytes,
            }
        })
        .collect();
    formats.sort_by(|a, b| b.count.cmp(&a.count).then(a.format.cmp(&b.format)));

    let bitrate = (!bitrates.is_empty()).then(|| {
        let sum: u64 = bitrates.iter().map(|&b| b as u64).sum();
        BitrateStat {
            avg: sum as f64 / bitrates.len() as f64,
            min: *bitrates.iter().min().unwrap(),
            max: *bitrates.iter().max().unwrap(),
            below_floor: bitrates
                .iter()
                .filter(|&&b| b < DEFAULT_BITRATE_FLOOR)
                .count(),
        }
    });

    // Genre distribution + per-genre rating tally.
    let mut genre_counts: HashMap<String, usize> = HashMap::new();
    let mut genre_tallies: HashMap<String, RatingTally> = HashMap::new();
    for g in genre_rows {
        *genre_counts.entry(g.genre.clone()).or_default() += 1;
        genre_tallies
            .entry(g.genre.clone())
            .or_default()
            .add(g.rating);
    }
    let mut genres: Vec<GenreStat> = genre_counts
        .into_iter()
        .map(|(genre, count)| GenreStat { genre, count })
        .collect();
    genres.sort_by(|a, b| b.count.cmp(&a.count).then(a.genre.cmp(&b.genre)));
    // Per-genre rating tally in the same descending-count order as `genres`.
    let genre_ratings: Vec<(String, RatingTally)> = genres
        .iter()
        .map(|g| {
            (
                g.genre.clone(),
                genre_tallies.get(&g.genre).copied().unwrap_or_default(),
            )
        })
        .collect();

    let mut top_artists: Vec<ArtistStat> = artist_counts
        .into_iter()
        .map(|(artist, count)| ArtistStat { artist, count })
        .collect();
    top_artists.sort_by(|a, b| b.count.cmp(&a.count).then(a.artist.cmp(&b.artist)));

    LibraryStats {
        total_tracks: counts.tracks,
        total_albums: counts.albums,
        total_artists: counts.artists,
        total_size_bytes: sized_any.then_some(total_size),
        total_duration_secs: total_duration,
        fully_tagged,
        formats,
        bitrate,
        ratings,
        genres,
        genre_ratings,
        top_artists,
    }
}

/// Human-readable byte count (B / KB / MB / GB), ported from Lattice
/// `_format_size`.
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn trow(
        format: &str,
        bitrate: Option<u32>,
        duration: Option<f64>,
        rating: i64,
        artist: Option<&str>,
        title: &str,
        track_no: Option<u32>,
        genre_count: i64,
    ) -> StatsTrackRow {
        StatsTrackRow {
            format: Some(format.to_string()),
            bitrate,
            duration,
            rating,
            file_path: format!("p/{title}.{format}"),
            title: title.to_string(),
            artist: artist.map(str::to_string),
            track_no,
            genre_count,
        }
    }

    fn counts() -> LibraryCounts {
        LibraryCounts {
            artists: 2,
            albums: 1,
            tracks: 3,
        }
    }

    #[test]
    fn format_and_bitrate_and_duration() {
        let tracks = vec![
            trow("mp3", Some(320), Some(100.0), 5, Some("A"), "x", Some(1), 1),
            trow("mp3", Some(128), Some(50.0), 0, Some("A"), "y", Some(2), 1),
            trow(
                "flac",
                Some(900),
                Some(200.0),
                3,
                Some("B"),
                "z",
                Some(1),
                1,
            ),
        ];
        let stats = compute_stats(&tracks, &[], counts(), None);

        assert_eq!(stats.formats[0].format, "mp3"); // most common first
        assert_eq!(stats.formats[0].count, 2);
        assert!(stats.formats[0].size_bytes.is_none(), "no root => no size");
        assert_eq!(stats.total_duration_secs, 350.0);

        let br = stats.bitrate.unwrap();
        assert_eq!(br.min, 128);
        assert_eq!(br.max, 900);
        assert!((br.avg - (320.0 + 128.0 + 900.0) / 3.0).abs() < 1e-9);
        assert_eq!(br.below_floor, 1, "only the 128 kbps track is below 192");
    }

    #[test]
    fn ratings_zero_is_unrated() {
        let tracks = vec![
            trow("mp3", None, None, 0, Some("A"), "x", Some(1), 1),
            trow("mp3", None, None, 5, Some("A"), "y", Some(2), 1),
            trow("mp3", None, None, 5, Some("A"), "z", Some(3), 1),
        ];
        let stats = compute_stats(&tracks, &[], counts(), None);
        assert_eq!(stats.ratings.unrated, 1);
        assert_eq!(stats.ratings.stars[4], 2, "two 5-star");
        assert_eq!(stats.ratings.rated(), 2);
    }

    #[test]
    fn fully_tagged_predicate() {
        let tracks = vec![
            trow("mp3", None, None, 0, Some("A"), "complete", Some(1), 1),
            trow("mp3", None, None, 0, None, "no artist", Some(2), 1),
            trow("mp3", None, None, 0, Some("A"), "no genre", Some(3), 0),
            trow("mp3", None, None, 0, Some("A"), "", Some(4), 1),
        ];
        let stats = compute_stats(&tracks, &[], counts(), None);
        assert_eq!(stats.fully_tagged, 1);
    }

    #[test]
    fn genres_and_top_artists() {
        let tracks = vec![
            trow("mp3", None, None, 5, Some("Aesop Rock"), "x", Some(1), 1),
            trow("mp3", None, None, 0, Some("Aesop Rock"), "y", Some(2), 1),
            trow("mp3", None, None, 3, Some("Aphex Twin"), "z", Some(1), 1),
        ];
        let genre_rows = vec![
            StatsGenreRow {
                genre: "Hip Hop".into(),
                rating: 5,
            },
            StatsGenreRow {
                genre: "Hip Hop".into(),
                rating: 0,
            },
            StatsGenreRow {
                genre: "Ambient".into(),
                rating: 3,
            },
        ];
        let stats = compute_stats(&tracks, &genre_rows, counts(), None);

        assert_eq!(stats.genres[0].genre, "Hip Hop");
        assert_eq!(stats.genres[0].count, 2);
        assert_eq!(stats.top_artists[0].artist, "Aesop Rock");
        assert_eq!(stats.top_artists[0].count, 2);

        let hh = &stats.genre_ratings[0];
        assert_eq!(hh.0, "Hip Hop");
        assert_eq!(hh.1.stars[4], 1);
        assert_eq!(hh.1.unrated, 1);
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }
}
