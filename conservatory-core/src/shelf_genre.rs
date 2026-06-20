//! Shelf-genre resolver (spec §5.2, §7.2, docs/genre-normalization.md, Phase 2b).
//!
//! `shelf_genre` is the single-valued, filed-under value that is the *only* input
//! to the genre folder level (spec §5.1). It is decoupled from the raw,
//! multi-valued `track_genres`, which this module reads but never mutates (the
//! §5.2 decoupling). The resolver normalizes raw tags and runs the priority chain
//! to derive a stable shelf genre.
//!
//! The vocabulary seed is **empty and user-built** (settled, spec §16.4):
//! Conservatory ships no default alias map or priority list. With empty tables,
//! normalization is identity-with-cleanup (split, trim, dedup, casing preserved)
//! and ties fall to first-seen order. The schema supports seeding a vocabulary
//! later without a migration.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::errors::Result;

/// The bucket used when no genre can be derived (config `genre.default_unknown`
/// arrives with the config layer; the default is `"Unknown"`).
pub const UNKNOWN: &str = "Unknown";

/// The user-built normalization vocabulary: an alias map (`raw → canonical`) and
/// a priority list (`genre → rank`) for tie-breaks. Both keyed case-folded.
#[derive(Debug, Default, Clone)]
pub struct GenreVocab {
    aliases: HashMap<String, String>,
    priority: HashMap<String, i64>,
}

impl GenreVocab {
    /// An empty vocabulary: no aliases, no priorities (the v1 default, §16.4).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Load the alias map and priority list from the database.
    pub fn load(conn: &Connection) -> Result<Self> {
        let mut aliases = HashMap::new();
        let mut stmt = conn.prepare("SELECT raw, canonical FROM genre_aliases")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>("raw")?, r.get::<_, String>("canonical")?))
        })?;
        for row in rows {
            let (raw, canonical) = row?;
            aliases.insert(raw.to_lowercase(), canonical);
        }

        let mut priority = HashMap::new();
        let mut stmt = conn.prepare("SELECT genre, rank FROM genre_priority")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>("genre")?, r.get::<_, i64>("rank")?))
        })?;
        for row in rows {
            let (genre, rank) = row?;
            priority.insert(genre.to_lowercase(), rank);
        }

        Ok(Self { aliases, priority })
    }

    fn rank(&self, canonical: &str) -> Option<i64> {
        self.priority.get(&canonical.to_lowercase()).copied()
    }
}

/// The inputs to a single album's shelf-genre derivation (spec §5.2 chain).
#[derive(Debug, Default)]
pub struct AlbumGenreInput<'a> {
    /// A user-set shelf genre; authoritative, never overwritten by re-import.
    pub manual_override: Option<&'a str>,
    /// A single album-level genre tag from the import draft, if the file had one.
    pub album_tag: Option<&'a str>,
    /// Per-track raw genre strings (each may itself be multi-valued, e.g.
    /// `"Electronic; Ambient"`, since raw tags are stored verbatim).
    pub track_genres: &'a [Vec<String>],
}

/// Split a raw genre string on the standard separators, trim, drop empties.
fn split_raw(raw: &str) -> impl Iterator<Item = &str> {
    raw.split([';', '/', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Normalize one raw genre string into canonical display names, deduped and
/// order-preserving. Case-folding is for matching only; the output keeps the
/// alias's canonical casing, or the raw tag's own casing when unmapped.
pub fn normalize(raw: &str, vocab: &GenreVocab) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for piece in split_raw(raw) {
        let canonical = vocab
            .aliases
            .get(&piece.to_lowercase())
            .cloned()
            .unwrap_or_else(|| piece.to_string());
        if seen.insert(canonical.to_lowercase()) {
            out.push(canonical);
        }
    }
    out
}

/// The deduped canonical set a single track contributes (its raw strings unioned).
fn normalize_track(raw_genres: &[String], vocab: &GenreVocab) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in raw_genres {
        for canonical in normalize(raw, vocab) {
            if seen.insert(canonical.to_lowercase()) {
                out.push(canonical);
            }
        }
    }
    out
}

/// Derive an album's `shelf_genre` via the priority chain (spec §5.2). The first
/// rule that yields a value wins; `Unknown` is the floor. Pure and deterministic.
pub fn resolve_shelf_genre(input: &AlbumGenreInput, vocab: &GenreVocab) -> String {
    // 1. Manual override.
    if let Some(m) = input
        .manual_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return m.to_string();
    }

    // 2. A single album-level genre tag.
    if let Some(tag) = input.album_tag {
        if let Some(first) = normalize(tag, vocab).into_iter().next() {
            return first;
        }
    }

    // 3. Most common normalized genre across the tracks. Each track contributes
    //    its set once; ties break by priority rank, then first-seen order.
    let mut counts: HashMap<String, Candidate> = HashMap::new();
    let mut order = 0usize;
    for track in input.track_genres {
        for canonical in normalize_track(track, vocab) {
            let key = canonical.to_lowercase();
            let entry = counts.entry(key).or_insert_with(|| {
                let seen_at = order;
                order += 1;
                Candidate {
                    display: canonical.clone(),
                    count: 0,
                    seen_at,
                }
            });
            entry.count += 1;
        }
    }
    if !counts.is_empty() {
        let mut cands: Vec<Candidate> = counts.into_values().collect();
        cands.sort_by(|a, b| best_first(a, b, vocab));
        return cands.swap_remove(0).display;
    }

    // 4. Unknown bucket.
    UNKNOWN.to_string()
}

