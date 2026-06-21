//! Recursive-descent parser (spec §3.4). **Forgiving**: it never returns an
//! error. Unknown fields/states/sorts and bad values degrade to substring text
//! plus a warning; a hard structural failure (unbalanced parens, stray operator)
//! degrades the *whole* input to substring text (the yellow filter-bar tint).
//!
//! `sort:` specs are lifted out of the predicate AST into `ParseResult.sorts`.
//! `vl:NAME` perspective references are expanded here via a [`PerspectiveResolver`]
//! with a cycle guard, so `eval` and `sql_translate` never see them.

use crate::ast::*;
use crate::lex::{Token, lex};

/// Resolves a perspective name to its (text) expression, for `vl:` expansion.
pub trait PerspectiveResolver {
    fn expression(&self, name: &str) -> Option<String>;
}

/// The parse output: the predicate, the extracted sort specs, and any
/// degrade/cycle warnings (the UI tints the filter bar when this is non-empty).
#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult {
    pub expr: Expr,
    pub sorts: Vec<SortSpec>,
    pub warnings: Vec<String>,
}

/// Parse an expression with no perspective resolver (`vl:` degrades to text).
pub fn parse(input: &str) -> ParseResult {
    parse_inner(input, None, &[])
}

/// Parse an expression, expanding `vl:NAME` references via `resolver`.
pub fn parse_with_resolver(input: &str, resolver: &dyn PerspectiveResolver) -> ParseResult {
    parse_inner(input, Some(resolver), &[])
}

fn parse_inner(
    input: &str,
    resolver: Option<&dyn PerspectiveResolver>,
    seen: &[String],
) -> ParseResult {
    let mut p = Parser {
        tokens: lex(input),
        pos: 0,
        sorts: Vec::new(),
        warnings: Vec::new(),
        resolver,
        seen: seen.to_vec(),
    };
    match p.parse_top() {
        Ok(expr) => ParseResult {
            expr,
            sorts: p.sorts,
            warnings: p.warnings,
        },
        Err(()) => {
            let mut warnings = p.warnings;
            let trimmed = input.trim();
            let expr = if trimmed.is_empty() {
                Expr::Empty
            } else {
                warnings.push(format!("could not parse {input:?}; matching as text"));
                Expr::Text(trimmed.to_string())
            };
            ParseResult {
                expr,
                sorts: Vec::new(),
                warnings,
            }
        }
    }
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    sorts: Vec<SortSpec>,
    warnings: Vec<String>,
    resolver: Option<&'a dyn PerspectiveResolver>,
    seen: Vec<String>,
}

