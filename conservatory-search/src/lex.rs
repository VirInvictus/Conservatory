//! Tokenizer (spec §3.4). Best-effort: it never fails, so the parser can decide
//! how to degrade malformed input. Ported in shape from `atrium-search`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bareword (may contain `.`, `-`, digits, unicode).
    Word(String),
    /// A `"quoted string"`.
    Quoted(String),
    Colon,
    LParen,
    RParen,
    Eq,     // =
    Ne,     // !=
    Lt,     // <
    Le,     // <=
    Gt,     // >
    Ge,     // >=
    Tilde,  // ~ (regex prefix)
    Quest,  // ? (fuzzy prefix)
    Bang,   // ! (NOT)
    DotDot, // .. (range)
}

/// Characters that always terminate a bareword.
fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | ':' | '"' | '~' | '?' | '!' | '<' | '>' | '=')
}

pub fn lex(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            '~' => {
                tokens.push(Token::Tilde);
                i += 1;
            }
            '?' => {
                tokens.push(Token::Quest);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
            }
            '>' => {
                tokens.push(two(&chars, &mut i, '=', Token::Ge, Token::Gt));
            }
            '<' => {
                tokens.push(two(&chars, &mut i, '=', Token::Le, Token::Lt));
            }
            '!' => {
                tokens.push(two(&chars, &mut i, '=', Token::Ne, Token::Bang));
            }
            '"' => {
                tokens.push(Token::Quoted(scan_quoted(&chars, &mut i)));
            }
            '.' if chars.get(i + 1) == Some(&'.') => {
                tokens.push(Token::DotDot);
                i += 2;
            }
            _ => {
                tokens.push(Token::Word(scan_word(&chars, &mut i)));
            }
        }
    }
    tokens
}

/// Consume `c`; if the next char is `second`, consume it and emit `both`, else
/// emit `single`.
fn two(chars: &[char], i: &mut usize, second: char, both: Token, single: Token) -> Token {
    *i += 1;
    if chars.get(*i) == Some(&second) {
        *i += 1;
        both
    } else {
        single
    }
}

/// Scan a `"..."` string (supports `\"`). An unterminated quote runs to EOF.
fn scan_quoted(chars: &[char], i: &mut usize) -> String {
    *i += 1; // opening quote
    let mut out = String::new();
    while *i < chars.len() {
        let c = chars[*i];
        if c == '\\' && chars.get(*i + 1) == Some(&'"') {
            out.push('"');
            *i += 2;
        } else if c == '"' {
            *i += 1;
            break;
        } else {
            out.push(c);
            *i += 1;
        }
    }
    out
}

/// Scan a bareword. Stops at a boundary char or a `..` (range), keeping a lone
/// `.` inside the word (so `1998..2004` splits but `Mr. X` keeps the dot).
fn scan_word(chars: &[char], i: &mut usize) -> String {
    let mut out = String::new();
    while *i < chars.len() {
        let c = chars[*i];
        if is_boundary(c) {
            break;
        }
        if c == '.' && chars.get(*i + 1) == Some(&'.') {
            break;
        }
        out.push(c);
        *i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_field_value() {
        assert_eq!(
            lex("artist:Aphex"),
            vec![
                Token::Word("artist".into()),
                Token::Colon,
                Token::Word("Aphex".into())
            ]
        );
    }

    #[test]
    fn lexes_operators_and_quotes() {
        assert_eq!(
            lex("year:>=1990 album:\"Selected Works\""),
            vec![
                Token::Word("year".into()),
                Token::Colon,
                Token::Ge,
                Token::Word("1990".into()),
                Token::Word("album".into()),
                Token::Colon,
                Token::Quoted("Selected Works".into()),
            ]
        );
    }

    #[test]
    fn match_kind_prefixes() {
        assert_eq!(
            lex("title:~rx genre:?amb t:=x"),
            vec![
                Token::Word("title".into()),
                Token::Colon,
                Token::Tilde,
                Token::Word("rx".into()),
                Token::Word("genre".into()),
                Token::Colon,
                Token::Quest,
                Token::Word("amb".into()),
                Token::Word("t".into()),
                Token::Colon,
                Token::Eq,
                Token::Word("x".into()),
            ]
        );
    }

    #[test]
    fn unterminated_quote_runs_to_eof() {
        assert_eq!(lex("\"no end"), vec![Token::Quoted("no end".into())]);
    }
}
