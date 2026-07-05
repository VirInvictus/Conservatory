//! Conservatory search-expression language (spec §3.4, docs/search-grammar.md).
//!
//! A Calibre-shaped grammar typed against Conservatory's domain. The pipeline
//! mirrors `atrium-search`: `lex` → `parse` (typed AST + extracted `sort:` specs,
//! forgiving) → two consumers, `eval` (in-memory bool) and `sql_translate` (an
//! all-or-nothing SQL `WHERE`, so the paths never diverge), with `rank` for
//! bare-text relevance. Semantics are modeled on CalibreQuarry; the
//! implementation is independent (the Belfry precedent, ATTRIBUTIONS.md).
//!
//! The crate is storage-agnostic (no rusqlite, no conservatory-core): the
//! consumer maps [`SqlValue`] to its driver and fills [`SearchItem`] from a read.

pub mod ast;
pub mod dates;
pub mod eval;
pub mod fold;
pub mod lex;
pub mod parse;
pub mod rank;
pub mod sql_translate;

pub use ast::{Comparator, DateSpec, Expr, Field, MatchKind, SortKey, SortSpec, State, Value};
pub use eval::{SearchItem, evaluate};
pub use fold::fold;
pub use parse::{ParseResult, PerspectiveResolver, parse, parse_with_resolver};
pub use rank::{blend_relevance, collect_text_terms};
pub use sql_translate::{SqlClause, SqlValue, try_translate};
