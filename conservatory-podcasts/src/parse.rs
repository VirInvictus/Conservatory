//! Feed parsing: `feed-rs` for the RSS/Atom core, merged with the hand-rolled
//! `podcast:`/`itunes:` [`namespace`](crate::namespace) pass (Phase 6a-ii-b).
//!
//! [`parse_feed`] turns a fetched body into a [`ParsedFeed`]: the channel
//! metadata plus a flat list of [`ParsedEpisode`]s. It is storage-agnostic
//! (no `conservatory-core` types) so it stays a pure, fixture-testable
//! function; the refresh orchestration ([`crate::refresh`]) maps these into
//! core `Show` / `Episode` rows.
//!
//! Episode identity is `(show_id, guid)` (spec §8); the guid here is the
//! item-level `<podcast:guid>` when present, else feed-rs's entry id (which is
//! the RSS `<guid>` or a hash of the first link). Notes are extracted raw here
//! (the `<description>` / summary, falling back to `<content:encoded>` when it
//! is blank); the `ammonia` sanitise and chapter fetch both happen in the
//! refresh layer ([`crate::refresh`]), not here.

use chrono::{DateTime, Utc};

use crate::error::{FetchError, Result};
use crate::namespace::{self, ItemNamespaceData};

/// Channel-level metadata from a parsed feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelMeta {
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    /// `<podcast:guid>` at channel level — a stable cross-platform show id.
    pub podcast_guid: Option<String>,
}

/// One feed item, flattened to the fields a core `Episode` needs. Storage
/// concerns (`show_id`, `folder_path`, `audio_path`) are filled by the refresh
/// layer, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub pub_date: Option<DateTime<Utc>>,
    pub duration: Option<u32>, // seconds
    pub file_size: Option<u64>,
    pub audio_url: Option<String>,
    pub mime_type: Option<String>,
    pub season: Option<u32>,
    pub episode_number: Option<u32>,
    pub episode_type: Option<String>,
    pub chapters_url: Option<String>,
}

/// A fully parsed feed: channel metadata plus its episodes in feed order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFeed {
    pub channel: ChannelMeta,
    pub episodes: Vec<ParsedEpisode>,
}

/// Parse a fetched feed body. The bytes are parsed twice: once by feed-rs for
/// the RSS/Atom/JSON core, and once by the `podcast:`/`itunes:` namespace pass
/// (XML only). The two are merged by item position, with a guid cross-check.
pub fn parse_feed(body: &[u8]) -> Result<ParsedFeed> {
    let feed = feed_rs::parser::parse(body).map_err(|e| FetchError::Parse(e.to_string()))?;

    // The namespace pass only understands XML. A JSON Feed simply yields no
    // namespace data (every lookup falls back), which is correct.
    let ns = match std::str::from_utf8(body) {
        Ok(text) => namespace::parse(text),
        Err(_) => namespace::NamespaceData::default(),
    };

    let channel = ChannelMeta {
        title: feed
            .title
            .map(|t| t.content)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Untitled Podcast".to_string()),
        author: feed.authors.into_iter().next().map(|p| p.name),
        description: feed.description.map(|t| t.content),
        homepage_url: homepage_link(&feed.links),
        podcast_guid: ns.show_guid,
    };

    let episodes = feed
        .entries
        .into_iter()
        .enumerate()
        .map(|(i, entry)| {
            let ns_item = ns.items.get(i);
            map_entry(entry, ns_item)
        })
        .collect();

    Ok(ParsedFeed { channel, episodes })
}

/// Pick the human-facing homepage link: the first link that is not the feed's
/// own `self` reference. Atom marks the feed URL with `rel="self"`; RSS rarely
/// does, so the first link is almost always the website.
fn homepage_link(links: &[feed_rs::model::Link]) -> Option<String> {
    links
        .iter()
        .find(|l| l.rel.as_deref() != Some("self"))
        .or_else(|| links.first())
        .map(|l| l.href.clone())
}

