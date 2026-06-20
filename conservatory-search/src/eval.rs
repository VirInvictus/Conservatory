//! In-memory evaluator (the fallback path, spec §3.4). Runs when `try_translate`
//! can't push the whole expression to SQL (a `~regex` or `?fuzzy` node). Matching
//! is datatype-dispatched per CalibreQuarry; per-item like `atrium-search`.

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::NaiveDate;
use regex::Regex;

use crate::ast::{Comparator, Expr, Field, MatchKind, State, Value};
use crate::dates;

/// The searchable projection of a track. The consumer fills this from a DB read;
/// the crate owns it so it stays storage-agnostic and fuzzable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchItem {
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub shelf_genre: Option<String>,
    pub genres: Vec<String>,
    pub year: Option<i32>,
    pub added: Option<i64>, // epoch seconds
    pub rating: u8,
    pub bitrate: Option<i32>,
    pub duration: Option<f64>,
    pub format: Option<String>,
    pub played: bool,
    pub starred: bool,
    pub queued: bool,
}

/// Evaluate `expr` against `item`. `today` resolves relative date keywords (the
/// caller passes a stable date so eval and `sql_translate` agree).
pub fn evaluate(expr: &Expr, item: &SearchItem, today: NaiveDate) -> bool {
    eval(expr, item, today)
}

fn eval(expr: &Expr, item: &SearchItem, today: NaiveDate) -> bool {
    match expr {
        Expr::Empty => true,
        Expr::Text(s) => text_any(item, s),
        Expr::Field { field, kind } => field_match(item, *field, kind),
        Expr::Compare { field, comp, value } => compare(item, *field, *comp, value, today),
        Expr::Range { field, low, high } => range(item, *field, low, high, today),
        Expr::State(state) => state_match(item, *state),
        Expr::Not(inner) => !eval(inner, item, today),
        Expr::And(items) => items.iter().all(|e| eval(e, item, today)),
        Expr::Or(items) => items.iter().any(|e| eval(e, item, today)),
    }
}

/// Bare text: case-insensitive substring across the FTS-backed columns (the
/// offline approximation of the FTS path; bare text normally takes the SQL path).
fn text_any(item: &SearchItem, needle: &str) -> bool {
    let n = needle.to_lowercase();
    let cols = [
        Some(item.title.as_str()),
        item.artist.as_deref(),
        item.album.as_deref(),
    ];
    cols.iter().flatten().any(|c| c.to_lowercase().contains(&n))
}

