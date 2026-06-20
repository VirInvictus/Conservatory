//! All-or-nothing SQL translation (spec §3.4, the `atrium-search` dual path).
//!
//! `try_translate` emits a SQL `WHERE` fragment **only if every node maps
//! cleanly**; a `~regex` or `?fuzzy` node makes the whole thing return `None`, so
//! the caller falls back to [`crate::eval`] and the two paths never diverge. The
//! fragment is self-contained against the `tracks` table (columns + `EXISTS`
//! subqueries + an FTS `IN`), so the base query is `SELECT id FROM tracks WHERE …`.
//!
//! Storage-agnostic: parameters are [`SqlValue`] (no rusqlite); placeholders are
//! anonymous `?`, bound positionally in the order they appear (the consumer maps
//! `SqlValue` to its driver).

use chrono::NaiveDate;

use crate::ast::{Comparator, Expr, Field, MatchKind, State, Value};
use crate::dates;

/// A bindable parameter value, carrying no driver types.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Text(String),
    Int(i64),
    Real(f64),
}

/// A translated `WHERE` fragment and its positional parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlClause {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// Translate the whole expression, or `None` if any node can't be expressed in
/// SQL (the caller then evaluates in memory).
pub fn try_translate(expr: &Expr, today: NaiveDate) -> Option<SqlClause> {
    let mut params = Vec::new();
    let sql = node(expr, today, &mut params)?;
    Some(SqlClause { sql, params })
}

fn node(expr: &Expr, today: NaiveDate, p: &mut Vec<SqlValue>) -> Option<String> {
    Some(match expr {
        Expr::Empty => "1=1".to_string(),
        Expr::Text(s) => {
            p.push(SqlValue::Text(fts_phrase(s)));
            "tracks.id IN (SELECT rowid FROM track_fts WHERE track_fts MATCH ?)".to_string()
        }
        Expr::Field { field, kind } => field_sql(*field, kind, p)?,
        Expr::Compare { field, comp, value } => compare_sql(*field, *comp, value, today, p)?,
        Expr::Range { field, low, high } => range_sql(*field, low, high, today, p)?,
        Expr::State(state) => state_sql(*state),
        Expr::Not(inner) => format!("NOT ({})", node(inner, today, p)?),
        Expr::And(items) => join(items, "AND", today, p)?,
        Expr::Or(items) => join(items, "OR", today, p)?,
    })
}

fn join(items: &[Expr], op: &str, today: NaiveDate, p: &mut Vec<SqlValue>) -> Option<String> {
    let parts: Option<Vec<String>> = items.iter().map(|e| node(e, today, p)).collect();
    Some(format!("({})", parts?.join(&format!(" {op} "))))
}

fn field_sql(field: Field, kind: &MatchKind, p: &mut Vec<SqlValue>) -> Option<String> {
    match kind {
        // Regex / fuzzy can't be pushed down: bail so the whole query falls back.
        MatchKind::Regex(_) | MatchKind::Fuzzy(_) => None,
        MatchKind::Substring(v) => Some(text_cond(field, &like(v, false), p)),
        MatchKind::Exact(v) => Some(text_cond(field, &like(v, true), p)),
        MatchKind::HasAny => Some(presence_sql(field, true)),
        MatchKind::HasNone => Some(presence_sql(field, false)),
    }
}

/// A case-insensitive `LIKE` condition for a text field; pushes the pattern.
fn text_cond(field: Field, pattern: &str, p: &mut Vec<SqlValue>) -> String {
    p.push(SqlValue::Text(pattern.to_string()));
    match field {
        Field::Title => "tracks.title LIKE ? ESCAPE '\\'".into(),
        Field::Format => "tracks.format LIKE ? ESCAPE '\\'".into(),
        Field::Album => exists_album("a.title LIKE ? ESCAPE '\\'"),
        Field::ShelfGenre => exists_album("a.shelf_genre LIKE ? ESCAPE '\\'"),
        Field::AlbumArtist => {
            "EXISTS (SELECT 1 FROM albums a JOIN artists ar ON ar.id = a.album_artist_id \
             WHERE a.id = tracks.album_id AND ar.name LIKE ? ESCAPE '\\')"
                .into()
        }
        Field::Artist => "EXISTS (SELECT 1 FROM artists ar WHERE ar.id = tracks.artist_id \
             AND ar.name LIKE ? ESCAPE '\\')"
            .into(),
        Field::Genre => {
            "EXISTS (SELECT 1 FROM track_genres tg JOIN genres g ON g.id = tg.genre_id \
             WHERE tg.track_id = tracks.id AND g.name LIKE ? ESCAPE '\\')"
                .into()
        }
        // Numeric/date fields never reach text_cond.
        _ => "0=1".into(),
    }
}

