//! Hand-rolled `podcast:` (and `itunes:`) namespace handler.
//!
//! Ported from Belfry's `fetch/namespace.rs`. `feed-rs` covers the RSS / Atom
//! core, but the Podcast Index `<podcast:*>` namespace and the episode-ordering
//! fields of the iTunes namespace need project-owned parsing. This pass walks
//! the raw XML body via `quick-xml`'s event reader and extracts:
//!
//! - `<podcast:guid>` at channel level (canonical show identity).
//! - `<podcast:guid>` at item level (canonical episode identity).
//! - `<podcast:season>` / `<itunes:season>` (number; `podcast:` wins).
//! - `<podcast:episode>` / `<itunes:episode>` (number; `podcast:` wins).
//! - `<itunes:episodeType>` (full / trailer / bonus).
//! - `<podcast:chapters url="..." type="..."/>` — URL captured; storage of the
//!   chapters themselves is deferred (Phase 6a-iii / 6b).
//!
//! The `itunes:` episode/season fields are a Conservatory extension over the
//! Belfry port: `feed-rs` does not surface them, and real-world Apple-style
//! feeds carry season/episode/type there far more often than in the
//! `podcast:` namespace, so without this the columns would almost never
//! populate. `podcast:` values take precedence when both appear.
//!
//! Unknown `<podcast:*>` elements are logged at TRACE and dropped. The parser
//! is tolerant of malformed XML: recoverable errors are logged at WARN and
//! whatever was parsed cleanly is returned.
//!
//! ## Known limitations
//!
//! - Assumes the standard `podcast:` / `itunes:` prefixes. Feeds declaring
//!   either namespace under a non-standard prefix are not handled; real-world
//!   feeds use the conventional prefixes.
//! - First-text-wins for split-content text events. CDATA-wrapped content with
//!   mixed Text+CData events captures only the first segment. Real feeds don't
//!   split namespace text content this way.

use quick_xml::Reader;
use quick_xml::events::{BytesEnd, BytesStart, Event};

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct NamespaceData {
    /// `<podcast:guid>` at channel level — canonical show identity.
    pub show_guid: Option<String>,
    /// One entry per `<item>` (RSS) or `<entry>` (Atom) in source order.
    /// The parser merges these with feed-rs entries by position.
    pub items: Vec<ItemNamespaceData>,
}

#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ItemNamespaceData {
    /// RSS `<guid>` for this item — used for cross-checking the position-based
    /// merge with feed-rs's parsed entries.
    pub rss_guid: Option<String>,
    /// `<podcast:guid>` at item level.
    pub podcast_guid: Option<String>,
    /// `<podcast:season>` / `<itunes:season>` number.
    pub season: Option<u32>,
    /// `<podcast:episode>` / `<itunes:episode>` number.
    pub episode: Option<u32>,
    /// `<itunes:episodeType>` (full / trailer / bonus), kept as the raw string.
    pub episode_type: Option<String>,
    /// `<podcast:chapters url="..." />` URL.
    pub chapters_url: Option<String>,
}

/// Parse an XML body for `<podcast:*>` / `<itunes:*>` namespace elements.
///
/// Tolerant of malformed XML: unrecoverable errors abort the loop and return
/// whatever was parsed cleanly. Returns `NamespaceData::default()` on
/// completely unparseable input.
pub fn parse(xml: &str) -> NamespaceData {
    let mut reader = Reader::from_str(xml);
    let mut data = NamespaceData::default();
    let mut state = ParseState::default();

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => handle_start(&mut state, e),
            Ok(Event::Empty(e)) => handle_empty(&mut state, e),
            Ok(Event::End(e)) => handle_end(&mut state, &mut data, e),
            Ok(Event::Text(t)) => {
                let raw = t.unescape().unwrap_or_default();
                handle_text(&mut state, &mut data, raw.as_ref());
            }
            Ok(Event::CData(t)) => {
                let raw = String::from_utf8_lossy(t.as_ref());
                handle_text(&mut state, &mut data, raw.as_ref());
            }
            Ok(_) => {} // Decl, PI, Comment, DocType — ignored.
            Err(e) => {
                tracing::warn!(?e, "namespace parser: recoverable XML error; skipping");
                continue;
            }
        }
    }

    data
}

