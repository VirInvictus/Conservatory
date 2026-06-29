//! Duplicate detection (Phase 8b, roadmap "Phase 8 / 8b").
//!
//! A read-only, four-tier duplicate report over the music library, ported
//! faithfully from Brandon's Lattice `--duplicates` (`modes/audit.py`). Lattice
//! scans the filesystem; Conservatory owns the DB, so a Lattice "directory" maps
//! to an **album** (a managed folder) and the analysis runs over [`DedupRow`]s.
//! Report-only: any cleanup goes through the Phase 2c mover, never here.
//!
//! The tiers (Lattice's exact semantics):
//! 1. exact-duplicate albums — same `(norm artist, norm album)` in >1 folder;
//! 2. within-album multi-format — same `(track_no, norm title)` in >1 format;
//! 3. similar-name candidates — per artist, `loose(title)` pairs with a difflib
//!    [`ratio`] ≥ 0.85 (the exact-tier pairs skipped so they do not double-report);
//! 4. track-level cross-album — same `(norm artist, norm title)` across ≥2 albums,
//!    clustered by duration (Δ ≤ 2 s) so a studio and a live take surface apart.
//!
//! Normalization and the similarity ratio are pure and unit-tested.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use unicode_normalization::UnicodeNormalization;

use crate::db::DedupRow;

/// Curly quotes / apostrophes and the dash family folded to ASCII before
/// comparison (the Lattice `_QUOTE_DASH_FOLD` table), so "Rock 'n' Roll" and
/// "Rock 'n' Roll", or an en-dash and a hyphen, key the same.
const QUOTE_DASH_FOLD: &[(char, char)] = &[
    ('\u{2018}', '\''), // ' left single quote
    ('\u{2019}', '\''), // ' right single quote
    ('\u{02BC}', '\''), // ʼ modifier letter apostrophe
    ('\u{201C}', '"'),  // " left double quote
    ('\u{201D}', '"'),  // " right double quote
    ('\u{2010}', '-'),  // ‐ hyphen
    ('\u{2011}', '-'),  // ‑ non-breaking hyphen
    ('\u{2012}', '-'),  // ‒ figure dash
    ('\u{2013}', '-'),  // – en dash
    ('\u{2014}', '-'),  // — em dash
    ('\u{2015}', '-'),  // ― horizontal bar
];

