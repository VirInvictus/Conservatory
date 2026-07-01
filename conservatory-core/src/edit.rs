//! Pure metadata-edit resolution (Phase 5a, spec §3.5).
//!
//! Turns user `field=value` assignments (and search-and-replace) into the typed
//! track-level and album-level updates the worker applies, and classifies which
//! edits are *path-affecting* (they change the rendered tree, so the caller
//! follows with a move job, §5.4). No DB or I/O here: the CLI and the GTK dialog
//! share this logic, which keeps it unit-testable headless.
//!
//! Cover editing is Phase 5d (`albums.cover_path` is unpopulated until then) and
//! embedded write-back is Phase 5b; neither is an editable field here.

use crate::errors::{Error, Result};

/// An editable metadata field (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Track title.
    Title,
    /// Track artist (reassigns `tracks.artist_id`, get-or-create by sort name).
    Artist,
    /// Album title.
    Album,
    /// Album artist (reassigns `albums.album_artist_id`).
    AlbumArtist,
    /// Album release year.
    Year,
    /// The single-valued filed-under genre (`albums.shelf_genre`, §5.2).
    ShelfGenre,
    /// Raw multi-value genres (`track_genres`, §5.2); never reaches the tree.
    Genre,
    /// Track rating, 0..=5.
    Rating,
}

impl Field {
    /// Parse a field name (CLI/GUI accept a couple of spellings).
    pub fn parse(name: &str) -> Option<Field> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "title" => Field::Title,
            "artist" => Field::Artist,
            "album" => Field::Album,
            "albumartist" | "album_artist" | "album-artist" => Field::AlbumArtist,
            "year" => Field::Year,
            "shelfgenre" | "shelf_genre" | "shelf-genre" => Field::ShelfGenre,
            "genre" | "genres" => Field::Genre,
            "rating" => Field::Rating,
            _ => return None,
        })
    }

    /// Album-level fields update the album row (and thus the whole album); the
    /// rest update each selected track.
    pub fn is_album_level(self) -> bool {
        matches!(
            self,
            Field::Album | Field::AlbumArtist | Field::Year | Field::ShelfGenre
        )
    }

    /// Changing these re-renders the default path template, so a move job
    /// follows (spec §5.1, §5.4). Track artist and raw genres do not.
    pub fn is_path_affecting(self) -> bool {
        matches!(
            self,
            Field::Album | Field::AlbumArtist | Field::Year | Field::ShelfGenre
        )
    }
}

/// One parsed `field=value` assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub field: Field,
    pub value: String,
}

/// Parse a `field=value` token, validating numeric fields up front so a bad
/// assignment fails before any write (spec §3.5).
pub fn parse_assignment(s: &str) -> Result<Assignment> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| Error::Edit(format!("expected field=value, got {s:?}")))?;
    let field = Field::parse(key).ok_or_else(|| Error::Edit(format!("unknown field {key:?}")))?;
    let value = value.to_string();
    match field {
        Field::Year => {
            value
                .trim()
                .parse::<i32>()
                .map_err(|_| Error::Edit(format!("year must be an integer, got {value:?}")))?;
        }
        Field::Rating => {
            let r: u8 = value
                .trim()
                .parse()
                .map_err(|_| Error::Edit(format!("rating must be 0..=5, got {value:?}")))?;
            if r > 5 {
                return Err(Error::Edit(format!("rating must be 0..=5, got {r}")));
            }
        }
        _ => {}
    }
    Ok(Assignment { field, value })
}

/// Track-level field changes (each `Some` is set; `None` leaves it unchanged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub rating: Option<u8>,
}

impl TrackEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.artist.is_none() && self.rating.is_none()
    }
}

/// Album-level field changes (each `Some` is set; `None` leaves it unchanged).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumEdit {
    pub title: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<i32>,
    pub shelf_genre: Option<String>,
}

impl AlbumEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.album_artist.is_none()
            && self.year.is_none()
            && self.shelf_genre.is_none()
    }
}

/// Collect the track-level assignments into a `TrackEdit` (later wins on dupes).
pub fn build_track_edit(assignments: &[Assignment]) -> TrackEdit {
    let mut edit = TrackEdit::default();
    for a in assignments {
        match a.field {
            Field::Title => edit.title = Some(a.value.clone()),
            Field::Artist => edit.artist = Some(a.value.clone()),
            Field::Rating => edit.rating = a.value.trim().parse().ok(),
            _ => {}
        }
    }
    edit
}

/// Collect the album-level assignments into an `AlbumEdit` (later wins on dupes).
pub fn build_album_edit(assignments: &[Assignment]) -> AlbumEdit {
    let mut edit = AlbumEdit::default();
    for a in assignments {
        match a.field {
            Field::Album => edit.title = Some(a.value.clone()),
            Field::AlbumArtist => edit.album_artist = Some(a.value.clone()),
            Field::Year => edit.year = a.value.trim().parse().ok(),
            Field::ShelfGenre => edit.shelf_genre = Some(a.value.clone()),
            _ => {}
        }
    }
    edit
}

/// The new raw-genre set if a `genre=` assignment is present (last one wins);
/// `None` means genres are untouched.
pub fn genres_assignment(assignments: &[Assignment]) -> Option<Vec<String>> {
    assignments
        .iter()
        .rev()
        .find(|a| a.field == Field::Genre)
        .map(|a| split_genres(&a.value))
}