#[derive(Debug, Default)]
struct ParseState {
    in_channel: bool,
    in_item: bool,
    current_item: Option<ItemNamespaceData>,
    /// What the next Text/CData event populates. Cleared once consumed
    /// (first-text-wins).
    pending_text: Option<TextTarget>,
}

#[derive(Debug, Clone, Copy)]
enum TextTarget {
    ShowGuid,
    ItemRssGuid,
    ItemPodcastGuid,
    /// `podcast: true` overwrites; `itunes:` only fills a still-empty field.
    ItemSeason {
        podcast: bool,
    },
    ItemEpisode {
        podcast: bool,
    },
    ItemEpisodeType,
}

fn handle_start(state: &mut ParseState, e: BytesStart<'_>) {
    let (prefix_str, local_str) = element_name(&e);

    state.pending_text = match (prefix_str.as_str(), local_str.as_str()) {
        ("", "channel") | ("", "feed") => {
            state.in_channel = true;
            None
        }
        ("", "item") | ("", "entry") => {
            state.in_item = true;
            state.current_item = Some(ItemNamespaceData::default());
            None
        }
        ("", "guid") if state.in_item => Some(TextTarget::ItemRssGuid),
        ("podcast", "guid") if state.in_item => Some(TextTarget::ItemPodcastGuid),
        ("podcast", "guid") if state.in_channel => Some(TextTarget::ShowGuid),
        ("podcast", "season") if state.in_item => Some(TextTarget::ItemSeason { podcast: true }),
        ("podcast", "episode") if state.in_item => Some(TextTarget::ItemEpisode { podcast: true }),
        ("itunes", "season") if state.in_item => Some(TextTarget::ItemSeason { podcast: false }),
        ("itunes", "episode") if state.in_item => Some(TextTarget::ItemEpisode { podcast: false }),
        ("itunes", "episodeType") if state.in_item => Some(TextTarget::ItemEpisodeType),
        ("podcast", other) => {
            tracing::trace!(element = %other, "namespace parser: skipping unknown <podcast:>");
            None
        }
        _ => None,
    };
}

fn handle_empty(state: &mut ParseState, e: BytesStart<'_>) {
    let (prefix_str, local_str) = element_name(&e);

    if prefix_str == "podcast"
        && local_str == "chapters"
        && let Some(item) = state.current_item.as_mut()
    {
        for attr in e.attributes().with_checks(false).flatten() {
            if attr.key.as_ref() == b"url" {
                // URLs in chapter elements don't typically contain XML
                // entities; raw UTF-8 conversion is sufficient.
                let url = String::from_utf8_lossy(attr.value.as_ref()).into_owned();
                item.chapters_url = Some(url);
            }
        }
    }
}

fn handle_text(state: &mut ParseState, data: &mut NamespaceData, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let target = match state.pending_text.take() {
        Some(t) => t,
        None => return,
    };
    match target {
        TextTarget::ShowGuid => data.show_guid = Some(trimmed.to_string()),
        TextTarget::ItemRssGuid => {
            if let Some(item) = state.current_item.as_mut() {
                item.rss_guid = Some(trimmed.to_string());
            }
        }
        TextTarget::ItemPodcastGuid => {
            if let Some(item) = state.current_item.as_mut() {
                item.podcast_guid = Some(trimmed.to_string());
            }
        }
        TextTarget::ItemSeason { podcast } => {
            if let Some(item) = state.current_item.as_mut()
                && let Ok(n) = trimmed.parse::<u32>()
                && (podcast || item.season.is_none())
            {
                item.season = Some(n);
            }
        }
        TextTarget::ItemEpisode { podcast } => {
            if let Some(item) = state.current_item.as_mut()
                && let Ok(n) = trimmed.parse::<u32>()
                && (podcast || item.episode.is_none())
            {
                item.episode = Some(n);
            }
        }
        TextTarget::ItemEpisodeType => {
            if let Some(item) = state.current_item.as_mut() {
                item.episode_type = Some(trimmed.to_string());
            }
        }
    }
}