/// The base normalization key (Lattice `_norm_key`): NFKC, fold quotes/dashes,
/// collapse whitespace, trim, lowercase. Used for exact matching of artist /
/// album / track names.
pub fn norm_key(s: &str) -> String {
    let folded: String = s
        .nfkc()
        .map(|c| {
            QUOTE_DASH_FOLD
                .iter()
                .find(|(from, _)| *from == c)
                .map(|(_, to)| *to)
                .unwrap_or(c)
        })
        .collect();
    // split_whitespace collapses runs and trims; then lowercase.
    folded
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The loose key (Lattice `_loose_key`): `norm_key`, then strip a trailing
/// `feat.`/`ft.`/`featuring …` clause, then iteratively strip trailing
/// `(...)`/`[...]` groups. Used only for the fuzzy similar-name tier, so
/// "Domestica (Deluxe Edition)" and "Domestica" compare as equal.
pub fn loose_key(s: &str) -> String {
    let mut s = norm_key(s);
    if s.is_empty() {
        return s;
    }
    s = strip_feat(&s);
    loop {
        let stripped = strip_trailing_paren(&s);
        if stripped == s {
            break;
        }
        s = stripped;
    }
    s
}

/// Drop a trailing `feat.`/`feat`/`featuring`/`ft.`/`ft` clause (and everything
/// after it). Input is already `norm_key`-folded (single-spaced, lowercased).
fn strip_feat(s: &str) -> String {
    const MARKERS: &[&str] = &["feat", "feat.", "featuring", "ft", "ft."];
    let words: Vec<&str> = s.split(' ').collect();
    // A marker must be preceded by a word and followed by at least one (the
    // Lattice regex's `\s+(?:…)\s+.+$`), so scan from index 1.
    for i in 1..words.len() {
        if MARKERS.contains(&words[i]) && i + 1 < words.len() {
            return words[..i].join(" ");
        }
    }
    s.to_string()
}

/// Strip one trailing balanced `(...)` or `[...]` group (no nested brackets in
/// the content), plus the whitespace before it. `loose_key` loops this.
fn strip_trailing_paren(s: &str) -> String {
    let t = s.trim_end();
    let Some(last) = t.chars().last() else {
        return s.to_string();
    };
    let open = match last {
        ')' => '(',
        ']' => '[',
        _ => return s.to_string(),
    };
    if let Some(pos) = t.rfind(open) {
        let inner = &t[pos + open.len_utf8()..t.len() - last.len_utf8()];
        if !inner.contains(['(', '[', ')', ']']) {
            return t[..pos].trim_end().to_string();
        }
    }
    s.to_string()
}

/// Python `difflib.SequenceMatcher(None, a, b).ratio()`: `2·M / (len a + len b)`
/// where `M` is the total length of the matching blocks found by recursively
/// taking the longest contiguous match and recursing on the left and right
/// remainders (Ratcliff-Obershelp). No junk heuristic (names are short, so
/// difflib's autojunk never triggers and this matches it exactly). 1.0 for two
/// empty strings.
pub fn ratio(a: &str, b: &str) -> f64 {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let total = av.len() + bv.len();
    if total == 0 {
        return 1.0;
    }
    let matched = matching_chars(&av, &bv);
    2.0 * matched as f64 / total as f64
}

fn matching_chars(a: &[char], b: &[char]) -> usize {
    let (i, j, k) = longest_match(a, b);
    if k == 0 {
        return 0;
    }
    k + matching_chars(&a[..i], &b[..j]) + matching_chars(&a[i + k..], &b[j + k..])
}

/// The longest contiguous run common to `a` and `b`, returned as
/// `(i, j, k)` with `a[i..i+k] == b[j..j+k]`; the earliest such run on ties (the
/// difflib preference). O(len a · len b) rolling DP.
fn longest_match(a: &[char], b: &[char]) -> (usize, usize, usize) {
    let mut best = (0usize, 0usize, 0usize);
    let mut prev = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        let mut curr = vec![0usize; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            if ca == cb {
                let run = prev[j] + 1;
                curr[j + 1] = run;
                if run > best.2 {
                    best = (i + 1 - run, j + 1 - run, run);
                }
            }
        }
        prev = curr;
    }
    best
}

/// Which tiers to compute, and the two tunables (Lattice's defaults).
#[derive(Debug, Clone)]
pub struct DedupOptions {
    pub exact: bool,
    pub multiformat: bool,
    pub similar: bool,
    pub tracks: bool,
    pub similar_threshold: f64,
    pub duration_delta: f64,
}

impl Default for DedupOptions {
    fn default() -> Self {
        Self {
            exact: true,
            multiformat: true,
            similar: true,
            tracks: true,
            similar_threshold: 0.85,
            duration_delta: 2.0,
        }
    }
}

/// An album referenced in a report hit: its display title and managed folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRef {
    pub title: String,
    pub folder: String,
}

/// Tier 1: an album that exists in more than one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactAlbumDupe {
    pub artist: String,
    pub album: String,
    pub folders: Vec<String>,
}

/// Tier 2: one track present in several formats within one album folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiformatDupe {
    pub album_folder: String,
    pub track_no: Option<u32>,
    pub title: String,
    pub files: Vec<String>,
}

/// Tier 3: two albums by one artist whose loose titles are near-identical.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarAlbums {
    pub artist: String,
    pub ratio: f64,
    pub a: AlbumRef,
    pub b: AlbumRef,
}

/// Tier 4: one recording (artist + title) in ≥2 albums, a duration cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackDupe {
    pub artist: String,
    pub title: String,
    pub files: Vec<String>,
}

