//! Lyrics for one track, read from local sources only.
//!
//! Two sources, in preference order:
//!
//! 1. An **`.lrc` sidecar** beside the audio file (`Song.flac` -> `Song.lrc`).
//!    LRC lines carry timestamps, so these can be highlighted line by line as
//!    the track plays, which is the half that makes a Now Playing surface worth
//!    watching rather than a wall of text.
//! 2. **Embedded tags** via `lofty` (already a dependency): `USLT` on ID3,
//!    `LYRICS` on Vorbis comments, `©lyr` on MP4. Unsynced.
//!
//! There is deliberately no third source. Fetching lyrics from an online
//! service would make the library non-functional offline and send the user's
//! listening to a third party, both of which the house rules rule out. If a
//! track has no local lyrics, it has no lyrics, and the surface says so plainly.
//!
//! Nothing here is cached in the database. Lyrics are read on track change,
//! which is one small file read at a point where the app is already doing IO,
//! and skipping the cache means no migration and no staleness to reconcile.

use std::path::{Path, PathBuf};

use lofty::prelude::{ItemKey, TaggedFileExt};

/// One timestamped lyric line.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncedLine {
    /// Seconds from the start of the track.
    pub at: f64,
    pub text: String,
}

/// A track's lyrics, and whether they can follow the playhead.
#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
    /// Plain text, no timing. Shown as a scrolling block.
    Unsynced(String),
    /// Timestamped lines, sorted by time. Shown with the current line lit.
    Synced(Vec<SyncedLine>),
}

impl Lyrics {
    /// True when there is nothing worth showing.
    pub fn is_empty(&self) -> bool {
        match self {
            Lyrics::Unsynced(text) => text.trim().is_empty(),
            Lyrics::Synced(lines) => lines.is_empty(),
        }
    }

    /// Index of the line that should be lit at `pos` seconds.
    ///
    /// The last line whose timestamp has passed. `None` before the first line,
    /// which is the lead-in most LRC files have, and `None` for unsynced text
    /// since there is no line to point at.
    pub fn active_line(&self, pos: f64) -> Option<usize> {
        let Lyrics::Synced(lines) = self else {
            return None;
        };
        // partition_point over a sorted list: the count of lines already begun.
        let begun = lines.partition_point(|l| l.at <= pos);
        begun.checked_sub(1)
    }
}

/// The `.lrc` path for an audio file: same stem, `.lrc` extension.
pub fn sidecar_path(audio: &Path) -> PathBuf {
    audio.with_extension("lrc")
}

/// Load lyrics for `audio`, preferring a synced `.lrc` sidecar.
///
/// Returns `None` when neither source yields anything. Never fails loudly: a
/// malformed sidecar or an unreadable tag falls through to the next source
/// rather than erroring, because missing lyrics must never break playback.
pub fn load_for(audio: &Path) -> Option<Lyrics> {
    let sidecar = sidecar_path(audio);
    if let Ok(raw) = std::fs::read_to_string(&sidecar) {
        let parsed = parse_lrc(&raw);
        if !parsed.is_empty() {
            return Some(Lyrics::Synced(parsed));
        }
    }
    embedded(audio).map(Lyrics::Unsynced)
}

