//! Filesystem-safe slugs for the managed `Podcasts/` tree (spec §5.3).
//!
//! Podcasts adopt the managed-download model:
//! `<root>/Podcasts/<show-slug>/<YYYY-MM-DD>--<episode-slug>/`. Unlike the
//! music path template (§5.1), this is a fixed two-level shape, so a small
//! dedicated slugifier serves it rather than the template engine.

use chrono::{DateTime, Utc};

/// Cap a slug component at a generous byte budget. Feed titles are occasionally
/// pathological (a whole sentence as a title); 80 bytes keeps the path well
/// under filesystem limits while staying readable.
const MAX_SLUG_BYTES: usize = 80;

/// The top-level managed podcast folder (relative to the library root).
pub const PODCASTS_DIR: &str = "Podcasts";

/// Turn an arbitrary string into a lowercase, ASCII, dash-separated slug.
///
/// ASCII alphanumerics are kept (lowercased); every other run of characters
/// collapses to a single `-`. Leading/trailing dashes are trimmed and the
/// result is capped at [`MAX_SLUG_BYTES`] (always safe, since the slug is
/// ASCII-only: lowercase alphanumerics and `-`). An input that
/// reduces to nothing (punctuation-only, or non-ASCII-only) yields
/// `"untitled"`, so a folder name always exists.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    // Enforce the byte cap. `out` is ASCII (1 byte per char), so popping bytes
    // never splits a char; the trailing separator is trimmed just below.
    while out.len() > MAX_SLUG_BYTES {
        out.pop();
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
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
    fn empty_and_nonascii_fall_back() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
        assert_eq!(slugify("日本語"), "untitled");
    }

    #[test]
    fn byte_cap_enforced() {
        let long = "a".repeat(200);
        assert!(slugify(&long).len() <= MAX_SLUG_BYTES);
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