fn exists_album(inner: &str) -> String {
    format!("EXISTS (SELECT 1 FROM albums a WHERE a.id = tracks.album_id AND {inner})")
}

fn presence_sql(field: Field, want: bool) -> String {
    let has = match field {
        Field::Title => "(tracks.title IS NOT NULL AND tracks.title != '')".to_string(),
        Field::Format => "(tracks.format IS NOT NULL AND tracks.format != '')".to_string(),
        Field::Rating => "tracks.rating > 0".to_string(),
        Field::Bitrate => "tracks.bitrate IS NOT NULL".to_string(),
        Field::Duration => "tracks.duration IS NOT NULL".to_string(),
        Field::Added => "tracks.added_at IS NOT NULL".to_string(),
        Field::Artist => {
            "EXISTS (SELECT 1 FROM artists ar WHERE ar.id = tracks.artist_id)".to_string()
        }
        Field::Album => exists_album("1=1"),
        Field::Year => exists_album("a.year IS NOT NULL"),
        Field::ShelfGenre => exists_album("a.shelf_genre IS NOT NULL AND a.shelf_genre != ''"),
        Field::AlbumArtist => exists_album("a.album_artist_id IS NOT NULL"),
        Field::Genre => {
            "EXISTS (SELECT 1 FROM track_genres tg WHERE tg.track_id = tracks.id)".to_string()
        }
    };
    if want { has } else { format!("NOT ({has})") }
}

fn compare_sql(
    field: Field,
    comp: Comparator,
    value: &Value,
    today: NaiveDate,
    p: &mut Vec<SqlValue>,
) -> Option<String> {
    if field.is_date() {
        let Value::Date(spec) = value else {
            return None;
        };
        let (start, end) = dates::resolve_range(spec, today);
        return Some(date_cond("tracks.added_at", comp, start, end, p));
    }
    let rhs = value_sql(value)?;
    let op = comp.as_str();
    Some(match field {
        Field::Rating => num_cond("tracks.rating", op, rhs, p),
        Field::Bitrate => num_cond("tracks.bitrate", op, rhs, p),
        Field::Duration => num_cond("tracks.duration", op, rhs, p),
        Field::Year => {
            p.push(rhs);
            exists_album(&format!("a.year {op} ?"))
        }
        _ => return None,
    })
}

fn num_cond(col: &str, op: &str, rhs: SqlValue, p: &mut Vec<SqlValue>) -> String {
    p.push(rhs);
    format!("{col} {op} ?")
}

/// Mirror `dates::matches` exactly so the SQL and eval paths agree.
fn date_cond(col: &str, comp: Comparator, start: i64, end: i64, p: &mut Vec<SqlValue>) -> String {
    match comp {
        Comparator::Eq => {
            p.push(SqlValue::Int(start));
            p.push(SqlValue::Int(end));
            format!("({col} >= ? AND {col} < ?)")
        }
        Comparator::Ne => {
            p.push(SqlValue::Int(start));
            p.push(SqlValue::Int(end));
            format!("({col} < ? OR {col} >= ?)")
        }
        Comparator::Lt => {
            p.push(SqlValue::Int(start));
            format!("{col} < ?")
        }
        Comparator::Le => {
            p.push(SqlValue::Int(end));
            format!("{col} < ?")
        }
        Comparator::Gt => {
            p.push(SqlValue::Int(end));
            format!("{col} >= ?")
        }
        Comparator::Ge => {
            p.push(SqlValue::Int(start));
            format!("{col} >= ?")
        }
    }
}