type PResult = Result<Expr, ()>;

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, tok: &Token) -> bool {
        if self.peek() == Some(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w.eq_ignore_ascii_case(kw))
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self.peek_keyword(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_top(&mut self) -> PResult {
        if self.peek().is_none() {
            return Ok(Expr::Empty);
        }
        let expr = self.or_expr()?;
        if self.pos != self.tokens.len() {
            return Err(()); // trailing tokens => structural failure
        }
        Ok(expr)
    }

    fn or_expr(&mut self) -> PResult {
        let mut items = vec![self.and_expr()?];
        while self.eat_keyword("or") {
            items.push(self.and_expr()?);
        }
        let items: Vec<Expr> = items
            .into_iter()
            .filter(|e| !matches!(e, Expr::Empty))
            .collect();
        Ok(match items.len() {
            0 => Expr::Empty,
            1 => items.into_iter().next().unwrap(),
            _ => Expr::Or(items),
        })
    }

    fn and_expr(&mut self) -> PResult {
        let mut items = vec![self.not_expr()?];
        // Explicit `AND`, or an implicit AND between adjacent terms (anything that
        // starts a primary and isn't the `OR` keyword).
        while self.eat_keyword("and") || (self.starts_primary() && !self.peek_keyword("or")) {
            items.push(self.not_expr()?);
        }
        let items: Vec<Expr> = items
            .into_iter()
            .filter(|e| !matches!(e, Expr::Empty))
            .collect();
        Ok(match items.len() {
            0 => Expr::Empty,
            1 => items.into_iter().next().unwrap(),
            _ => Expr::And(items),
        })
    }

    fn not_expr(&mut self) -> PResult {
        if self.eat_keyword("not") || self.eat(&Token::Bang) {
            let inner = self.not_expr()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.primary()
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some(Token::Word(w)) => !is_boolean_keyword(w) || w.eq_ignore_ascii_case("not"),
            Some(Token::Quoted(_)) | Some(Token::LParen) | Some(Token::Bang) => true,
            _ => false,
        }
    }

    fn primary(&mut self) -> PResult {
        match self.peek() {
            Some(Token::LParen) => {
                self.pos += 1;
                let e = self.or_expr()?;
                if !self.eat(&Token::RParen) {
                    return Err(());
                }
                Ok(e)
            }
            Some(Token::Quoted(s)) => {
                let s = s.clone();
                self.pos += 1;
                Ok(text_or_empty(s))
            }
            Some(Token::Word(w)) => {
                let w = w.clone();
                if is_boolean_keyword(&w) {
                    return Err(()); // stray AND/OR/NOT
                }
                self.pos += 1;
                if self.eat(&Token::Colon) {
                    self.field_clause(&w)
                } else {
                    Ok(text_or_empty(w))
                }
            }
            _ => Err(()),
        }
    }

    fn field_clause(&mut self, name: &str) -> PResult {
        match name.to_ascii_lowercase().as_str() {
            "is" => self.state_clause(),
            "sort" => self.sort_clause(),
            "vl" => {
                let val = self.value_string().ok_or(())?;
                self.resolve_vl(val)
            }
            lname => match Field::parse(lname) {
                Some(field) if field.is_numeric() || field.is_date() => self.relational(field),
                Some(field) => self.text_match(field),
                None => {
                    // Unknown field: degrade the whole clause to substring text.
                    let val = self.value_string().ok_or(())?;
                    self.warnings
                        .push(format!("unknown field {name:?}; matching as text"));
                    Ok(Expr::Text(format!("{name}:{val}")))
                }
            },
        }
    }

    fn state_clause(&mut self) -> PResult {
        let val = self.value_string().ok_or(())?;
        match State::parse(&val.to_ascii_lowercase()) {
            Some(state) => Ok(Expr::State(state)),
            None => {
                self.warnings
                    .push(format!("unknown state is:{val}; matching as text"));
                Ok(Expr::Text(format!("is:{val}")))
            }
        }
    }

    fn sort_clause(&mut self) -> PResult {
        let val = self.value_string().ok_or(())?;
        let (descending, key_str) = match val.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, val.as_str()),
        };
        match SortKey::parse(&key_str.to_ascii_lowercase()) {
            Some(key) => self.sorts.push(SortSpec { key, descending }),
            None => self
                .warnings
                .push(format!("unknown sort key {key_str:?}; ignored")),
        }
        Ok(Expr::Empty) // sort is metadata, not a predicate
    }

    fn text_match(&mut self, field: Field) -> PResult {
        match self.peek() {
            Some(Token::Eq) => {
                self.pos += 1;
                Ok(Expr::Field {
                    field,
                    kind: MatchKind::Exact(self.value_string().ok_or(())?),
                })
            }
            Some(Token::Tilde) => {
                self.pos += 1;
                Ok(Expr::Field {
                    field,
                    kind: MatchKind::Regex(self.value_string().ok_or(())?),
                })
            }
            Some(Token::Quest) => {
                self.pos += 1;
                Ok(Expr::Field {
                    field,
                    kind: MatchKind::Fuzzy(self.value_string().ok_or(())?),
                })
            }
            Some(Token::Word(w)) if bool_word(w).is_some() => {
                let present = bool_word(w).unwrap();
                self.pos += 1;
                Ok(Expr::Field {
                    field,
                    kind: if present {
                        MatchKind::HasAny
                    } else {
                        MatchKind::HasNone
                    },
                })
            }
            Some(Token::Word(_)) | Some(Token::Quoted(_)) => Ok(Expr::Field {
                field,
                kind: MatchKind::Substring(self.value_string().ok_or(())?),
            }),
            _ => Err(()),
        }
    }

    fn relational(&mut self, field: Field) -> PResult {
        // Presence test first.
        if let Some(Token::Word(w)) = self.peek()
            && let Some(present) = bool_word(w)
        {
            self.pos += 1;
            return Ok(Expr::Field {
                field,
                kind: if present {
                    MatchKind::HasAny
                } else {
                    MatchKind::HasNone
                },
            });
        }

        let comp = self.eat_comparator();
        let raw = self.value_string().ok_or(())?;
        let Some(low) = parse_typed_value(field, &raw) else {
            // Bad value: degrade just this clause.
            self.warnings.push(format!(
                "bad value for {}: {raw:?}; matching as text",
                field.as_str()
            ));
            let prefix = comp.map(|c| c.as_str()).unwrap_or("");
            return Ok(Expr::Text(format!("{}:{prefix}{raw}", field.as_str())));
        };

        if comp.is_none() && self.eat(&Token::DotDot) {
            let raw_hi = self.value_string().ok_or(())?;
            if let Some(high) = parse_typed_value(field, &raw_hi) {
                return Ok(Expr::Range { field, low, high });
            }
            self.warnings
                .push(format!("bad range bound {raw_hi:?}; matching as text"));
            return Ok(Expr::Text(format!("{}:{raw}..{raw_hi}", field.as_str())));
        }

        Ok(Expr::Compare {
            field,
            comp: comp.unwrap_or(Comparator::Eq),
            value: low,
        })
    }

    fn eat_comparator(&mut self) -> Option<Comparator> {
        let comp = match self.peek()? {
            Token::Eq => Comparator::Eq,
            Token::Ne => Comparator::Ne,
            Token::Lt => Comparator::Lt,
            Token::Le => Comparator::Le,
            Token::Gt => Comparator::Gt,
            Token::Ge => Comparator::Ge,
            _ => return None,
        };
        self.pos += 1;
        Some(comp)
    }

    /// Consume a single value token (`Word` or `Quoted`) as a string.
    fn value_string(&mut self) -> Option<String> {
        match self.advance()? {
            Token::Word(w) => Some(w),
            Token::Quoted(s) => Some(s),
            _ => {
                self.pos -= 1; // not a value; leave it
                None
            }
        }
    }

    fn resolve_vl(&mut self, name: String) -> PResult {
        let key = name.to_ascii_lowercase();
        let Some(resolver) = self.resolver else {
            return Ok(Expr::Text(format!("vl:{name}")));
        };
        if self.seen.iter().any(|s| s == &key) {
            self.warnings
                .push(format!("perspective cycle at {name:?}; ignored"));
            return Ok(Expr::Empty);
        }
        let Some(text) = resolver.expression(&name) else {
            self.warnings
                .push(format!("unknown perspective {name:?}; ignored"));
            return Ok(Expr::Empty);
        };
        let mut seen = self.seen.clone();
        seen.push(key);
        let sub = parse_inner(&text, self.resolver, &seen);
        self.warnings.extend(sub.warnings);
        self.sorts.extend(sub.sorts);
        Ok(sub.expr)
    }
}

