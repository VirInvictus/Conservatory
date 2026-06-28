//! Audiobook shelf filtering (Phase 7b-ii, spec §3.4 / §3.8). The same grammar
//! as the music filter bar, applied to the in-memory book shelf.
//!
//! Unlike the music `query.rs`, this is **eval-only**: the shelf is small and is
//! already loaded whole (`list_book_rows` + `sort_shelf`), and the audiobook
//! fields have no column on the music `tracks` table, so `sql_translate` bails on
//! them anyway. So a parsed expression is evaluated in memory over each book's
//! projection. `vl:NAME` still expands from the saved Perspectives at parse time.

use chrono::NaiveDate;

use conservatory_core::db::BookListRow;
use conservatory_search::{PerspectiveResolver, SearchItem, evaluate, parse_with_resolver};

/// Filter `rows` by the filter-bar `query`, preserving the shelf order (the
/// grammar only removes books; `sort_shelf` already ordered them). Returns the
/// forgiving parser's warnings, non-empty when the input degraded (the UI tints
/// the bar). An empty query is the whole shelf.
pub fn filter_books(
    rows: &[BookListRow],
    query: &str,
    resolver: &dyn PerspectiveResolver,
    today: NaiveDate,
) -> (Vec<BookListRow>, Vec<String>) {
    if query.trim().is_empty() {
        return (rows.to_vec(), Vec::new());
    }
    let parsed = parse_with_resolver(query, resolver);
    let kept = rows
        .iter()
        .filter(|r| evaluate(&parsed.expr, &book_item(r), today))
        .cloned()
        .collect();
    (kept, parsed.warnings)
}

/// Project a shelf row into the grammar's [`SearchItem`]. The denormalized
/// author / narrator display strings are comma-joined in the read, so they split
/// back into the multi-valued candidates the grammar matches per-name. Books
/// carry no `artist`/`album`/`genre`/`added`, so those stay empty.
fn book_item(r: &BookListRow) -> SearchItem {
    SearchItem {
        title: r.title.clone(),
        authors: split_people(r.author_display.as_deref()),
        narrators: split_people(r.narrator_display.as_deref()),
        series: r.series_name.clone(),
        year: r.year,
        rating: r.rating,
        starred: r.starred,
        finished: r.finished,
        // The shelf shows total runtime, so `duration:` (seconds) works on books.
        duration: (r.total_duration > 0.0).then_some(r.total_duration),
        ..SearchItem::default()
    }
}

/// Split a `", "`-joined people display string into its names (empty when absent).
fn split_people(display: Option<&str>) -> Vec<String> {
    display
        .map(|s| {
            s.split(", ")
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A resolver with no saved Perspectives (every `vl:` is unknown).
    struct NoPerspectives;
    impl PerspectiveResolver for NoPerspectives {
        fn expression(&self, _name: &str) -> Option<String> {
            None
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 28).unwrap()
    }

    fn book(
        id: i64,
        title: &str,
        author: &str,
        series: Option<&str>,
        finished: bool,
    ) -> BookListRow {
        BookListRow {
            id,
            title: title.into(),
            subtitle: None,
            author_display: Some(author.into()),
            narrator_display: Some("Kate Reading, Michael Kramer".into()),
            series_name: series.map(str::to_string),
            series_sequence: series.map(|_| 1.0),
            year: Some(2010),
            cover_path: None,
            accent_rgb: None,
            rating: 4,
            starred: id == 1,
            position: if finished { 0.0 } else { 0.5 },
            finished,
            last_played: None,
            total_duration: 3600.0,
        }
    }

    fn shelf() -> Vec<BookListRow> {
        vec![
            book(
                1,
                "The Way of Kings",
                "Brandon Sanderson",
                Some("The Stormlight Archive"),
                true,
            ),
            book(
                2,
                "Words of Radiance",
                "Brandon Sanderson",
                Some("The Stormlight Archive"),
                false,
            ),
            book(3, "The Hobbit", "J.R.R. Tolkien", None, false),
        ]
    }

    fn run(query: &str) -> Vec<i64> {
        let (kept, _) = filter_books(&shelf(), query, &NoPerspectives, today());
        kept.iter().map(|b| b.id).collect()
    }

    #[test]
    fn empty_query_is_the_whole_shelf() {
        let (kept, warns) = filter_books(&shelf(), "  ", &NoPerspectives, today());
        assert_eq!(kept.len(), 3);
        assert!(warns.is_empty());
    }

    #[test]
    fn fielded_audiobook_matches() {
        assert_eq!(run("author:sanderson"), vec![1, 2]);
        assert_eq!(run("author:tolkien"), vec![3]);
        assert_eq!(run("series:stormlight"), vec![1, 2]);
        // A second narrator still matches (multi-valued split).
        assert_eq!(run("narrator:kramer"), vec![1, 2, 3]);
        // Order is the shelf order, not the match order.
        assert_eq!(run("series:stormlight OR author:tolkien"), vec![1, 2, 3]);
    }

    #[test]
    fn is_finished_and_composition() {
        assert_eq!(run("is:finished"), vec![1]);
        assert_eq!(run("NOT is:finished"), vec![2, 3]);
        assert_eq!(run("author:sanderson AND NOT is:finished"), vec![2]);
        // is:starred and the shared numeric grammar work on the shelf too.
        assert_eq!(run("is:starred"), vec![1]);
        assert_eq!(run("rating:>=4 AND year:2010"), vec![1, 2, 3]);
    }

    #[test]
    fn bare_text_scans_title_and_author() {
        assert_eq!(run("kings"), vec![1]); // title
        assert_eq!(run("tolkien"), vec![3]); // author
    }

    #[test]
    fn forgiving_degrade_warns_never_errors() {
        // Unknown field degrades to substring text and warns.
        let (_, warns) = filter_books(&shelf(), "bogus:x", &NoPerspectives, today());
        assert!(!warns.is_empty());
        // Unbalanced parens degrade the whole input, still no panic.
        let (_, warns) = filter_books(&shelf(), "(author:sanderson", &NoPerspectives, today());
        assert!(!warns.is_empty());
    }
}