struct Candidate {
    display: String,
    count: usize,
    seen_at: usize,
}

/// Order candidates best-first: higher count, then a priority entry (lower rank),
/// then earlier first-seen.
fn best_first(a: &Candidate, b: &Candidate, vocab: &GenreVocab) -> Ordering {
    b.count
        .cmp(&a.count)
        .then_with(|| priority_cmp(&a.display, &b.display, vocab))
        .then_with(|| a.seen_at.cmp(&b.seen_at))
}

fn priority_cmp(a: &str, b: &str, vocab: &GenreVocab) -> Ordering {
    match (vocab.rank(a), vocab.rank(b)) {
        (Some(x), Some(y)) => x.cmp(&y), // lower rank = higher priority
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Derive `shelf_genre` for an album from the database (the DB-driven entry
/// point). Reads the album's per-track genres; no album-level tag or manual
/// override is tracked in the schema yet, so this re-derives purely from tags.
pub fn resolve_album(conn: &Connection, album_id: i64, vocab: &GenreVocab) -> Result<String> {
    let track_genres = crate::db::reads::album_track_genres(conn, album_id)?;
    let input = AlbumGenreInput {
        track_genres: &track_genres,
        ..Default::default()
    };
    Ok(resolve_shelf_genre(&input, vocab))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(sets: &[&[&str]]) -> Vec<Vec<String>> {
        sets.iter()
            .map(|s| s.iter().map(|g| g.to_string()).collect())
            .collect()
    }

    fn resolve(input: &AlbumGenreInput) -> String {
        resolve_shelf_genre(input, &GenreVocab::empty())
    }

    #[test]
    fn empty_vocab_normalizes_by_split_and_dedup() {
        let v = GenreVocab::empty();
        assert_eq!(
            normalize("Electronic; Ambient", &v),
            ["Electronic", "Ambient"]
        );
        assert_eq!(normalize("Rock / Rock", &v), ["Rock"]); // dedup, casing kept
        assert_eq!(normalize("  ", &v), Vec::<String>::new());
    }

    #[test]
    fn aliases_map_to_canonical_casing() {
        let mut v = GenreVocab::empty();
        v.aliases.insert("idm".into(), "Electronic".into());
        v.aliases.insert("hip hop".into(), "Hip-Hop".into());
        assert_eq!(normalize("IDM", &v), ["Electronic"]);
        assert_eq!(normalize("Hip Hop; idm", &v), ["Hip-Hop", "Electronic"]);
    }

    #[test]
    fn manual_override_wins_over_everything() {
        let t = tracks(&[&["Jazz"], &["Jazz"]]);
        let input = AlbumGenreInput {
            manual_override: Some("  Electronica  "),
            album_tag: Some("Rock"),
            track_genres: &t,
        };
        assert_eq!(resolve(&input), "Electronica"); // trimmed, verbatim
    }

    #[test]
    fn album_tag_beats_track_counting() {
        let t = tracks(&[&["Jazz"], &["Jazz"]]);
        let input = AlbumGenreInput {
            album_tag: Some("Rock"),
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Rock");
    }

    #[test]
    fn most_common_track_genre_wins() {
        // Electronic on 3 tracks, Ambient on 1 (the §5.2 worked example).
        let t = tracks(&[
            &["IDM", "Electronic"],
            &["Electronic"],
            &["Ambient", "Electronic"],
        ]);
        // No alias map: "IDM" stays distinct, but "Electronic" still leads 3 to 1.
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Electronic");
    }

    #[test]
    fn agreeing_tracks_resolve_to_that_genre() {
        let t = tracks(&[&["Jazz"], &["Jazz"], &["Jazz"]]);
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Jazz");
    }

    #[test]
    fn absent_genres_fall_back_to_unknown() {
        let t = tracks(&[&[], &[]]);
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Unknown");
        // No tracks at all is also Unknown.
        let empty = AlbumGenreInput::default();
        assert_eq!(resolve(&empty), "Unknown");
    }

    #[test]
    fn tie_breaks_by_priority_then_first_seen() {
        // Two genres, one track each: a tie on count.
        let t = tracks(&[&["Ambient"], &["Electronic"]]);

        // First-seen order breaks the tie when no priority is set.
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Ambient");

        // A priority entry overrides first-seen.
        let mut v = GenreVocab::empty();
        v.priority.insert("electronic".into(), 1);
        assert_eq!(resolve_shelf_genre(&input, &v), "Electronic");
    }

    #[test]
    fn lower_rank_outranks_higher_rank() {
        let t = tracks(&[&["Ambient"], &["Electronic"]]);
        let mut v = GenreVocab::empty();
        v.priority.insert("ambient".into(), 5);
        v.priority.insert("electronic".into(), 2);
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve_shelf_genre(&input, &v), "Electronic");
    }

    #[test]
    fn raw_multi_value_in_one_tag_is_split() {
        // A single stored genre row carrying separators is still split (raw tags
        // are stored verbatim, so this case is real).
        let t = tracks(&[&["Electronic; Ambient"], &["Electronic"]]);
        let input = AlbumGenreInput {
            track_genres: &t,
            ..Default::default()
        };
        assert_eq!(resolve(&input), "Electronic");
    }
}