fn handle_end(state: &mut ParseState, data: &mut NamespaceData, e: BytesEnd<'_>) {
    let (prefix_str, local_str) = end_element_name(&e);

    match (prefix_str.as_str(), local_str.as_str()) {
        ("", "channel") | ("", "feed") => state.in_channel = false,
        ("", "item") | ("", "entry") => {
            if let Some(item) = state.current_item.take() {
                data.items.push(item);
            }
            state.in_item = false;
            state.pending_text = None;
        }
        _ => {
            // Pending text consumed by handle_text already; clear in case the
            // element body was empty (no Text event fired between Start/End).
            state.pending_text = None;
        }
    }
}

fn element_name(e: &BytesStart<'_>) -> (String, String) {
    let name = e.name();
    let local_str = String::from_utf8_lossy(name.local_name().as_ref()).into_owned();
    let prefix_str = match name.prefix() {
        Some(p) => String::from_utf8_lossy(p.as_ref()).into_owned(),
        None => String::new(),
    };
    (prefix_str, local_str)
}

fn end_element_name(e: &BytesEnd<'_>) -> (String, String) {
    let name = e.name();
    let local_str = String::from_utf8_lossy(name.local_name().as_ref()).into_owned();
    let prefix_str = match name.prefix() {
        Some(p) => String::from_utf8_lossy(p.as_ref()).into_owned(),
        None => String::new(),
    };
    (prefix_str, local_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0"?>
    <rss xmlns:podcast="https://podcastindex.org/namespace/1.0"
         xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
      <channel>
        <podcast:guid>show-guid-123</podcast:guid>
        <item>
          <guid>rss-guid-a</guid>
          <podcast:guid>pod-guid-a</podcast:guid>
          <podcast:season>2</podcast:season>
          <podcast:episode>5</podcast:episode>
          <itunes:episodeType>full</itunes:episodeType>
          <podcast:chapters url="https://ex.com/ch.json" type="application/json+chapters"/>
        </item>
        <item>
          <guid>rss-guid-b</guid>
          <itunes:season>1</itunes:season>
          <itunes:episode>9</itunes:episode>
          <itunes:episodeType>trailer</itunes:episodeType>
        </item>
      </channel>
    </rss>"#;

    #[test]
    fn extracts_show_and_item_namespace_data() {
        let data = parse(FEED);
        assert_eq!(data.show_guid.as_deref(), Some("show-guid-123"));
        assert_eq!(data.items.len(), 2);

        let a = &data.items[0];
        assert_eq!(a.rss_guid.as_deref(), Some("rss-guid-a"));
        assert_eq!(a.podcast_guid.as_deref(), Some("pod-guid-a"));
        assert_eq!(a.season, Some(2));
        assert_eq!(a.episode, Some(5));
        assert_eq!(a.episode_type.as_deref(), Some("full"));
        assert_eq!(a.chapters_url.as_deref(), Some("https://ex.com/ch.json"));
    }

    #[test]
    fn itunes_fields_fill_when_podcast_absent() {
        let data = parse(FEED);
        let b = &data.items[1];
        assert_eq!(b.rss_guid.as_deref(), Some("rss-guid-b"));
        assert_eq!(b.podcast_guid, None);
        assert_eq!(b.season, Some(1));
        assert_eq!(b.episode, Some(9));
        assert_eq!(b.episode_type.as_deref(), Some("trailer"));
    }

    #[test]
    fn podcast_namespace_wins_over_itunes_regardless_of_order() {
        // itunes first, podcast second — podcast must still win.
        let xml = r#"<rss xmlns:podcast="p" xmlns:itunes="i"><channel><item>
            <itunes:episode>1</itunes:episode>
            <podcast:episode>42</podcast:episode>
        </item></channel></rss>"#;
        assert_eq!(parse(xml).items[0].episode, Some(42));
    }

    #[test]
    fn empty_on_garbage() {
        assert_eq!(parse("not xml at all <<<"), NamespaceData::default());
    }
}