/// The full four-tier report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DuplicateReport {
    pub exact_albums: Vec<ExactAlbumDupe>,
    pub multiformat: Vec<MultiformatDupe>,
    pub similar_albums: Vec<SimilarAlbums>,
    pub track_dupes: Vec<TrackDupe>,
}

impl DuplicateReport {
    pub fn is_empty(&self) -> bool {
        self.exact_albums.is_empty()
            && self.multiformat.is_empty()
            && self.similar_albums.is_empty()
            && self.track_dupes.is_empty()
    }
}

/// One distinct album, collapsed from its tracks.
struct AlbumInfo {
    artist: String,
    title: String,
    folder: String,
}

/// Run the requested duplicate tiers over the library rows. Pure; the CLU
/// caller reads `dedup_rows` and renders the result.
pub fn find_duplicates(rows: &[DedupRow], opts: &DedupOptions) -> DuplicateReport {
    // Collapse to distinct albums (id -> first-seen artist/title/folder).
    let mut albums: BTreeMap<i64, AlbumInfo> = BTreeMap::new();
    for r in rows {
        if let Some(id) = r.album_id {
            albums.entry(id).or_insert_with(|| AlbumInfo {
                artist: r.album_artist.clone().unwrap_or_default(),
                title: r.album_title.clone().unwrap_or_default(),
                folder: r.album_folder.clone().unwrap_or_default(),
            });
        }
    }

    // The set of exact-duplicate album keys, shared by tiers 1 and 3.
    let mut by_album_key: HashMap<(String, String), Vec<i64>> = HashMap::new();
    for (id, a) in &albums {
        let (na, nt) = (norm_key(&a.artist), norm_key(&a.title));
        if na.is_empty() || nt.is_empty() {
            continue;
        }
        by_album_key.entry((na, nt)).or_default().push(*id);
    }
    let exact_keys: BTreeSet<(String, String)> = by_album_key
        .iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(k, _)| k.clone())
        .collect();

    let mut report = DuplicateReport::default();
    if opts.exact {
        report.exact_albums = tier_exact(&albums, &by_album_key);
    }
    if opts.multiformat {
        report.multiformat = tier_multiformat(rows);
    }
    if opts.similar {
        report.similar_albums = tier_similar(&albums, &exact_keys, opts.similar_threshold);
    }
    if opts.tracks {
        report.track_dupes = tier_tracks(rows, opts.duration_delta);
    }
    report
}

fn tier_exact(
    albums: &BTreeMap<i64, AlbumInfo>,
    by_album_key: &HashMap<(String, String), Vec<i64>>,
) -> Vec<ExactAlbumDupe> {
    let mut out = Vec::new();
    for (_, ids) in by_album_key.iter().filter(|(_, ids)| ids.len() > 1) {
        let first = &albums[&ids[0]];
        let mut folders: Vec<String> = ids.iter().map(|id| albums[id].folder.clone()).collect();
        folders.sort();
        out.push(ExactAlbumDupe {
            artist: first.artist.clone(),
            album: first.title.clone(),
            folders,
        });
    }
    out.sort_by(|a, b| (&a.artist, &a.album).cmp(&(&b.artist, &b.album)));
    out
}

