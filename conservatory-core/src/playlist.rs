//! `.m3u` / `.m3u8` playlist rendering and parsing (Phase 8d).
//!
//! Pure format helpers, modeled on Lattice's `--playlist` output: an extended
//! M3U is an `#EXTM3U` header followed by, per track, an `#EXTINF:<seconds>,
//! <Artist> - <Title>` line and then the file path. Conservatory exports its
//! managed tracks this way and re-imports any `.m3u` by reading the path lines.
//!
//! These functions never touch the filesystem or the database; the CLI verbs
//! orchestrate (resolve a selector to tracks, choose a path style, queue the
//! resolved ids). Keeping the format pure makes it CLI-testable (spec §2.2).

/// A track to render into an extended-M3U entry. `duration_secs` is the audio
/// length (seconds); `None` renders the conventional `-1` (unknown). `artist`
/// is the display artist (track artist, falling back to the album artist).
#[derive(Debug, Clone, PartialEq)]
pub struct M3uTrack {
    pub duration_secs: Option<f64>,
    pub artist: Option<String>,
    pub title: String,
    pub path: String,
}

/// Render an extended-M3U document: the `#EXTM3U` header, then an
/// `#EXTINF`/path pair per track. The display is `"Artist - Title"`, or just
/// the title when the artist is unknown (mirrors Lattice).
pub fn build_m3u(tracks: &[M3uTrack]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let secs = t.duration_secs.map(|d| d as i64).unwrap_or(-1);
        let display = match t.artist.as_deref() {
            Some(a) if !a.is_empty() => format!("{a} - {}", t.title),
            _ => t.title.clone(),
        };
        out.push_str(&format!("#EXTINF:{secs},{display}\n{}\n", t.path));
    }
    out
}

/// Extract the path lines from an `.m3u`: every non-empty line that is not a
/// `#`-comment (so `#EXTM3U` and `#EXTINF` are skipped), trimmed, in file
/// order. Import is deliberately liberal: the metadata lines are advisory; the
/// paths are what map back to managed tracks.
pub fn parse_m3u(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(duration: Option<f64>, artist: Option<&str>, title: &str, path: &str) -> M3uTrack {
        M3uTrack {
            duration_secs: duration,
            artist: artist.map(str::to_string),
            title: title.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn build_renders_header_extinf_and_paths() {
        let m3u = build_m3u(&[
            t(Some(213.7), Some("Aesop Rock"), "Daylight", "a/01.mp3"),
            t(None, None, "Untitled", "b/02.flac"),
        ]);
        assert_eq!(
            m3u,
            "#EXTM3U\n\
             #EXTINF:213,Aesop Rock - Daylight\n\
             a/01.mp3\n\
             #EXTINF:-1,Untitled\n\
             b/02.flac\n"
        );
    }

    #[test]
    fn parse_skips_header_extinf_comments_and_blanks_keeping_order() {
        let text = "#EXTM3U\n\
                    #EXTINF:213,Aesop Rock - Daylight\n\
                    a/01.mp3\n\
                    \n\
                    # a stray comment\n\
                    b/02.flac\n";
        assert_eq!(parse_m3u(text), vec!["a/01.mp3", "b/02.flac"]);
    }

    #[test]
    fn build_then_parse_round_trips_the_paths() {
        let tracks = vec![
            t(Some(1.0), Some("X"), "One", "dir/one.mp3"),
            t(Some(2.0), Some("Y"), "Two", "dir/two.opus"),
            t(Some(3.0), None, "Three", "dir/three.flac"),
        ];
        let paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();
        assert_eq!(parse_m3u(&build_m3u(&tracks)), paths);
    }
}