/// Split a raw multi-genre value on the tag delimiters, trimming and dropping
/// empties while preserving order and first-seen case (the §5.2 raw side; no
/// normalization, that is the shelf-genre resolver's job).
pub fn split_genres(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in value.split([';', '/', ',']) {
        let g = part.trim();
        if !g.is_empty() && !out.iter().any(|e| e.eq_ignore_ascii_case(g)) {
            out.push(g.to_string());
        }
    }
    out
}

/// Whether any assignment in the set is path-affecting (the caller then runs a
/// scoped move job for the touched albums).
pub fn any_path_affecting(assignments: &[Assignment]) -> bool {
    assignments.iter().any(|a| a.field.is_path_affecting())
}

/// Literal search-and-replace within a single field value (Phase 5a).
pub fn replace_in(current: &str, find: &str, replace: &str) -> String {
    current.replace(find, replace)
}

/// The value shared by every entry in `vals`, or `None` when they differ (the
/// bulk-edit "multiple values" state, Phase 16c; promoted to core at 16.5g so
/// the music and audiobook editors share one collapse). An empty selection
/// collapses to a shared empty string. Pure.
pub fn common_value(mut vals: Vec<String>) -> Option<String> {
    match vals.pop() {
        None => Some(String::new()),
        Some(first) if vals.iter().all(|v| *v == first) => Some(first),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_value_agrees_or_reports_mixed() {
        // All the same collapses to that value (the shared prefill).
        assert_eq!(
            common_value(vec!["Aphex Twin".into(), "Aphex Twin".into()]),
            Some("Aphex Twin".into())
        );
        // A single row is trivially "shared".
        assert_eq!(common_value(vec!["Solo".into()]), Some("Solo".into()));
        // Differing values are "multiple values" (None).
        assert_eq!(common_value(vec!["A".into(), "B".into()]), None);
        // All-empty is a shared empty string, not mixed.
        assert_eq!(
            common_value(vec![String::new(), String::new()]),
            Some(String::new())
        );
        // An empty selection collapses to empty, not mixed.
        assert_eq!(common_value(vec![]), Some(String::new()));
    }

    #[test]
    fn field_parse_aliases() {
        assert_eq!(Field::parse("Album Artist".trim()), None); // space, not an alias
        assert_eq!(Field::parse("albumartist"), Some(Field::AlbumArtist));
        assert_eq!(Field::parse("album_artist"), Some(Field::AlbumArtist));
        assert_eq!(Field::parse("SHELFGENRE"), Some(Field::ShelfGenre));
        assert_eq!(Field::parse("genres"), Some(Field::Genre));
        assert_eq!(Field::parse("nope"), None);
    }

    #[test]
    fn album_level_and_path_affecting() {
        for f in [
            Field::Album,
            Field::AlbumArtist,
            Field::Year,
            Field::ShelfGenre,
        ] {
            assert!(f.is_album_level());
            assert!(f.is_path_affecting());
        }
        for f in [Field::Title, Field::Artist, Field::Genre, Field::Rating] {
            assert!(!f.is_album_level());
            assert!(!f.is_path_affecting());
        }
    }

    #[test]
    fn parse_assignment_validates_numbers() {
        assert_eq!(
            parse_assignment("year=1992").unwrap(),
            Assignment {
                field: Field::Year,
                value: "1992".into()
            }
        );
        assert!(parse_assignment("year=nope").is_err());
        assert!(parse_assignment("rating=5").is_ok());
        assert!(parse_assignment("rating=6").is_err());
        assert!(parse_assignment("noequals").is_err());
        assert!(parse_assignment("bogus=x").is_err());
        // an empty value is allowed for text fields (clearing-by-blank is a set).
        assert_eq!(parse_assignment("title=").unwrap().value, "");
    }

    #[test]
    fn builders_split_track_and_album() {
        let asg = vec![
            parse_assignment("title=New Title").unwrap(),
            parse_assignment("rating=4").unwrap(),
            parse_assignment("album=New Album").unwrap(),
            parse_assignment("year=2001").unwrap(),
        ];
        let t = build_track_edit(&asg);
        assert_eq!(t.title.as_deref(), Some("New Title"));
        assert_eq!(t.rating, Some(4));
        assert!(t.artist.is_none());
        let a = build_album_edit(&asg);
        assert_eq!(a.title.as_deref(), Some("New Album"));
        assert_eq!(a.year, Some(2001));
        assert!(a.album_artist.is_none() && a.shelf_genre.is_none());
        assert!(any_path_affecting(&asg));
    }

    #[test]
    fn genres_split_dedupe_case_insensitive() {
        let asg = vec![parse_assignment("genre=Electronic; Ambient / electronic").unwrap()];
        assert_eq!(
            genres_assignment(&asg),
            Some(vec!["Electronic".to_string(), "Ambient".to_string()])
        );
        assert_eq!(split_genres("  ,  ; "), Vec::<String>::new());
    }

    #[test]
    fn replace_is_literal() {
        assert_eq!(replace_in("Disc 1 - Track", "Disc 1 - ", ""), "Track");
        assert_eq!(replace_in("aXbXc", "X", "_"), "a_b_c");
    }

    #[test]
    fn empty_edits() {
        assert!(TrackEdit::default().is_empty());
        assert!(AlbumEdit::default().is_empty());
        assert!(!build_track_edit(&[parse_assignment("title=x").unwrap()]).is_empty());
    }
}