fn tier_multiformat(rows: &[DedupRow]) -> Vec<MultiformatDupe> {
    // Group tracks by album folder.
    let mut by_album: BTreeMap<String, Vec<&DedupRow>> = BTreeMap::new();
    for r in rows {
        if let Some(folder) = &r.album_folder {
            by_album.entry(folder.clone()).or_default().push(r);
        }
    }
    let mut out = Vec::new();
    for (folder, tracks) in &by_album {
        let formats: BTreeSet<String> = tracks
            .iter()
            .filter_map(|t| file_ext(&t.file_path))
            .collect();
        if formats.len() < 2 {
            continue;
        }
        // Group this album's tracks by (track_no, normalized title); a key in
        // >1 format (extension) is a multi-format duplicate.
        let mut by_key: BTreeMap<(Option<u32>, String), BTreeMap<String, String>> = BTreeMap::new();
        for t in tracks {
            let Some(ext) = file_ext(&t.file_path) else {
                continue;
            };
            let key = (t.track_no, norm_key(&t.title));
            by_key
                .entry(key)
                .or_default()
                .insert(ext, t.file_path.clone());
        }
        for ((track_no, title), files) in by_key {
            if files.len() > 1 {
                out.push(MultiformatDupe {
                    album_folder: folder.clone(),
                    track_no,
                    title,
                    files: files.into_values().collect(),
                });
            }
        }
    }
    out
}

fn tier_similar(
    albums: &BTreeMap<i64, AlbumInfo>,
    exact_keys: &BTreeSet<(String, String)>,
    threshold: f64,
) -> Vec<SimilarAlbums> {
    // Group eligible albums by normalized artist (skipping exact-tier dupes and
    // empty keys), carrying the loose title for the ratio.
    let mut by_artist: BTreeMap<String, Vec<(&AlbumInfo, String)>> = BTreeMap::new();
    for a in albums.values() {
        let na = norm_key(&a.artist);
        if na.is_empty() {
            continue;
        }
        if exact_keys.contains(&(na.clone(), norm_key(&a.title))) {
            continue;
        }
        let loose = loose_key(&a.title);
        if loose.is_empty() {
            continue;
        }
        by_artist.entry(na).or_default().push((a, loose));
    }
    let mut out = Vec::new();
    for (artist, items) in &by_artist {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let (a, la) = &items[i];
                let (b, lb) = &items[j];
                let r = if la == lb { 1.0 } else { ratio(la, lb) };
                if r >= threshold {
                    out.push(SimilarAlbums {
                        artist: artist.clone(),
                        ratio: r,
                        a: AlbumRef {
                            title: a.title.clone(),
                            folder: a.folder.clone(),
                        },
                        b: AlbumRef {
                            title: b.title.clone(),
                            folder: b.folder.clone(),
                        },
                    });
                }
            }
        }
    }
    // Strongest matches first.
    out.sort_by(|x, y| {
        y.ratio
            .partial_cmp(&x.ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then((&x.artist, &x.a.title).cmp(&(&y.artist, &y.a.title)))
    });
    out
}

/// An entry in the track-dupe map: which album folder, file, and duration.
struct TrackEntry {
    folder: String,
    file: String,
    duration: Option<f64>,
}

fn tier_tracks(rows: &[DedupRow], delta: f64) -> Vec<TrackDupe> {
    let mut by_key: BTreeMap<(String, String), Vec<TrackEntry>> = BTreeMap::new();
    for r in rows {
        // Lattice: the track artist falls back to the album artist.
        let artist = r
            .track_artist
            .clone()
            .or_else(|| r.album_artist.clone())
            .unwrap_or_default();
        let (na, nt) = (norm_key(&artist), norm_key(&r.title));
        if na.is_empty() || nt.is_empty() {
            continue;
        }
        by_key.entry((na, nt)).or_default().push(TrackEntry {
            folder: r.album_folder.clone().unwrap_or_default(),
            file: r.file_path.clone(),
            duration: r.duration,
        });
    }
    let mut out = Vec::new();
    for ((artist, title), entries) in by_key {
        // Need ≥2 distinct folders before clustering is worth it.
        let folders: BTreeSet<&String> = entries.iter().map(|e| &e.folder).collect();
        if folders.len() < 2 {
            continue;
        }
        for cluster in cluster_by_duration(&entries, delta) {
            let mut files: Vec<String> = cluster.iter().map(|e| e.file.clone()).collect();
            files.sort();
            out.push(TrackDupe {
                artist: artist.clone(),
                title: title.clone(),
                files,
            });
        }
    }
    out
}

