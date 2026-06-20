//! The typed AST and its round-trippable `Display` (spec §3.4).
//!
//! `parse → Display → re-parse` is stable (tested), which is what lets
//! Perspectives be stored as text and re-parsed on load. The shape is ported
//! from `atrium-search`; the field set is Conservatory's music domain.

use std::fmt;

/// A search field. Unknown field names never reach here: the parser degrades
/// them to [`Expr::Text`] (forgiving, spec §3.4). Podcast/audiobook fields land
/// at Phases 6/7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Genre,      // raw multi-value tags (the §5.2 facet side)
    ShelfGenre, // single-valued filed-under (the §5.2 filesystem side)
    Year,
    Added,
    Rating,
    Bitrate,
    Duration,
    Format,
}

impl Field {
    /// Resolve a (lowercased) field token; `None` means "not a known field".
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "artist" => Self::Artist,
            "albumartist" | "album_artist" => Self::AlbumArtist,
            "album" => Self::Album,
            "title" => Self::Title,
            "genre" => Self::Genre,
            "shelfgenre" | "shelf_genre" => Self::ShelfGenre,
            "year" => Self::Year,
            "added" => Self::Added,
            "rating" => Self::Rating,
            "bitrate" => Self::Bitrate,
            "duration" => Self::Duration,
            "format" => Self::Format,
            _ => return None,
        })
    }

    /// The canonical token (what `Display` emits).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::AlbumArtist => "albumartist",
            Self::Album => "album",
            Self::Title => "title",
            Self::Genre => "genre",
            Self::ShelfGenre => "shelfgenre",
            Self::Year => "year",
            Self::Added => "added",
            Self::Rating => "rating",
            Self::Bitrate => "bitrate",
            Self::Duration => "duration",
            Self::Format => "format",
        }
    }

    /// Whether the field is numeric (drives compare/range vs text matching).
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::Year | Self::Rating | Self::Bitrate | Self::Duration
        )
    }

    pub fn is_date(self) -> bool {
        matches!(self, Self::Added)
    }
}

/// How a text field is matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchKind {
    Substring(String),
    Exact(String),
    Regex(String),
    Fuzzy(String),
    /// `field:true` — the field is present / non-empty.
    HasAny,
    /// `field:false` — the field is absent / empty.
    HasNone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparator {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Comparator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A date keyword / precision spec, resolved to an epoch range in `dates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateSpec {
    Today,
    Yesterday,
    ThisWeek,
    ThisMonth,
    ThisYear,
    DaysAgo(u32),
    /// `YYYY`, `YYYY-MM`, or `YYYY-MM-DD` (the `Vec` length is the precision).
    Ymd(i32, Option<u32>, Option<u32>),
}

impl fmt::Display for DateSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Today => write!(f, "today"),
            Self::Yesterday => write!(f, "yesterday"),
            Self::ThisWeek => write!(f, "thisweek"),
            Self::ThisMonth => write!(f, "thismonth"),
            Self::ThisYear => write!(f, "thisyear"),
            Self::DaysAgo(n) => write!(f, "{n}daysago"),
            Self::Ymd(y, None, _) => write!(f, "{y:04}"),
            Self::Ymd(y, Some(m), None) => write!(f, "{y:04}-{m:02}"),
            Self::Ymd(y, Some(m), Some(d)) => write!(f, "{y:04}-{m:02}-{d:02}"),
        }
    }
}

/// A comparison / range value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Real(f64),
    Date(DateSpec),
    Text(String),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::Real(x) => write!(f, "{x}"),
            Self::Date(d) => write!(f, "{d}"),
            Self::Text(s) => write!(f, "{s}"),
        }
    }
}

/// A boolean state predicate (`is:...`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Played,
    Starred,
    Queued,
}

impl State {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "played" => Self::Played,
            "starred" => Self::Starred,
            "queued" => Self::Queued,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Played => "played",
            Self::Starred => "starred",
            Self::Queued => "queued",
        }
    }
}

/// A `sort:` spec, lifted out of the predicate AST into result metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub descending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Title,
    Artist,
    Album,
    Year,
    Added,
    Rating,
    Duration,
}

impl SortKey {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "title" => Self::Title,
            "artist" => Self::Artist,
            "album" => Self::Album,
            "year" => Self::Year,
            "added" => Self::Added,
            "rating" => Self::Rating,
            "duration" => Self::Duration,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Year => "year",
            Self::Added => "added",
            Self::Rating => "rating",
            Self::Duration => "duration",
        }
    }
}

/// The predicate AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Matches nothing changes; the identity for AND, used for empty input and
    /// degraded `vl:` cycles.
    Empty,
    /// Bare free text, matched via FTS (SQL path) or substring (eval fallback).
    Text(String),
    Field {
        field: Field,
        kind: MatchKind,
    },
    Compare {
        field: Field,
        comp: Comparator,
        value: Value,
    },
    Range {
        field: Field,
        low: Value,
        high: Value,
    },
    State(State),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => Ok(()),
            Self::Text(s) => write!(f, "{}", quote_if_needed(s)),
            Self::Field { field, kind } => write_field(f, *field, kind),
            Self::Compare { field, comp, value } => {
                write!(f, "{}:{}{}", field.as_str(), comp.as_str(), value)
            }
            Self::Range { field, low, high } => {
                write!(f, "{}:{}..{}", field.as_str(), low, high)
            }
            Self::State(state) => write!(f, "is:{}", state.as_str()),
            Self::Not(inner) => write!(f, "NOT {}", paren(inner)),
            Self::And(items) => write_joined(f, items, "AND", false),
            Self::Or(items) => write_joined(f, items, "OR", true),
        }
    }
}

fn write_field(f: &mut fmt::Formatter<'_>, field: Field, kind: &MatchKind) -> fmt::Result {
    let name = field.as_str();
    match kind {
        MatchKind::Substring(v) => write!(f, "{name}:{}", quote_if_needed(v)),
        MatchKind::Exact(v) => write!(f, "{name}:={}", quote_if_needed(v)),
        MatchKind::Regex(v) => write!(f, "{name}:~{v}"),
        MatchKind::Fuzzy(v) => write!(f, "{name}:?{}", quote_if_needed(v)),
        MatchKind::HasAny => write!(f, "{name}:true"),
        MatchKind::HasNone => write!(f, "{name}:false"),
    }
}

/// Render an OR's children space-joined, parenthesizing nested ANDs only when an
/// `Or` sits inside (so the round-trip is precedence-stable: `NOT > AND > OR`).
fn write_joined(f: &mut fmt::Formatter<'_>, items: &[Expr], op: &str, is_or: bool) -> fmt::Result {
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            write!(f, " {op} ")?;
        }
        // An AND child that is itself an Or needs parens; an OR child that is an
        // And does not (AND binds tighter). NOT children are handled by `paren`.
        let needs = match item {
            Expr::Or(_) if !is_or => true,
            Expr::And(_) if is_or => false,
            _ => false,
        };
        if needs {
            write!(f, "({item})")?;
        } else {
            write!(f, "{item}")?;
        }
    }
    Ok(())
}

/// Parenthesize a NOT operand unless it is atomic.
fn paren(expr: &Expr) -> String {
    match expr {
        Expr::And(_) | Expr::Or(_) => format!("({expr})"),
        _ => format!("{expr}"),
    }
}

/// Quote a value that contains whitespace or a leading operator so it re-parses.
fn quote_if_needed(s: &str) -> String {
    let needs = s.is_empty()
        || s.chars().any(char::is_whitespace)
        || s.starts_with(['=', '~', '?', '!', '(', ')', '"']);
    if needs {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}
