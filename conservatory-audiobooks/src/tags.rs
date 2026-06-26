//! Embedded-tag reader for audiobooks (Phase 7a-ii).
//!
//! Reads one audio file's embedded tags into a [`BookTags`] (the raw,
//! pre-merge view; every field is optional). Modeled on
//! `conservatory-core/src/tags.rs` `read_track`, but the field mapping is the
//! audiobook convention, validated against the real testdata:
//!
//! - the **book title** lives in the album tag (`AlbumTitle`), not the track
//!   title (which is the chapter/part name);
//! - the **author** is the album artist;
//! - the **narrator** is the composer (both testdata books carry it there) plus
//!   a custom `NARRATOR` frame;
//! - **series** / **series sequence** are custom `SERIES` / `SERIES-PART` frames.
//!
//! Custom frames surface through lofty's unified tag as
//! [`ItemKey::Unknown`]: an ID3v2 `TXXX:NARRATOR` is `Unknown("NARRATOR")`,
//! while an MP4 freeform atom is `Unknown("----:com.apple.iTunes:NARRATOR")`.
//! [`custom`] tries both spellings so one reader covers mp3 and m4b.

use std::path::Path;

use lofty::prelude::{Accessor, ItemKey, TaggedFileExt};
use lofty::tag::Tag;

use crate::error::Result;

/// The raw tag view of one audiobook file, before sidecar/folder merge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookTags {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub series: Option<String>,
    pub series_sequence: Option<f64>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    /// The part/track number, used to order a multi-file M4B's parts.
    pub part_no: Option<u32>,
    pub cover: Option<Vec<u8>>,
}

/// Read one file's embedded tags into a [`BookTags`]. An untagged but readable
/// file yields an all-empty `BookTags`, never an error.
pub fn read_book_tags(path: &Path) -> Result<BookTags> {
    let tagged = lofty::read_from_path(path)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let mut t = BookTags::default();
    if let Some(tag) = tag {
        fill(&mut t, tag);
    }
    Ok(t)
}

fn fill(t: &mut BookTags, tag: &Tag) {
    // The book title is the album tag; fall back to the track title.
    t.title = tag
        .album()
        .map(|c| c.to_string())
        .or_else(|| tag.title().map(|c| c.to_string()));
    t.subtitle = tag.get_string(&ItemKey::TrackSubtitle).map(str::to_string);

    // Author = album artist (fall back to the track artist), split on credits.
    let author = tag
        .get_string(&ItemKey::AlbumArtist)
        .map(str::to_string)
        .or_else(|| tag.artist().map(|c| c.to_string()));
    t.authors = author.as_deref().map(split_people).unwrap_or_default();

    // Narrator = composer (present in both testdata books) plus any custom
    // NARRATOR frame; merge and de-duplicate.
    let mut narrators = Vec::new();
    if let Some(c) = tag.get_string(&ItemKey::Composer) {
        narrators.extend(split_people(c));
    }
    if let Some(n) = custom(tag, "NARRATOR") {
        for name in split_people(&n) {
            if !narrators.contains(&name) {
                narrators.push(name);
            }
        }
    }
    t.narrators = narrators;

    t.series = custom(tag, "SERIES");
    t.series_sequence = custom(tag, "SERIES-PART")
        .or_else(|| custom(tag, "SERIES-INDEX"))
        .as_deref()
        .and_then(parse_decimal);

    t.year = tag
        .year()
        .map(|y| y as i32)
        .or_else(|| tag.get_string(&ItemKey::RecordingDate).and_then(parse_year));
    t.publisher = tag.get_string(&ItemKey::Publisher).map(str::to_string);
    t.isbn = custom(tag, "ISBN");
    t.asin = custom(tag, "ASIN");
    t.description = tag.get_string(&ItemKey::Comment).map(str::to_string);
    t.language = tag.get_string(&ItemKey::Language).map(str::to_string);
    t.part_no = tag.track();
    t.cover = tag.pictures().first().map(|p| p.data().to_vec());
}

/// Read a non-standard frame by description, covering both the ID3v2 `TXXX`
/// spelling (`Unknown("NARRATOR")`) and the MP4 freeform spelling
/// (`Unknown("----:com.apple.iTunes:NARRATOR")`).
fn custom(tag: &Tag, name: &str) -> Option<String> {
    tag.get_string(&ItemKey::Unknown(name.to_string()))
        .or_else(|| tag.get_string(&ItemKey::Unknown(format!("----:com.apple.iTunes:{name}"))))
        .map(str::to_string)
}

/// Split a credit string into individual people. Audiobook tags pack multiple
/// authors/narrators into one field with mixed separators (the full-cast M4B is
/// `"A, B, C & D"`); a single name (`"Neil Gaiman"`) stays one entry.
pub fn split_people(raw: &str) -> Vec<String> {
    raw.split([',', ';', '&'])
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a decimal series index (`"1"`, `"1.5"`); a non-numeric value is `None`.
fn parse_decimal(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

/// Parse a four-digit year from the head of a date string (`"2009"`,
/// `"2009-04-01"`).
fn parse_year(s: &str) -> Option<i32> {
    s.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_people_single_and_multi() {
        assert_eq!(split_people("Neil Gaiman"), vec!["Neil Gaiman"]);
        assert_eq!(split_people("Nick Podehl"), vec!["Nick Podehl".to_string()]);
        assert_eq!(
            split_people(
                "Neil Gaiman, Derek Jacobi, Robert Madge, Clare Corbett, Miriam Margolyes, Andrew Scott & Julian Rhind-Tutt"
            ),
            vec![
                "Neil Gaiman",
                "Derek Jacobi",
                "Robert Madge",
                "Clare Corbett",
                "Miriam Margolyes",
                "Andrew Scott",
                "Julian Rhind-Tutt",
            ]
        );
    }

    #[test]
    fn split_people_and_separator() {
        assert_eq!(
            split_people("Kvothe and Bast"),
            vec!["Kvothe".to_string(), "Bast".to_string()]
        );
    }

    #[test]
    fn parse_decimal_and_year() {
        assert_eq!(parse_decimal("1"), Some(1.0));
        assert_eq!(parse_decimal("1.5"), Some(1.5));
        assert_eq!(parse_decimal("nope"), None);
        assert_eq!(parse_year("2009"), Some(2009));
        assert_eq!(parse_year("2014-03-01"), Some(2014));
        assert_eq!(parse_year("xx"), None);
    }
}