/// Partition entries into duration clusters spanning ≤ `delta` seconds (greedy
/// over the sorted durations; no-duration entries form one best-effort cluster),
/// keeping only clusters of ≥2 entries across ≥2 distinct folders (Lattice
/// `_cluster_by_duration`).
fn cluster_by_duration(entries: &[TrackEntry], delta: f64) -> Vec<Vec<&TrackEntry>> {
    let mut with: Vec<&TrackEntry> = entries.iter().filter(|e| e.duration.is_some()).collect();
    let without: Vec<&TrackEntry> = entries.iter().filter(|e| e.duration.is_none()).collect();
    with.sort_by(|a, b| {
        a.duration
            .unwrap()
            .partial_cmp(&b.duration.unwrap())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut clusters: Vec<Vec<&TrackEntry>> = Vec::new();
    if let Some((first, rest)) = with.split_first() {
        let mut current = vec![*first];
        let mut anchor = first.duration.unwrap();
        for e in rest {
            if e.duration.unwrap() - anchor <= delta {
                current.push(e);
            } else {
                clusters.push(std::mem::take(&mut current));
                current = vec![e];
                anchor = e.duration.unwrap();
            }
        }
        clusters.push(current);
    }
    if without.len() >= 2 {
        clusters.push(without);
    }

    clusters
        .into_iter()
        .filter(|c| c.len() >= 2 && c.iter().map(|e| &e.folder).collect::<BTreeSet<_>>().len() >= 2)
        .collect()
}

/// The lowercased file extension (no dot), the multi-format discriminator.
fn file_ext(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn row(
        album_id: i64,
        folder: &str,
        album_artist: &str,
        album_title: &str,
        track_no: Option<u32>,
        title: &str,
        track_artist: Option<&str>,
        file: &str,
        duration: Option<f64>,
    ) -> DedupRow {
        DedupRow {
            album_id: Some(album_id),
            album_folder: Some(folder.to_string()),
            album_artist: Some(album_artist.to_string()),
            album_title: Some(album_title.to_string()),
            track_no,
            disc_no: Some(1),
            title: title.to_string(),
            track_artist: track_artist.map(str::to_string),
            format: file_ext(file),
            duration,
            file_path: file.to_string(),
        }
    }

    #[test]
    fn norm_key_folds_quotes_dashes_case_space() {
        assert_eq!(norm_key("  The   BEATLES "), "the beatles");
        // curly apostrophe + em dash fold to ASCII.
        assert_eq!(
            norm_key("Rock \u{2019}n\u{2019} Roll \u{2014} Live"),
            "rock 'n' roll - live"
        );
        // NFKC folds a full-width latin char.
        assert_eq!(norm_key("\u{FF21}bc"), "abc");
    }

    #[test]
    fn loose_key_strips_feat_and_parens() {
        assert_eq!(loose_key("Domestica (Deluxe Edition)"), "domestica");
        assert_eq!(loose_key("Song feat. Someone"), "song");
        assert_eq!(loose_key("Track ft. X"), "track");
        assert_eq!(loose_key("Album (Remastered) (2009)"), "album");
        // A bare title is unchanged; a leading "feat" word is not a clause.
        assert_eq!(loose_key("Domestica"), "domestica");
    }

    #[test]
    fn ratio_matches_difflib() {
        assert_eq!(ratio("", ""), 1.0);
        assert_eq!(ratio("abcd", "abcd"), 1.0);
        // difflib ratio("abcd","abce") = 2*3/8 = 0.75.
        assert!((ratio("abcd", "abce") - 0.75).abs() < 1e-9);
        // domestica vs domestic = 2*8/17.
        assert!((ratio("domestica", "domestic") - (16.0 / 17.0)).abs() < 1e-9);
        assert_eq!(ratio("abcdef", "uvwxyz"), 0.0);
    }

    #[test]
    fn tier_exact_album_in_two_folders() {
        let rows = vec![
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(1),
                "T1",
                None,
                "Music/A/Album/01.flac",
                Some(100.0),
            ),
            row(
                2,
                "Music/B/Album",
                "artist",
                "album",
                Some(1),
                "T1",
                None,
                "Music/B/Album/01.flac",
                Some(100.0),
            ),
        ];
        let rep = find_duplicates(&rows, &DedupOptions::default());
        assert_eq!(rep.exact_albums.len(), 1);
        assert_eq!(rep.exact_albums[0].folders.len(), 2);
    }

    #[test]
    fn tier_multiformat_flac_and_mp3() {
        let rows = vec![
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(1),
                "Song",
                None,
                "Music/A/Album/01.flac",
                Some(100.0),
            ),
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(1),
                "Song",
                None,
                "Music/A/Album/01.mp3",
                Some(100.0),
            ),
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(2),
                "Other",
                None,
                "Music/A/Album/02.flac",
                Some(120.0),
            ),
        ];
        let rep = find_duplicates(&rows, &DedupOptions::default());
        assert_eq!(rep.multiformat.len(), 1, "only track 1 is in two formats");
        assert_eq!(rep.multiformat[0].files.len(), 2);
    }

    #[test]
    fn tier_similar_near_miss_album_name() {
        let rows = vec![
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(1),
                "T1",
                None,
                "Music/A/Album/01.flac",
                Some(100.0),
            ),
            row(
                2,
                "Music/A/Album (Remastered)",
                "Artist",
                "Album (Remastered)",
                Some(1),
                "T1",
                None,
                "Music/A/Album (Remastered)/01.flac",
                Some(100.0),
            ),
        ];
        let rep = find_duplicates(&rows, &DedupOptions::default());
        // Loose titles both reduce to "album" → ratio 1.0 → a candidate; the
        // norm titles differ so they are not an exact-album dupe.
        assert!(rep.exact_albums.is_empty());
        assert_eq!(rep.similar_albums.len(), 1);
        assert!(rep.similar_albums[0].ratio >= 0.85);
    }

    #[test]
    fn tier_tracks_cross_album_duration_split() {
        // Same recording in two albums (studio ~180s); a live take (~240s) in a
        // third album clusters separately.
        let rows = vec![
            row(
                1,
                "Music/Studio1",
                "Band",
                "Album1",
                Some(3),
                "Hit",
                None,
                "Music/Studio1/03.flac",
                Some(180.0),
            ),
            row(
                2,
                "Music/Studio2",
                "Band",
                "Album2",
                Some(5),
                "Hit",
                None,
                "Music/Studio2/05.flac",
                Some(181.0),
            ),
            row(
                3,
                "Music/Live",
                "Band",
                "LiveAlbum",
                Some(8),
                "Hit",
                None,
                "Music/Live/08.flac",
                Some(240.0),
            ),
        ];
        let rep = find_duplicates(&rows, &DedupOptions::default());
        // The two studio takes form one ≥2 cross-album cluster; the lone live
        // take does not (one folder).
        assert_eq!(rep.track_dupes.len(), 1);
        assert_eq!(rep.track_dupes[0].files.len(), 2);
    }

    #[test]
    fn tier_filter_runs_only_requested() {
        let rows = vec![
            row(
                1,
                "Music/A/Album",
                "Artist",
                "Album",
                Some(1),
                "T1",
                None,
                "Music/A/Album/01.flac",
                Some(100.0),
            ),
            row(
                2,
                "Music/B/Album",
                "Artist",
                "Album",
                Some(1),
                "T1",
                None,
                "Music/B/Album/01.flac",
                Some(100.0),
            ),
        ];
        let opts = DedupOptions {
            exact: true,
            multiformat: false,
            similar: false,
            tracks: false,
            ..DedupOptions::default()
        };
        let rep = find_duplicates(&rows, &opts);
        assert_eq!(rep.exact_albums.len(), 1);
        assert!(rep.track_dupes.is_empty(), "tracks tier was not requested");
    }
}
