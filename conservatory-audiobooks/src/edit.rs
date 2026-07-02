//! Pure audiobook metadata-edit resolution (Phase 7b-iii, spec §3.5).
//!
//! The book-shaped twin of `conservatory-core`'s music `edit` module: it carries
//! the typed edit deltas and classifies which of them are **path-affecting** (they
//! change the rendered folder, so the caller follows with a book reorganize move,
//! §5.4, §5.7). No DB or I/O here, so the CLI and the GTK dialog share it and it
//! stays unit-testable headless.
//!
//! `authors`/`narrators` **replace** the whole credited set (the `set_track_genres`
//! semantic); `series: Some(Clear)` makes a book standalone (it re-shelves under
//! `Audiobooks/<author>/Standalone/…`); every `None` leaves a field untouched.

use std::path::PathBuf;

use conservatory_core::db::models::Book;
use conservatory_core::{BookFields, PathTemplate};

use crate::person_sort_name;

/// How a book's series changes; `None` at the [`BookEdit`] level means unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesEdit {
    /// Set / change the series name (the id is resolved by the caller).
    Set(String),
    /// Remove the series — the book becomes standalone.
    Clear,
}

/// Typed book-edit deltas; each `None` leaves the field unchanged.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookEdit {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub series: Option<SeriesEdit>,
    pub series_index: Option<f64>,
    /// Replace the credited authors (display names; the caller resolves ids).
    pub authors: Option<Vec<String>>,
    /// Replace the credited narrators.
    pub narrators: Option<Vec<String>>,
    pub shelf_genre: Option<String>,
    pub rating: Option<u8>,
    pub starred: Option<bool>,
}

impl BookEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.year.is_none()
            && self.series.is_none()
            && self.series_index.is_none()
            && self.authors.is_none()
            && self.narrators.is_none()
            && self.shelf_genre.is_none()
            && self.rating.is_none()
            && self.starred.is_none()
    }

    /// Whether the edit changes the rendered folder, so a reorganize move follows
    /// (spec §5.7). The default template is
    /// `Audiobooks/{author}/{series}/{series_index}. {title} ({year})`, so author,
    /// series, series index, title and year are path-affecting; narrator,
    /// shelf genre, rating and starred are not (they never reach the tree).
    pub fn is_path_affecting(&self) -> bool {
        self.title.is_some()
            || self.year.is_some()
            || self.series.is_some()
            || self.series_index.is_some()
            || self.authors.is_some()
    }
}

/// Split a `;`-separated people value into display names, trimming, dropping
/// empties, and de-duplicating case-insensitively while preserving order. Used by
/// the author / narrator fields (the multi-value replace set).
pub fn split_people(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in value.split(';') {
        let name = part.trim();
        if !name.is_empty() && !out.iter().any(|e| e.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    }
    out
}

/// The value shared across a shelf selection for each bulk-edit field (16.5g,
/// the music `bulk_edit_commons` twin): `Some(shared)` pre-fills the dialog
/// entry, `None` reads "multiple values". `shelf_genres` rides alongside
/// because `BookListRow` does not carry the shelf genre (the caller resolves
/// it per book). The people displays are `", "`-joined by the read; the
/// prefill converts to the dialog's `";"`-separated form so a ticked write
/// round-trips through `split_people`. Pure.
pub fn book_edit_commons(
    rows: &[conservatory_core::db::BookListRow],
    shelf_genres: Vec<String>,
) -> std::collections::HashMap<&'static str, Option<String>> {
    use conservatory_core::common_value;

    fn collect(
        rows: &[conservatory_core::db::BookListRow],
        f: impl Fn(&conservatory_core::db::BookListRow) -> String,
    ) -> Vec<String> {
        rows.iter().map(f).collect()
    }
    let people = |s: &Option<String>| s.clone().unwrap_or_default().replace(", ", "; ");

    let mut out = std::collections::HashMap::new();
    out.insert(
        "author",
        common_value(collect(rows, |r| people(&r.author_display))),
    );
    out.insert(
        "narrator",
        common_value(collect(rows, |r| people(&r.narrator_display))),
    );
    out.insert(
        "series",
        common_value(collect(rows, |r| r.series_name.clone().unwrap_or_default())),
    );
    out.insert(
        "series_index",
        common_value(collect(rows, |r| {
            r.series_sequence.map(fmt_series_index).unwrap_or_default()
        })),
    );
    out.insert("title", common_value(collect(rows, |r| r.title.clone())));
    out.insert(
        "year",
        common_value(collect(rows, |r| {
            r.year.map(|y| y.to_string()).unwrap_or_default()
        })),
    );
    out.insert("shelfgenre", common_value(shelf_genres));
    out.insert(
        "rating",
        common_value(collect(rows, |r| r.rating.to_string())),
    );
    out
}