fn range_sql(
    field: Field,
    low: &Value,
    high: &Value,
    today: NaiveDate,
    p: &mut Vec<SqlValue>,
) -> Option<String> {
    if field.is_date() {
        let (Value::Date(lo), Value::Date(hi)) = (low, high) else {
            return None;
        };
        let (start, _) = dates::resolve_range(lo, today);
        let (_, end) = dates::resolve_range(hi, today);
        p.push(SqlValue::Int(start));
        p.push(SqlValue::Int(end));
        return Some("(tracks.added_at >= ? AND tracks.added_at < ?)".into());
    }
    let (lo, hi) = (value_sql(low)?, value_sql(high)?);
    Some(match field {
        Field::Rating => between("tracks.rating", lo, hi, p),
        Field::Bitrate => between("tracks.bitrate", lo, hi, p),
        Field::Duration => between("tracks.duration", lo, hi, p),
        Field::Year => {
            p.push(lo);
            p.push(hi);
            exists_album("a.year >= ? AND a.year <= ?")
        }
        _ => return None,
    })
}

fn between(col: &str, lo: SqlValue, hi: SqlValue, p: &mut Vec<SqlValue>) -> String {
    p.push(lo);
    p.push(hi);
    format!("({col} >= ? AND {col} <= ?)")
}

fn state_sql(state: State) -> String {
    match state {
        State::Played => "tracks.play_count > 0".into(),
        State::Starred => "tracks.starred = 1".into(),
        // The queue table lands with playback (Phase 4b); until then nothing is
        // queued. `eval` mirrors this (SearchItem.queued is false).
        State::Queued => "0=1".into(),
    }
}

fn value_sql(value: &Value) -> Option<SqlValue> {
    match value {
        Value::Int(n) => Some(SqlValue::Int(*n)),
        Value::Real(x) => Some(SqlValue::Real(*x)),
        _ => None,
    }
}

/// Escape `%` `_` `\` so a substring/exact value is matched literally by LIKE.
fn like(v: &str, exact: bool) -> String {
    let escaped = v
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    if exact {
        escaped
    } else {
        format!("%{escaped}%")
    }
}

/// Wrap a bare-text term as an FTS phrase (double embedded quotes), so FTS5 sees
/// a literal phrase rather than operator syntax.
fn fts_phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 20).unwrap()
    }

    fn tr(expr: &str) -> Option<SqlClause> {
        try_translate(&parse(expr).expr, today())
    }

    #[test]
    fn translatable_nodes_emit_sql_and_params() {
        let c = tr("genre:ambient AND rating:>=4").unwrap();
        assert!(c.sql.contains("track_genres"));
        assert!(c.sql.contains("tracks.rating >= ?"));
        assert_eq!(
            c.params,
            vec![SqlValue::Text("%ambient%".into()), SqlValue::Int(4)]
        );
    }

    #[test]
    fn bare_text_uses_fts() {
        let c = tr("roygbiv").unwrap();
        assert!(c.sql.contains("track_fts MATCH ?"));
        assert_eq!(c.params, vec![SqlValue::Text("\"roygbiv\"".into())]);
    }

    #[test]
    fn regex_and_fuzzy_do_not_translate() {
        assert!(tr("title:~rx").is_none());
        assert!(tr("artist:?boards").is_none());
        // A regex anywhere poisons the whole translation (all-or-nothing).
        assert!(tr("genre:ambient AND title:~rx").is_none());
    }

    #[test]
    fn year_and_range() {
        let c = tr("year:1990..2000").unwrap();
        assert!(c.sql.contains("a.year >= ? AND a.year <= ?"));
        assert_eq!(c.params, vec![SqlValue::Int(1990), SqlValue::Int(2000)]);
    }

    #[test]
    fn state_and_presence() {
        assert!(tr("is:starred").unwrap().sql.contains("tracks.starred = 1"));
        assert!(
            tr("rating:false")
                .unwrap()
                .sql
                .contains("NOT (tracks.rating > 0)")
        );
    }
}
