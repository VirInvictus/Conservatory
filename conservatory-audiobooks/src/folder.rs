//! Folder-structure inference (Phase 7a-ii, spec §5.7).
//!
//! The lowest-precedence metadata source: when neither the embedded tags nor a
//! sidecar give an author / series / title, the on-disk layout often does. The
//! two input conventions:
//!
//! - `Author/Title/` -> a standalone book;
//! - `Author/Series/NN - Title/` -> a series entry (`NN` is the decimal sequence).
//!
//! A series entry is recognised conservatively: the title folder must carry a
//! leading index *and* have a grandparent. A bare numeric folder (`1984`, no
//! separator + remainder) is not an index, so a numerically-titled standalone is
//! not mistaken for a series entry. Best-effort by nature, so it only fills gaps.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderInfo {
    pub author: Option<String>,
    pub series: Option<String>,
    pub title: Option<String>,
    pub sequence: Option<f64>,
}

/// Infer author / series / title / sequence from a book folder's path.
pub fn infer(book_dir: &Path) -> FolderInfo {
    let leaf = match component(book_dir, 0) {
        Some(c) => c,
        None => return FolderInfo::default(),
    };
    let parent = component(book_dir, 1);
    let grandparent = component(book_dir, 2);

    let (sequence, stripped) = parse_indexed_title(&leaf);

    // A series entry: a leading index plus a grandparent to be the author.
    if sequence.is_some() && grandparent.is_some() {
        FolderInfo {
            author: grandparent,
            series: parent,
            title: Some(stripped),
            sequence,
        }
    } else {
        // The managed tree renders a series-less book as
        // `Author/Standalone/Title/` (spec §5.7), so a literal `Standalone`
        // parent is the series level, not the author; the real author sits
        // one level up. (Found by the 2026-09-11 functional pass: a
        // re-import of a standalone book from Conservatory's own tree
        // resolved the author as "Standalone".)
        if parent.as_deref() == Some(conservatory_core::STANDALONE) {
            FolderInfo {
                author: grandparent,
                series: None,
                title: Some(leaf),
                sequence: None,
            }
        } else {
            FolderInfo {
                author: parent,
                series: None,
                title: Some(leaf),
                sequence: None,
            }
        }
    }
}

/// The nth path component from the end (`0` = the leaf), as a string.
fn component(path: &Path, from_end: usize) -> Option<String> {
    path.components()
        .rev()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .nth(from_end)
}

/// Split a leading decimal index off a folder name: `"1 - Title"` -> `(1.0,
/// "Title")`, `"01. Title"` -> `(1.0, "Title")`, `"1.5 - Title"` -> `(1.5,
/// "Title")`. A name with no separated remainder (`"1984"`) yields `(None, name)`.
fn parse_indexed_title(name: &str) -> (Option<f64>, String) {
    let trimmed = name.trim();
    let consumed: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let numeric = consumed.trim_end_matches('.');
    if numeric.is_empty() {
        return (None, trimmed.to_string());
    }
    if let Ok(seq) = numeric.parse::<f64>() {
        let rest = trimmed[consumed.len()..]
            .trim_start_matches([' ', '.', '-', '_'])
            .trim();
        if !rest.is_empty() {
            return (Some(seq), rest.to_string());
        }
    }
    (None, trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn series_entry_three_levels() {
        let p = PathBuf::from(
            "/lib/Patrick Rothfuss/The Kingkiller Chronicle/1 - The Name of the Wind",
        );
        let info = infer(&p);
        assert_eq!(info.author.as_deref(), Some("Patrick Rothfuss"));
        assert_eq!(info.series.as_deref(), Some("The Kingkiller Chronicle"));
        assert_eq!(info.title.as_deref(), Some("The Name of the Wind"));
        assert_eq!(info.sequence, Some(1.0));
    }

    #[test]
    fn standalone_two_levels() {
        let p = PathBuf::from("/lib/Neil Gaiman/The Graveyard Book - Full-Cast Production");
        let info = infer(&p);
        assert_eq!(info.author.as_deref(), Some("Neil Gaiman"));
        assert_eq!(info.series, None);
        assert_eq!(
            info.title.as_deref(),
            Some("The Graveyard Book - Full-Cast Production")
        );
        assert_eq!(info.sequence, None);
    }

    #[test]
    fn numeric_titled_standalone_is_not_a_series() {
        // "1984" is a title, not an index (no separated remainder).
        let p = PathBuf::from("/lib/George Orwell/1984");
        let info = infer(&p);
        assert_eq!(info.author.as_deref(), Some("George Orwell"));
        assert_eq!(info.series, None);
        assert_eq!(info.title.as_deref(), Some("1984"));
        assert_eq!(info.sequence, None);
    }

    #[test]
    fn standalone_literal_level_is_not_the_author() {
        // The managed tree renders a series-less book as
        // `Author/Standalone/Title/` (spec §5.7); reading that tree back must
        // resolve the real author, not the literal level.
        let p = PathBuf::from("/lib/Test Author/Standalone/The Fixture Method");
        let info = infer(&p);
        assert_eq!(info.author.as_deref(), Some("Test Author"));
        assert_eq!(info.series, None);
        assert_eq!(info.title.as_deref(), Some("The Fixture Method"));
        assert_eq!(info.sequence, None);
    }

    #[test]
    fn standalone_literal_at_the_root_has_no_author() {
        // `Standalone/Title/` with nothing above the level: no author to find
        // (the import falls through to Unknown Author), and the level itself
        // must not become one.
        let p = PathBuf::from("/Standalone/Some Book");
        let info = infer(&p);
        assert_eq!(info.author, None);
        assert_eq!(info.series, None);
        assert_eq!(info.title.as_deref(), Some("Some Book"));
    }

    #[test]
    fn decimal_index_parses() {
        assert_eq!(
            parse_indexed_title("1.5 - Interlude"),
            (Some(1.5), "Interlude".to_string())
        );
        assert_eq!(
            parse_indexed_title("01. Title"),
            (Some(1.0), "Title".to_string())
        );
        assert_eq!(parse_indexed_title("Title"), (None, "Title".to_string()));
    }
}
