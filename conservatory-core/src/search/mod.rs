//! The search integration: [`domain`] maps Conservatory's schema onto the
//! shared `vir_search` grammar, and the two evaluation paths live here
//! ([`sql_translate`]'s SQL push-down with [`eval`] as the in-memory
//! fallback, per spec §3.4).

pub mod domain;
pub mod eval;
pub mod sql_translate;

pub use domain::{Field, SortKey, State};
pub use eval::{SearchItem, evaluate};
pub use sql_translate::{SqlClause, SqlValue, try_translate};
