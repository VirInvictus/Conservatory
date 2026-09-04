//! Filesystem-safe slugs for the managed `Podcasts/` tree (spec §5.3).
//!
//! Podcasts adopt the managed-download model:
//! `<root>/Podcasts/<show-slug>/<YYYY-MM-DD>--<episode-slug>/`. Unlike the
//! music path template (§5.1), this is a fixed two-level shape, so a small
//! dedicated slugifier serves it rather than the template engine.

use chrono::{DateTime, Utc};
use unicode_normalization::UnicodeNormalization;

/// Cap a slug component at a generous byte budget. Feed titles are occasionally
/// pathological (a whole sentence as a title); 80 bytes keeps the path well
/// under filesystem limits while staying readable.
const MAX_SLUG_BYTES: usize = 80;

/// The top-level managed podcast folder (relative to the library root).
pub const PODCASTS_DIR: &str = "Podcasts";

/// Turn an arbitrary string into a lowercase, filesystem-safe,
/// dash-separated slug.
///
/// Diacritics fold to their base letter (NFKD, then the combining mark drops),
/// so Latin-script titles slug to plain ASCII ("Café" -> "cafe"). Non-Latin
/// scripts keep their letters ("日本語のラジオ" stays readable) instead of
/// collapsing into an `"untitled"` collision, the path-template sanitizer's
/// convention: the constraint is filesystem safety, not ASCII. Separators,
/// control characters, punctuation, and whitespace runs each collapse to a
/// single `-`; leading/trailing dashes are trimmed; the result is capped at
/// [`MAX_SLUG_BYTES`] on a char boundary. An input that reduces to nothing
/// yields `"untitled"`, so a folder name always exists.
pub fn slugify(input: &str) -> String {
    let folded = input.nfkd().filter(|c| !is_latin_diacritic(*c));
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in folded {
        let separator = ch.is_whitespace()
            || ch == '/'
            || ch == '\\'
            || ch == '\0'
            || ch.is_control()
            || (!ch.is_alphanumeric() && !is_voicing_mark(ch));
        if separator {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.extend(ch.to_lowercase());
            prev_dash = false;
        }
    }
    // Enforce the byte cap on a char boundary (the slug is no longer
    // guaranteed ASCII, so this can no longer pop bytes).
    if out.len() > MAX_SLUG_BYTES {
        let mut end = MAX_SLUG_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The combining-diacritical block: after NFKD a precomposed accented Latin,
/// Greek, or Cyrillic letter has split into its base plus one of these, and
/// the mark is what we drop. Deliberately *not* the katakana/Hiragana voicing
/// marks (U+3099/U+309A): they carry meaning (シ + mark is ジ), so they are
/// kept (see [`is_voicing_mark`]) and render correctly in the folder name.
fn is_latin_diacritic(c: char) -> bool {
    ('\u{0300}'..='\u{036F}').contains(&c)
}

/// The two combining voiced-sound marks: not `char::is_alphanumeric`, so the
/// separator branch would eat them, but they are the difference between シ
/// and ジ. Kept in the slug.
fn is_voicing_mark(c: char) -> bool {
    ('\u{3099}'..='\u{309A}').contains(&c)
}

/// The relative folder for one episode: `Podcasts/<show-slug>/<date>--<slug>`.
///
/// `show_slug` is the already-slugified show folder component (so a show's
/// episodes all sit under one directory). A missing publish date falls back to
/// `undated`, so an episode without a `pubDate` still gets a stable folder.
pub fn episode_dir(
    show_slug: &str,
    pub_date: Option<DateTime<Utc>>,
    episode_title: &str,
) -> String {
    let date = pub_date
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "undated".to_string());
    let ep_slug = slugify(episode_title);
    format!("{PODCASTS_DIR}/{show_slug}/{date}--{ep_slug}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn basic_slug() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("The Daily — News & Notes"), "the-daily-news-notes");
    }

    #[test]
    fn collapses_and_trims_separators() {
        assert_eq!(slugify("  multiple   spaces  "), "multiple-spaces");
        assert_eq!(
            slugify("---leading-and-trailing---"),
            "leading-and-trailing"
        );
        assert_eq!(slugify("a/b\\c:d"), "a-b-c-d");
    }

    #[test]
    fn diacritics_fold_to_ascii() {
        assert_eq!(slugify("Café"), "cafe");
        assert_eq!(slugify("Hörspiel"), "horspiel");
        assert_eq!(slugify("Émission du soir"), "emission-du-soir");
    }

    #[test]
    fn non_latin_scripts_keep_their_letters() {
        // Regression for the 2026-08-23 sweep: non-ASCII titles used to
        // collapse to "untitled", so every such show collided into one folder.
        assert_ne!(slugify("日本語のラジオ"), "untitled");
        assert_ne!(slugify("中文節目"), "untitled");
        assert_ne!(
            slugify("日本語"),
            slugify("中文節目"),
            "distinct shows stay distinct"
        );
        assert_eq!(slugify("Привет"), "привет");
        // Precomposed and decomposed spellings of ジ slug identically (the
        // NFKD fold is the normalizer), and the voiced mark is not eaten.
        assert_eq!(slugify("\u{30B8}"), slugify("\u{30B7}\u{3099}"));
        assert!(slugify("\u{30B8}").contains('\u{3099}'));
    }

    #[test]
    fn empty_and_punctuation_only_fall_back() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
    }

    #[test]
    fn byte_cap_enforced() {
        let long = "a".repeat(200);
        assert!(slugify(&long).len() <= MAX_SLUG_BYTES);
        // The cap cuts on a char boundary now that a slug can be non-ASCII.
        let cjk = "日".repeat(60); // 180 bytes
        let capped = slugify(&cjk);
        assert!(capped.len() <= MAX_SLUG_BYTES);
        assert!(
            capped.chars().all(|c| c == '日'),
            "no split char: {capped:?}"
        );
    }

    #[test]
    fn episode_dir_shape() {
        let date = Utc.with_ymd_and_hms(2024, 3, 7, 12, 0, 0).unwrap();
        assert_eq!(
            episode_dir("the-daily", Some(date), "Episode One!"),
            "Podcasts/the-daily/2024-03-07--episode-one"
        );
        assert_eq!(
            episode_dir("the-daily", None, "Pilot"),
            "Podcasts/the-daily/undated--pilot"
        );
    }
}