/// The text candidates a field exposes (multi-valued for genre).
fn candidates(item: &SearchItem, field: Field) -> Vec<&str> {
    match field {
        Field::Title => vec![item.title.as_str()],
        Field::Artist => item.artist.as_deref().into_iter().collect(),
        Field::AlbumArtist => item.album_artist.as_deref().into_iter().collect(),
        Field::Album => item.album.as_deref().into_iter().collect(),
        Field::ShelfGenre => item.shelf_genre.as_deref().into_iter().collect(),
        Field::Format => item.format.as_deref().into_iter().collect(),
        Field::Genre => item.genres.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Whether a field has a value (for `:true`/`:false` presence).
fn present(item: &SearchItem, field: Field) -> bool {
    match field {
        Field::Year => item.year.is_some(),
        Field::Rating => item.rating > 0,
        Field::Bitrate => item.bitrate.is_some(),
        Field::Duration => item.duration.is_some(),
        Field::Added => item.added.is_some(),
        _ => candidates(item, field).iter().any(|c| !c.is_empty()),
    }
}

fn field_match(item: &SearchItem, field: Field, kind: &MatchKind) -> bool {
    match kind {
        MatchKind::HasAny => present(item, field),
        MatchKind::HasNone => !present(item, field),
        MatchKind::Substring(v) => {
            let v = v.to_lowercase();
            candidates(item, field)
                .iter()
                .any(|c| c.to_lowercase().contains(&v))
        }
        MatchKind::Exact(v) => candidates(item, field)
            .iter()
            .any(|c| c.eq_ignore_ascii_case(v)),
        MatchKind::Regex(v) => with_regex(v, |re| {
            candidates(item, field).iter().any(|c| re.is_match(c))
        }),
        MatchKind::Fuzzy(v) => {
            let threshold = fuzzy_threshold(v);
            candidates(item, field)
                .iter()
                .any(|c| fuzzy_hit(c, v, threshold))
        }
    }
}

fn compare(
    item: &SearchItem,
    field: Field,
    comp: Comparator,
    value: &Value,
    today: NaiveDate,
) -> bool {
    if field.is_date() {
        let Value::Date(spec) = value else {
            return false;
        };
        let Some(added) = item.added else {
            return false;
        };
        let (start, end) = dates::resolve_range(spec, today);
        return dates::matches(comp, added, start, end);
    }
    let Some(lhs) = numeric(item, field) else {
        return false;
    };
    let Some(rhs) = value_as_f64(value) else {
        return false;
    };
    apply_comp(comp, lhs, rhs)
}

fn range(item: &SearchItem, field: Field, low: &Value, high: &Value, today: NaiveDate) -> bool {
    if field.is_date() {
        let (Value::Date(lo), Value::Date(hi)) = (low, high) else {
            return false;
        };
        let Some(added) = item.added else {
            return false;
        };
        let (start, _) = dates::resolve_range(lo, today);
        let (_, end) = dates::resolve_range(hi, today);
        return added >= start && added < end;
    }
    let (Some(lhs), Some(lo), Some(hi)) =
        (numeric(item, field), value_as_f64(low), value_as_f64(high))
    else {
        return false;
    };
    lhs >= lo && lhs <= hi
}

fn numeric(item: &SearchItem, field: Field) -> Option<f64> {
    match field {
        Field::Year => item.year.map(|y| y as f64),
        Field::Rating => Some(item.rating as f64),
        Field::Bitrate => item.bitrate.map(|b| b as f64),
        Field::Duration => item.duration,
        _ => None,
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::Real(x) => Some(*x),
        _ => None,
    }
}

fn apply_comp(comp: Comparator, lhs: f64, rhs: f64) -> bool {
    match comp {
        Comparator::Eq => lhs == rhs,
        Comparator::Ne => lhs != rhs,
        Comparator::Lt => lhs < rhs,
        Comparator::Le => lhs <= rhs,
        Comparator::Gt => lhs > rhs,
        Comparator::Ge => lhs >= rhs,
    }
}

fn state_match(item: &SearchItem, state: State) -> bool {
    match state {
        State::Played => item.played,
        State::Starred => item.starred,
        State::Queued => item.queued,
    }
}

/// Compile (and cache) a regex; a bad pattern matches nothing.
fn with_regex(pattern: &str, f: impl Fn(&Regex) -> bool) -> bool {
    thread_local! {
        static CACHE: RefCell<HashMap<String, Option<Regex>>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let entry = cache
            .entry(pattern.to_string())
            .or_insert_with(|| Regex::new(&format!("(?i){pattern}")).ok());
        entry.as_ref().map(&f).unwrap_or(false)
    })
}

fn fuzzy_threshold(needle: &str) -> usize {
    match needle.chars().count() {
        0..=4 => 1,
        5..=7 => 2,
        _ => 3,
    }
}

/// A fuzzy hit if the needle is within `threshold` edits of the whole candidate
/// or any of its whitespace-separated words.
fn fuzzy_hit(candidate: &str, needle: &str, threshold: usize) -> bool {
    let cand = candidate.to_lowercase();
    let need = needle.to_lowercase();
    if damerau_levenshtein(&cand, &need) <= threshold {
        return true;
    }
    cand.split_whitespace()
        .any(|word| damerau_levenshtein(word, &need) <= threshold)
}

/// Optimal string alignment (Damerau-Levenshtein with adjacent transpositions).
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev2 = vec![0usize; m + 1];
    let mut prev = (0..=m).collect::<Vec<_>>();
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                curr[j] = curr[j].min(prev2[j - 2] + 1);
            }
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::DateSpec;

    fn item() -> SearchItem {
        SearchItem {
            title: "Roygbiv".into(),
            artist: Some("Boards of Canada".into()),
            album: Some("Music Has the Right to Children".into()),
            shelf_genre: Some("Electronic".into()),
            genres: vec!["Electronic".into(), "Ambient".into()],
            year: Some(1998),
            added: Some(1_000_000_000),
            rating: 5,
            bitrate: Some(1000),
            duration: Some(151.0),
            format: Some("flac".into()),
            played: true,
            starred: false,
            queued: false,
            album_artist: Some("Boards of Canada".into()),
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 20).unwrap()
    }

    fn run(expr: &str) -> bool {
        evaluate(&crate::parse::parse(expr).expr, &item(), today())
    }

    #[test]
    fn substring_and_exact() {
        assert!(run("artist:boards"));
        assert!(run("artist:=\"Boards of Canada\""));
        assert!(!run("artist:=boards"));
    }

    #[test]
    fn multi_value_genre() {
        assert!(run("genre:ambient"));
        assert!(run("genre:electronic"));
        assert!(!run("genre:jazz"));
    }

    #[test]
    fn numeric_relops_and_range() {
        assert!(run("rating:>=4"));
        assert!(run("year:1990..2000"));
        assert!(!run("year:2001"));
        assert!(run("bitrate:>900"));
    }

    #[test]
    fn presence_and_state() {
        assert!(run("rating:true"));
        assert!(run("is:played"));
        assert!(!run("is:starred"));
        assert!(run("format:true"));
    }

    #[test]
    fn regex_and_fuzzy() {
        assert!(run("title:~^roy"));
        assert!(run("artist:?canadaa")); // one transposition/insert off
        assert!(!run("artist:?zzzzz"));
    }

    #[test]
    fn boolean_composition() {
        assert!(run("genre:ambient AND rating:>=4"));
        assert!(run("genre:jazz OR genre:ambient"));
        assert!(run("NOT genre:jazz"));
        assert!(!run("genre:ambient AND is:starred"));
    }

    #[test]
    fn empty_matches_everything() {
        assert!(evaluate(&Expr::Empty, &item(), today()));
    }

    #[test]
    fn date_keyword() {
        let mut it = item();
        let (start, _) = dates::resolve_range(&DateSpec::Today, today());
        it.added = Some(start + 100);
        assert!(evaluate(
            &crate::parse::parse("added:today").expr,
            &it,
            today()
        ));
        assert!(!evaluate(
            &crate::parse::parse("added:yesterday").expr,
            &it,
            today()
        ));
    }
}
