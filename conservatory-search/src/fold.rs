//! Accent-folding for search matching (Phase 18a).
//!
//! `fold` removes diacritics and lowercases, so `Björk` and `bjork` match. It
//! mirrors SQLite FTS5's `unicode61 remove_diacritics 2` on the in-memory eval
//! side, which keeps the all-or-nothing dual path consistent: bare text folds on
//! the SQL path (the FTS tokenizer) and here (this function).
//!
//! This is deliberately narrower than `conservatory-core`'s dedup `norm_key`,
//! which also folds quote/dash punctuation and collapses whitespace: search
//! folding only strips diacritics and lowercases, so it never changes token
//! boundaries. Applied only to the *forgiving* match kinds (substring, quoted
//! substring, fuzzy); `=exact` and `~regex` stay literal.

use unicode_normalization::UnicodeNormalization;

/// The combining-mark range dropped after NFD decomposition (the diacritics).
const COMBINING: std::ops::RangeInclusive<char> = '\u{0300}'..='\u{036F}';

/// Diacritic-fold and lowercase `s` for accent-insensitive matching.
pub fn fold(s: &str) -> String {
    s.nfd()
        .filter(|c| !COMBINING.contains(c))
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_diacritics_and_lowercases() {
        assert_eq!(fold("Björk"), "bjork");
        assert_eq!(fold("Sigur Rós"), "sigur ros");
        assert_eq!(fold("Motörhead"), "motorhead");
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("Antonín Dvořák"), "antonin dvorak");
    }

    #[test]
    fn ascii_is_only_lowercased() {
        assert_eq!(fold("Aphex Twin"), "aphex twin");
        assert_eq!(fold("MF DOOM"), "mf doom");
    }

    #[test]
    fn is_idempotent() {
        let once = fold("Mötley Crüe");
        assert_eq!(fold(&once), once);
    }

    #[test]
    fn preserves_token_boundaries() {
        // Unlike dedup's norm_key, whitespace runs are not collapsed here.
        assert_eq!(fold("a  b"), "a  b");
    }
}
