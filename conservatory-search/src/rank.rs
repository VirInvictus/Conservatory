//! Relevance ranking for bare-text hits (spec §3.4). The blend is Atrium's:
//! FTS5 `bm25` (saturated to [0,1)) plus a recency half-life bonus. The consumer
//! fetches `bm25` from the FTS table and `days_since` from `added_at`.

use crate::ast::Expr;

/// Blend an FTS `bm25` score (lower magnitude = better match in SQLite, so we
/// take `|bm25|`) with a recency bonus. Relevance dominates; recency breaks ties.
pub fn blend_relevance(bm25: f64, days_since: i64, half_life_days: f64) -> f64 {
    let mag = bm25.abs();
    let relevance = mag / (1.0 + mag); // saturates toward 1
    let recency = if half_life_days > 0.0 {
        0.5f64.powf(days_since.max(0) as f64 / half_life_days)
    } else {
        0.0
    };
    relevance + 0.25 * recency
}

/// The bare-text terms in an expression (skips fielded/compare/state nodes), for
/// the FTS query that drives ranking.
pub fn collect_text_terms(expr: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

fn walk(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Text(s) if !s.is_empty() => out.push(s.clone()),
        Expr::Not(inner) => walk(inner, out),
        Expr::And(items) | Expr::Or(items) => items.iter().for_each(|e| walk(e, out)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    #[test]
    fn relevance_saturates_and_recency_decays() {
        // Higher bm25 magnitude => higher relevance.
        assert!(blend_relevance(8.0, 0, 30.0) > blend_relevance(1.0, 0, 30.0));
        // At the half-life, recency bonus halves.
        let fresh = blend_relevance(0.0, 0, 30.0);
        let half = blend_relevance(0.0, 30, 30.0);
        assert!((fresh - 0.25).abs() < 1e-9);
        assert!((half - 0.125).abs() < 1e-9);
    }

    #[test]
    fn collects_only_bare_text() {
        let p = parse("roygbiv boards genre:ambient rating:>=4");
        assert_eq!(collect_text_terms(&p.expr), vec!["roygbiv", "boards"]);
    }
}