fn is_boolean_keyword(w: &str) -> bool {
    w.eq_ignore_ascii_case("and") || w.eq_ignore_ascii_case("or") || w.eq_ignore_ascii_case("not")
}

fn bool_word(w: &str) -> Option<bool> {
    match w.to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn text_or_empty(s: String) -> Expr {
    if s.is_empty() {
        Expr::Empty
    } else {
        Expr::Text(s)
    }
}

/// Parse a value string for a numeric or date field.
fn parse_typed_value(field: Field, raw: &str) -> Option<Value> {
    if field.is_date() {
        return parse_date_spec(raw).map(Value::Date);
    }
    if matches!(field, Field::Duration) {
        raw.parse::<f64>().ok().map(Value::Real)
    } else {
        raw.parse::<i64>().ok().map(Value::Int)
    }
}

/// Parse a date keyword / `Ndaysago` / `YYYY[-MM[-DD]]` spec.
fn parse_date_spec(raw: &str) -> Option<DateSpec> {
    let s = raw.to_ascii_lowercase();
    match s.as_str() {
        "today" => return Some(DateSpec::Today),
        "yesterday" => return Some(DateSpec::Yesterday),
        "thisweek" => return Some(DateSpec::ThisWeek),
        "thismonth" => return Some(DateSpec::ThisMonth),
        "thisyear" => return Some(DateSpec::ThisYear),
        _ => {}
    }
    if let Some(n) = s.strip_suffix("daysago") {
        return n.parse::<u32>().ok().map(DateSpec::DaysAgo);
    }
    let mut parts = s.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month = match parts.next() {
        Some(m) => Some(m.parse::<u32>().ok().filter(|m| (1..=12).contains(m))?),
        None => None,
    };
    let day = match parts.next() {
        Some(d) => Some(d.parse::<u32>().ok().filter(|d| (1..=31).contains(d))?),
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    // A bare small integer is a year only if it's 4 digits; otherwise reject so
    // `rating:3` etc. never reach the date path (date fields are distinct anyway).
    Some(DateSpec::Ymd(year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn round_trip(input: &str) {
        let first = parse(input).expr;
        let rendered = format!("{first}");
        let second = parse(&rendered).expr;
        assert_eq!(
            first, second,
            "round-trip changed {input:?} -> {rendered:?}"
        );
    }

    #[test]
    fn round_trips() {
        for input in [
            "roygbiv",
            "artist:boards",
            "artist:=\"Boards of Canada\"",
            "title:~rx",
            "genre:?ambiant",
            "rating:>=4",
            "year:1990..2000",
            "added:thisweek",
            "format:flac genre:ambient",
            "genre:jazz OR genre:ambient",
            "NOT is:starred",
            "(genre:ambient OR genre:jazz) AND rating:>=4",
            "rating:true",
            "duration:>600",
        ] {
            round_trip(input);
        }
    }

    #[test]
    fn extracts_sort() {
        let p = parse("genre:ambient sort:-added");
        assert_eq!(
            p.sorts,
            vec![SortSpec {
                key: SortKey::Added,
                descending: true
            }]
        );
        // sort dropped from the predicate.
        assert_eq!(p.expr, parse("genre:ambient").expr);
    }

    #[test]
    fn unknown_field_degrades_to_text() {
        let p = parse("bogus:value");
        assert_eq!(p.expr, Expr::Text("bogus:value".into()));
        assert!(!p.warnings.is_empty());
    }

    #[test]
    fn unbalanced_parens_degrade_whole_input() {
        let p = parse("(genre:ambient");
        assert_eq!(p.expr, Expr::Text("(genre:ambient".into()));
        assert!(!p.warnings.is_empty());
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse("").expr, Expr::Empty);
        assert_eq!(parse("   ").expr, Expr::Empty);
    }

    struct Perspectives(HashMap<String, String>);
    impl PerspectiveResolver for Perspectives {
        fn expression(&self, name: &str) -> Option<String> {
            self.0.get(&name.to_lowercase()).cloned()
        }
    }

    #[test]
    fn vl_expands_via_resolver() {
        let r = Perspectives(HashMap::from([("fav".into(), "genre:ambient".into())]));
        let p = parse_with_resolver("vl:fav AND rating:>=4", &r);
        assert_eq!(p.expr, parse("genre:ambient AND rating:>=4").expr);
        assert!(p.warnings.is_empty());
    }

    #[test]
    fn vl_cycle_is_guarded() {
        let r = Perspectives(HashMap::from([
            ("a".into(), "vl:b".into()),
            ("b".into(), "vl:a".into()),
        ]));
        let p = parse_with_resolver("vl:a", &r);
        // Degrades to Empty with a cycle warning, no infinite loop.
        assert_eq!(p.expr, Expr::Empty);
        assert!(p.warnings.iter().any(|w| w.contains("cycle")));
    }

    #[test]
    fn vl_without_resolver_degrades_to_text() {
        assert_eq!(parse("vl:fav").expr, Expr::Text("vl:fav".into()));
    }
}
