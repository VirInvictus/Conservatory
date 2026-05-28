//! Conservatory search-expression language.
//!
//! The Calibre-shaped grammar (lex / parse / AST / evaluator / SQL translator)
//! typed against Conservatory's domain (Track / Album / Artist / Show /
//! Episode). The grammar *shape* is ported from `atrium-search`; the
//! implementation is independent so the projects evolve without coupling (the
//! Belfry precedent, recorded in ATTRIBUTIONS.md). See spec §3.4.
//!
//! Phase 0 skeleton: no implementation yet. The grammar lands at Phase 3
//! alongside the GTK browse surface.