fn map_entry(entry: feed_rs::model::Entry, ns: Option<&ItemNamespaceData>) -> ParsedEpisode {
    // guid: item-level podcast:guid wins (spec §8), else feed-rs's entry id.
    let guid = ns
        .and_then(|n| n.podcast_guid.clone())
        .unwrap_or_else(|| entry.id.clone());

    // Cross-check the position merge: the namespace item's RSS <guid> should
    // match feed-rs's entry id. A mismatch means the two parsers disagree on
    // item order (rare); log it but trust feed-rs's structural parse.
    if let Some(rss_guid) = ns.and_then(|n| n.rss_guid.as_deref())
        && rss_guid != entry.id
    {
        tracing::warn!(
            feed_rs_id = %entry.id,
            namespace_guid = %rss_guid,
            "parse: namespace/feed-rs item order disagree; using feed-rs order"
        );
    }

    let title = entry
        .title
        .map(|t| t.content)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Untitled Episode".to_string());

    // Notes: the RSS <description> / Atom summary, falling back to
    // <content:encoded> when that is absent OR blank. Cortex (and others) ship an
    // empty <description/> with the real notes in content:encoded; feed-rs then
    // hands us `summary = Some("")`, so a plain `summary.or(content)` keeps the
    // empty string and stores no notes. Treat blank as absent at each step; a
    // blank result either way collapses to None (the ingest sanitize no-ops).
    let description = entry
        .summary
        .map(|t| t.content)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| entry.content.and_then(|c| c.body))
        .filter(|s| !s.trim().is_empty());

    // Enclosure: feed-rs maps RSS <enclosure> (and MediaRSS content) into the
    // media objects. Take the first content that carries a URL.
    let enclosure = entry
        .media
        .iter()
        .flat_map(|m| &m.content)
        .find(|c| c.url.is_some());

    let audio_url = enclosure.and_then(|c| c.url.as_ref().map(|u| u.to_string()));
    let mime_type = enclosure.and_then(|c| c.content_type.as_ref().map(|m| m.to_string()));
    let file_size = enclosure.and_then(|c| c.size);

    // Duration: itunes:duration lands on the media object; some feeds put it on
    // the content element instead. Prefer the object, fall back to content.
    let duration = entry
        .media
        .iter()
        .find_map(|m| m.duration)
        .or_else(|| enclosure.and_then(|c| c.duration))
        .map(|d| d.as_secs().min(u32::MAX as u64) as u32);

    ParsedEpisode {
        guid,
        title,
        description,
        pub_date: entry.published,
        duration,
        file_size,
        audio_url,
        mime_type,
        season: ns.and_then(|n| n.season),
        episode_number: ns.and_then(|n| n.episode),
        episode_type: ns.and_then(|n| n.episode_type.clone()),
        chapters_url: ns.and_then(|n| n.chapters_url.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
    <rss version="2.0"
         xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd"
         xmlns:podcast="https://podcastindex.org/namespace/1.0">
      <channel>
        <title>Test Show</title>
        <link>https://example.com</link>
        <description>A test podcast.</description>
        <itunes:author>Jane Host</itunes:author>
        <podcast:guid>show-guid-xyz</podcast:guid>
        <item>
          <title>Episode One</title>
          <guid>ep-1-guid</guid>
          <pubDate>Tue, 05 Mar 2024 10:00:00 GMT</pubDate>
          <enclosure url="https://example.com/1.mp3" length="123456" type="audio/mpeg"/>
          <itunes:duration>1830</itunes:duration>
          <podcast:season>1</podcast:season>
          <podcast:episode>1</podcast:episode>
          <itunes:episodeType>full</itunes:episodeType>
          <description>First episode.</description>
        </item>
        <item>
          <title>Episode Two</title>
          <podcast:guid>pod-guid-2</podcast:guid>
          <guid>ep-2-rss-guid</guid>
          <enclosure url="https://example.com/2.mp3" length="222" type="audio/mpeg"/>
        </item>
      </channel>
    </rss>"#;

    #[test]
    fn parses_channel_metadata() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(feed.channel.title, "Test Show");
        assert_eq!(feed.channel.author.as_deref(), Some("Jane Host"));
        assert_eq!(feed.channel.description.as_deref(), Some("A test podcast."));
        // feed-rs normalises the link URI (adds the trailing slash).
        assert_eq!(
            feed.channel.homepage_url.as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(feed.channel.podcast_guid.as_deref(), Some("show-guid-xyz"));
    }

    #[test]
    fn parses_episode_fields_and_enclosure() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(feed.episodes.len(), 2);
        let one = &feed.episodes[0];
        assert_eq!(one.guid, "ep-1-guid"); // no podcast:guid → feed-rs id
        assert_eq!(one.title, "Episode One");
        assert_eq!(one.audio_url.as_deref(), Some("https://example.com/1.mp3"));
        assert_eq!(one.mime_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(one.file_size, Some(123456));
        assert_eq!(one.duration, Some(1830));
        assert_eq!(one.season, Some(1));
        assert_eq!(one.episode_number, Some(1));
        assert_eq!(one.episode_type.as_deref(), Some("full"));
        assert!(one.pub_date.is_some());
    }

    #[test]
    fn podcast_guid_overrides_rss_guid() {
        let feed = parse_feed(RSS.as_bytes()).unwrap();
        assert_eq!(feed.episodes[1].guid, "pod-guid-2");
    }

    // An empty <description/> with the real notes in <content:encoded> (the
    // Cortex shape): the parser must fall through to content, not keep the empty
    // string. Regression for "show notes didn't appear" on recent Cortex episodes.
    const RSS_EMPTY_DESC_WITH_CONTENT: &str = r#"<?xml version="1.0"?>
    <rss version="2.0"
         xmlns:content="http://purl.org/rss/1.0/modules/content/">
      <channel>
        <title>Notes Show</title>
        <link>https://example.com</link>
        <item>
          <title>Episode With Content</title>
          <guid>ep-content</guid>
          <description/>
          <content:encoded><![CDATA[<p>The real <b>show notes</b> live here.</p>]]></content:encoded>
          <enclosure url="https://example.com/c.mp3" length="1" type="audio/mpeg"/>
        </item>
      </channel>
    </rss>"#;

    #[test]
    fn empty_description_falls_back_to_content_encoded() {
        let feed = parse_feed(RSS_EMPTY_DESC_WITH_CONTENT.as_bytes()).unwrap();
        let ep = &feed.episodes[0];
        // The raw content body is taken (the ingest sanitize cleans it later); the
        // empty <description/> must not win.
        assert_eq!(
            ep.description.as_deref(),
            Some("<p>The real <b>show notes</b> live here.</p>")
        );
    }

    #[test]
    fn rejects_non_feed_body() {
        assert!(matches!(
            parse_feed(b"<html><body>not a feed</body></html>"),
            Err(FetchError::Parse(_))
        ));
    }
}