/// Embedded lyrics from the file's tags, if any tag carries them.
fn embedded(audio: &Path) -> Option<String> {
    let tagged = lofty::read_from_path(audio).ok()?;
    // primary_tag first, then any tag: a file can carry both ID3v2 and Vorbis,
    // and the lyrics may live on either.
    let tags = tagged.primary_tag().into_iter().chain(tagged.tags().iter());
    for tag in tags {
        if let Some(text) = tag.get_string(&ItemKey::Lyrics) {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// Parse LRC text into timestamped lines, sorted by time.
///
/// Handles the parts of the format that appear in the wild:
///
/// - `[mm:ss.xx] text`, with centiseconds or milliseconds, or no fraction.
/// - **Several timestamps on one line** (`[00:12.00][01:30.00] chorus`), which
///   is how LRC files avoid repeating a repeated chorus. Each one becomes its
///   own line.
/// - ID tags (`[ar:...]`, `[ti:...]`, `[offset:...]`) are skipped: the bracket
///   content is not a timestamp, so it simply does not parse and is dropped.
///
/// Blank lyric text is kept when it is timestamped, because an empty line is
/// how an LRC marks an instrumental gap, and dropping it would make the
/// previous line stay lit through the whole break.
pub fn parse_lrc(raw: &str) -> Vec<SyncedLine> {
    let mut out: Vec<SyncedLine> = Vec::new();

    for line in raw.lines() {
        let mut rest = line.trim();
        let mut stamps: Vec<f64> = Vec::new();

        // Peel leading [..] groups; the first one that is not a timestamp ends
        // the run (that is an ID tag, or the start of the lyric text itself).
        while rest.starts_with('[') {
            let Some(close) = rest.find(']') else { break };
            let inside = &rest[1..close];
            let Some(secs) = parse_timestamp(inside) else {
                break;
            };
            stamps.push(secs);
            rest = rest[close + 1..].trim_start();
        }

        if stamps.is_empty() {
            continue;
        }
        let text = rest.trim().to_string();
        for at in stamps {
            out.push(SyncedLine {
                at,
                text: text.clone(),
            });
        }
    }

    // Stable sort: several timestamps can collide, and keeping file order
    // between equal stamps is more predictable than reordering them.
    out.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` to seconds. `None` if it is not one.
fn parse_timestamp(s: &str) -> Option<f64> {
    let (mins, rest) = s.split_once(':')?;
    let mins: u64 = mins.trim().parse().ok()?;
    let secs: f64 = match rest.split_once('.') {
        Some((whole, frac)) => {
            let whole: u64 = whole.parse().ok()?;
            // Two digits are centiseconds, three are milliseconds. Anything
            // else is not a timestamp we understand.
            let frac_val: f64 = frac.parse().ok()?;
            let scale = match frac.len() {
                2 => 100.0,
                3 => 1000.0,
                _ => return None,
            };
            whole as f64 + frac_val / scale
        }
        None => rest.trim().parse().ok()?,
    };
    Some(mins as f64 * 60.0 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_timestamped_lines() {
        let lines = parse_lrc("[00:12.00]first\n[00:15.50]second");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at, 12.0);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].at, 15.5);
    }

    #[test]
    fn one_line_with_several_timestamps_becomes_several_lines() {
        // How an LRC avoids writing a repeated chorus out twice.
        let lines = parse_lrc("[00:10.00][01:00.00]chorus");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at, 10.0);
        assert_eq!(lines[1].at, 60.0);
        assert_eq!(lines[1].text, "chorus");
    }

    #[test]
    fn id_tags_are_skipped_not_parsed_as_lyrics() {
        let lines = parse_lrc("[ar:Someone]\n[ti:A Song]\n[00:05.00]real line");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "real line");
    }

    #[test]
    fn milliseconds_and_bare_seconds_both_parse() {
        assert_eq!(parse_timestamp("01:02.500"), Some(62.5));
        assert_eq!(parse_timestamp("01:02"), Some(62.0));
        assert_eq!(parse_timestamp("00:02.25"), Some(2.25));
    }

    #[test]
    fn nonsense_timestamps_are_rejected() {
        assert_eq!(parse_timestamp("ar:Someone"), None);
        assert_eq!(parse_timestamp("not a stamp"), None);
        assert_eq!(parse_timestamp("00:02.1234"), None);
    }

    #[test]
    fn instrumental_gaps_keep_their_empty_line() {
        // An empty timestamped line ends the previous lyric. Dropping it would
        // leave the last line lit through the whole instrumental break.
        let lines = parse_lrc("[00:10.00]words\n[00:20.00]\n[00:30.00]more");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "");
    }

    #[test]
    fn lines_come_back_sorted_even_when_the_file_is_not() {
        let lines = parse_lrc("[01:00.00]late\n[00:10.00]early");
        assert_eq!(lines[0].text, "early");
        assert_eq!(lines[1].text, "late");
    }

    #[test]
    fn active_line_tracks_the_playhead() {
        let lyrics = Lyrics::Synced(parse_lrc("[00:10.00]a\n[00:20.00]b\n[00:30.00]c"));
        assert_eq!(lyrics.active_line(0.0), None, "lead-in lights nothing");
        assert_eq!(lyrics.active_line(9.9), None);
        assert_eq!(
            lyrics.active_line(10.0),
            Some(0),
            "lights exactly on the stamp"
        );
        assert_eq!(lyrics.active_line(19.9), Some(0));
        assert_eq!(lyrics.active_line(20.0), Some(1));
        assert_eq!(lyrics.active_line(999.0), Some(2), "last line stays lit");
    }

    #[test]
    fn unsynced_lyrics_have_no_active_line() {
        assert_eq!(Lyrics::Unsynced("words".into()).active_line(10.0), None);
    }

    #[test]
    fn emptiness_is_reported_honestly() {
        assert!(Lyrics::Unsynced("   \n ".into()).is_empty());
        assert!(!Lyrics::Unsynced("a".into()).is_empty());
        assert!(Lyrics::Synced(vec![]).is_empty());
    }

    #[test]
    fn sidecar_path_swaps_the_extension() {
        assert_eq!(
            sidecar_path(Path::new("/m/Song.flac")),
            Path::new("/m/Song.lrc")
        );
        assert_eq!(
            sidecar_path(Path::new("/m/A.Song.mp3")),
            Path::new("/m/A.Song.lrc")
        );
    }

    #[test]
    fn a_missing_file_yields_no_lyrics_rather_than_an_error() {
        assert_eq!(load_for(Path::new("/nonexistent/track.flac")), None);
    }
}
