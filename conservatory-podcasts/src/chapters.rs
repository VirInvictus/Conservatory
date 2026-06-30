//! Podcast chapter fetch + parse (Phase 6c-iii-a, spec §8).
//!
//! A `<podcast:chapters url="…">` element points at a JSON document (the Podcast
//! Index "Podcast Namespace" chapters format, version 1.x). The namespace handler
//! ([`crate::namespace`]) already captures the URL into `ParsedEpisode.chapters_url`;
//! this module fetches that URL and parses it into core [`Chapter`] rows, which the
//! refresh path stores through the existing `replace_chapters` worker command. The
//! storage plumbing (table, write, `list_chapters` read) predates this; only the
//! fetch + parse is new.
//!
//! `parse_chapters_json` is pure (unit-tested headless); `fetch_chapters` is the
//! thin network wrapper, sharing the refresh [`Fetcher`](crate::fetcher)'s client.

use conservatory_core::db::Chapter;
use serde::Deserialize;

use crate::error::{FetchError, Result};

/// The chapters JSON document: `{ "version": "1.2.0", "chapters": [ … ] }`. Only
/// the `chapters` array matters; a missing array is an empty set (some encoders
/// emit `{}` for an episode with no chapters).
#[derive(Deserialize)]
struct ChaptersDoc {
    #[serde(default)]
    chapters: Vec<ChapterEntry>,
}

/// One entry. `startTime` is the only required field (Podcast Index spec); the
/// rest are optional. Unknown fields (`toc`, `location`, …) are ignored.
#[derive(Deserialize)]
struct ChapterEntry {
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(default)]
    title: Option<String>,
    #[serde(rename = "endTime", default)]
    end_time: Option<f64>,
    #[serde(default)]
    url: Option<String>,
    /// A chapter image URL. Stored verbatim in `image_path`; downloading the
    /// image into the managed tree is deferred (6c-iii follow-on).
    #[serde(default)]
    img: Option<String>,
}

/// Parse a Podcast Index chapters JSON document into core [`Chapter`] rows
/// (`episode_id` / `id` are placeholders; `replace_chapters` assigns the real
/// ids). Empty / chapter-less documents yield an empty vec; malformed JSON is an
/// error (the caller treats a chapter fetch as best-effort). Pure.
pub fn parse_chapters_json(body: &str) -> Result<Vec<Chapter>> {
    let doc: ChaptersDoc =
        serde_json::from_str(body).map_err(|e| FetchError::Parse(format!("chapters json: {e}")))?;
    Ok(doc
        .chapters
        .into_iter()
        .map(|e| Chapter {
            id: 0,
            episode_id: 0,
            start_time: e.start_time,
            end_time: e.end_time,
            title: e.title.filter(|s| !s.is_empty()),
            url: e.url.filter(|s| !s.is_empty()),
            image_path: e.img.filter(|s| !s.is_empty()),
        })
        .collect())
}

/// Fetch and parse the chapters JSON at `url`, sharing `client` with the feed
/// fetcher's connection pool (a plain GET; chapters carry no conditional-GET
/// bookkeeping). Returns the parsed chapter set, or an error the caller logs.
pub async fn fetch_chapters(client: &reqwest::Client, url: &str) -> Result<Vec<Chapter>> {
    tracing::debug!(target: "conservatory::net", url, "chapters: GET");
    let body = client.get(url).send().await?.text().await?;
    let chapters = parse_chapters_json(&body)?;
    tracing::debug!(target: "conservatory::net", url, count = chapters.len(), "chapters: parsed");
    Ok(chapters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_chapters_doc() {
        let json = r#"{
            "version": "1.2.0",
            "chapters": [
                { "startTime": 0, "title": "Intro" },
                { "startTime": 73.5, "title": "Main", "endTime": 600, "url": "https://ex.com/x", "img": "https://ex.com/i.jpg" }
            ]
        }"#;
        let chs = parse_chapters_json(json).unwrap();
        assert_eq!(chs.len(), 2);
        assert_eq!(chs[0].start_time, 0.0);
        assert_eq!(chs[0].title.as_deref(), Some("Intro"));
        assert_eq!(chs[0].end_time, None);
        assert_eq!(chs[1].start_time, 73.5);
        assert_eq!(chs[1].end_time, Some(600.0));
        assert_eq!(chs[1].url.as_deref(), Some("https://ex.com/x"));
        assert_eq!(chs[1].image_path.as_deref(), Some("https://ex.com/i.jpg"));
    }

    #[test]
    fn empty_chapters_and_missing_array_yield_empty() {
        assert!(
            parse_chapters_json(r#"{"chapters": []}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_chapters_json(r#"{"version": "1.2.0"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_chapters_json("{}").unwrap().is_empty());
    }

    #[test]
    fn blank_optional_strings_become_none() {
        let chs =
            parse_chapters_json(r#"{"chapters":[{"startTime":5,"title":"","url":""}]}"#).unwrap();
        assert_eq!(chs.len(), 1);
        assert_eq!(chs[0].title, None);
        assert_eq!(chs[0].url, None);
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_chapters_json("not json").is_err());
        // A non-numeric startTime fails the whole document (startTime is required).
        assert!(parse_chapters_json(r#"{"chapters":[{"title":"no start"}]}"#).is_err());
    }
}