/// A series index for display/prefill: whole numbers drop the fraction
/// (`1`, not `1.0`; `1.5` stays).
fn fmt_series_index(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        v.to_string()
    }
}

/// Parse an optional year entry: a blank string is "unchanged" (`Ok(None)`), a
/// valid integer is `Ok(Some(_))`, anything else is an error (for the GTK dialog;
/// the CLI gets a typed `i32` from clap).
pub fn parse_opt_year(s: &str) -> Result<Option<i32>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<i32>()
        .map(Some)
        .map_err(|_| format!("year must be an integer, got {s:?}"))
}

/// Parse an optional series-index entry (decimal allowed: `1`, `1.5`).
pub fn parse_opt_index(s: &str) -> Result<Option<f64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<f64>()
        .map(Some)
        .map_err(|_| format!("series index must be a number, got {s:?}"))
}

/// Parse an optional rating entry (0..=5).
pub fn parse_opt_rating(s: &str) -> Result<Option<u8>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u8>() {
        Ok(r) if r <= 5 => Ok(Some(r)),
        _ => Err(format!("rating must be 0..=5, got {s:?}")),
    }
}

/// Render the managed folder a book would land in after `edit` is applied, without
/// touching the database (the CLI dry-run preview). The author component is the
/// first author's sort name: from `edit.authors[0]` when the authors change, else
/// the caller-supplied current first-author sort name. A cleared or absent series
/// collapses to the `Standalone` bucket via the template.
pub fn rendered_folder(
    current: &Book,
    current_series: Option<&str>,
    current_first_author_sort: Option<&str>,
    edit: &BookEdit,
) -> PathBuf {
    let title = edit.title.as_deref().unwrap_or(current.title.as_str());
    let year = edit.year.or(current.year);
    let series_index = edit.series_index.or(current.series_sequence);

    let series: Option<String> = match &edit.series {
        Some(SeriesEdit::Set(name)) => Some(name.clone()),
        Some(SeriesEdit::Clear) => None,
        None => current_series.map(str::to_string),
    };
    let author_sort: Option<String> = match &edit.authors {
        Some(list) => list.first().map(|n| person_sort_name(n)),
        None => current_first_author_sort.map(str::to_string),
    };

    let fields = BookFields {
        shelf_genre: None,
        author: author_sort.as_deref(),
        narrator: None,
        series: series.as_deref(),
        series_index,
        title: Some(title),
        year,
    };
    PathTemplate::default_audiobook().render_book(&fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn book() -> Book {
        Book {
            id: 1,
            title: "The Way of Kings".into(),
            subtitle: None,
            series_id: Some(7),
            series_sequence: Some(1.0),
            year: Some(2010),
            publisher: None,
            isbn: None,
            asin: None,
            description: None,
            language: None,
            shelf_genre: None,
            cover_path: None,
            accent_rgb: None,
            folder_path:
                "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)"
                    .into(),
            rating: 4,
            starred: false,
            added_at: Some(Utc::now()),
        }
    }

    #[test]
    fn book_edit_commons_collapses_shared_and_reports_mixed() {
        use conservatory_core::db::BookListRow;
        let row = |title: &str, author: Option<&str>, year: Option<i32>| BookListRow {
            id: 1,
            title: title.to_string(),
            subtitle: None,
            author_display: author.map(str::to_string),
            narrator_display: Some("Kate Reading, Michael Kramer".into()),
            series_name: Some("The Stormlight Archive".into()),
            series_sequence: Some(1.5),
            year,
            cover_path: None,
            accent_rgb: None,
            rating: 4,
            starred: false,
            position: 0.0,
            finished: false,
            last_played: None,
            total_duration: 0.0,
        };
        let rows = [
            row("A", Some("Brandon Sanderson"), Some(2010)),
            row("B", Some("Brandon Sanderson"), None),
        ];
        let commons = book_edit_commons(&rows, vec!["Fantasy".into(), "Fantasy".into()]);
        // Shared values collapse; the people display converts to the dialog's
        // ";"-separated form so a ticked write round-trips split_people.
        assert_eq!(commons["author"], Some("Brandon Sanderson".into()));
        assert_eq!(
            commons["narrator"],
            Some("Kate Reading; Michael Kramer".into())
        );
        assert_eq!(commons["series_index"], Some("1.5".into()));
        assert_eq!(commons["shelfgenre"], Some("Fantasy".into()));
        assert_eq!(commons["rating"], Some("4".into()));
        // Differing titles / years read "multiple values".
        assert_eq!(commons["title"], None);
        assert_eq!(commons["year"], None);
    }

    #[test]
    fn series_index_formats_whole_and_fractional() {
        assert_eq!(fmt_series_index(1.0), "1");
        assert_eq!(fmt_series_index(1.5), "1.5");
    }

    #[test]
    fn path_affecting_matrix() {
        assert!(
            BookEdit {
                title: Some("x".into()),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            BookEdit {
                year: Some(2011),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            BookEdit {
                series: Some(SeriesEdit::Clear),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            BookEdit {
                series_index: Some(2.0),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            BookEdit {
                authors: Some(vec!["A".into()]),
                ..Default::default()
            }
            .is_path_affecting()
        );
        // Not path-affecting (never reach the tree):
        assert!(
            !BookEdit {
                narrators: Some(vec!["N".into()]),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            !BookEdit {
                shelf_genre: Some("SF".into()),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            !BookEdit {
                rating: Some(5),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(
            !BookEdit {
                starred: Some(true),
                ..Default::default()
            }
            .is_path_affecting()
        );
        assert!(BookEdit::default().is_empty());
        assert!(
            !BookEdit {
                rating: Some(5),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn split_people_dedupes_and_trims() {
        assert_eq!(
            split_people("Brandon Sanderson ; Neil Gaiman; brandon sanderson"),
            vec!["Brandon Sanderson".to_string(), "Neil Gaiman".to_string()]
        );
        assert!(split_people("  ;  ").is_empty());
    }

    #[test]
    fn parse_helpers_blank_is_unchanged() {
        assert_eq!(parse_opt_year("   ").unwrap(), None);
        assert_eq!(parse_opt_year("1999").unwrap(), Some(1999));
        assert!(parse_opt_year("nope").is_err());
        assert_eq!(parse_opt_index("").unwrap(), None);
        assert_eq!(parse_opt_index("1.5").unwrap(), Some(1.5));
        assert!(parse_opt_index("x").is_err());
        assert_eq!(parse_opt_rating("").unwrap(), None);
        assert_eq!(parse_opt_rating("5").unwrap(), Some(5));
        assert!(parse_opt_rating("6").is_err());
    }

    #[test]
    fn rendered_folder_overlays_edit() {
        let b = book();
        // No change: reproduce the current shape from the supplied current fields.
        let same = rendered_folder(
            &b,
            Some("The Stormlight Archive"),
            Some("Sanderson, Brandon"),
            &BookEdit::default(),
        );
        assert_eq!(
            same.to_string_lossy(),
            "Audiobooks/Sanderson, Brandon/The Stormlight Archive/01. The Way of Kings (2010)"
        );
        // Clear the series -> Standalone bucket; bump the index is ignored once standalone.
        let standalone = rendered_folder(
            &b,
            Some("The Stormlight Archive"),
            Some("Sanderson, Brandon"),
            &BookEdit {
                series: Some(SeriesEdit::Clear),
                ..Default::default()
            },
        );
        assert!(standalone.to_string_lossy().contains("/Standalone/"));
        // Change the author -> the folder follows the new first author's sort name.
        let reauthored = rendered_folder(
            &b,
            Some("The Stormlight Archive"),
            Some("Sanderson, Brandon"),
            &BookEdit {
                authors: Some(vec!["Neil Gaiman".into()]),
                ..Default::default()
            },
        );
        assert!(
            reauthored
                .to_string_lossy()
                .starts_with("Audiobooks/Gaiman, Neil/")
        );
    }
}
