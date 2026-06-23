//! OPML import / export (Phase 6a-iii-a).
//!
//! Round-trips a subscription list: the feed URL and title, plus the two
//! things Belfry's contract (spec §8, Belfry spec §7.6) preserves that most
//! apps drop:
//!
//! - **`applePodcastsID`** (the Overcast / Apple show id), kept verbatim in
//!   `shows.apple_podcasts_id`.
//! - **Tags**, carried as the Pocket Casts `category="a,b"` outline attribute
//!   (Conservatory's `tags` / `show_tags`, from 6a-i).
//!
//! Folder hierarchy is intentionally flattened: every `<outline>` carrying an
//! `xmlUrl` is a subscription, regardless of nesting (Belfry's tag round-trip
//! replaces folders). Import is **network-free** — it creates the subscription
//! rows; a subsequent `refresh` pulls episodes.
//!
//! The parser is forgiving in the house style (cf. [`crate::namespace`]): a
//! malformed or foreign OPML yields whatever outlines parsed cleanly rather
//! than erroring.

use conservatory_core::db::{ReadPool, Show, WorkerHandle, list_shows, list_tags_for_show};
use quick_xml::Reader;
use quick_xml::escape::{escape, unescape};
use quick_xml::events::Event;

use crate::error::Result;
use crate::slug;

/// One subscription as it appears in an OPML outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlSubscription {
    pub feed_url: String,
    pub title: String,
    pub apple_podcasts_id: Option<String>,
    pub tags: Vec<String>,
}

/// What an import did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    /// Outlines with a feed URL (the subscriptions we acted on).
    pub total: usize,
    /// Subscriptions whose feed URL was not already present.
    pub created: usize,
}

/// Parse an OPML body into its subscriptions. Forgiving: outlines without an
/// `xmlUrl` are skipped, and an unrecoverable XML error returns whatever was
/// collected so far rather than failing.
pub fn parse_opml(body: &[u8]) -> Vec<OpmlSubscription> {
    let text = String::from_utf8_lossy(body);
    let mut reader = Reader::from_str(&text);
    let mut subs = Vec::new();

    loop {
        let ev = reader.read_event();
        match ev {
            Ok(Event::Eof) => break,
            // Outlines are usually empty elements (`<outline ... />`) but a
            // folder outline is a Start with children; either way the feed
            // attributes live on the opening tag.
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"outline"
                    && let Some(sub) = outline_to_subscription(&e)
                {
                    subs.push(sub);
                }
            }
            Err(err) => {
                tracing::warn!(?err, "opml parser: recoverable XML error; stopping");
                break;
            }
            _ => {}
        }
    }
    subs
}

fn outline_to_subscription(e: &quick_xml::events::BytesStart<'_>) -> Option<OpmlSubscription> {
    let mut feed_url = None;
    let mut title = None;
    let mut text = None;
    let mut apple_podcasts_id = None;
    let mut tags = Vec::new();

    for attr in e.attributes().with_checks(false).flatten() {
        let raw = String::from_utf8_lossy(&attr.value);
        let value = unescape(&raw)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| raw.into_owned());
        match attr.key.local_name().as_ref() {
            b"xmlUrl" => feed_url = Some(value),
            b"title" => title = Some(value),
            b"text" => text = Some(value),
            b"applePodcastsID" => apple_podcasts_id = Some(value),
            b"category" => {
                tags = value
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }

    let feed_url = feed_url?;
    // OPML uses `text` as the display label; `title` is the optional formal
    // name. Prefer whichever is present, falling back to the URL.
    let title = title.or(text).unwrap_or_else(|| feed_url.clone());
    Some(OpmlSubscription {
        feed_url,
        title,
        apple_podcasts_id,
        tags,
    })
}

/// Serialize subscriptions to an OPML 2.0 document.
pub fn write_opml(subs: &[OpmlSubscription]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<opml version=\"2.0\">\n");
    out.push_str("  <head>\n    <title>Conservatory subscriptions</title>\n  </head>\n");
    out.push_str("  <body>\n");
    for sub in subs {
        out.push_str("    <outline type=\"rss\"");
        push_attr(&mut out, "text", &sub.title);
        push_attr(&mut out, "title", &sub.title);
        push_attr(&mut out, "xmlUrl", &sub.feed_url);
        if !sub.tags.is_empty() {
            push_attr(&mut out, "category", &sub.tags.join(","));
        }
        if let Some(id) = &sub.apple_podcasts_id {
            push_attr(&mut out, "applePodcastsID", id);
        }
        out.push_str("/>\n");
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

fn push_attr(out: &mut String, key: &str, value: &str) {
    out.push(' ');
    out.push_str(key);
    out.push_str("=\"");
    out.push_str(&escape(value));
    out.push('"');
}

/// Import an OPML body: create (or resolve) each subscription's show and apply
/// its tags. Network-free; episodes arrive on the next `refresh`.
pub async fn import_opml(
    worker: &WorkerHandle,
    pool: &ReadPool,
    body: &[u8],
) -> Result<ImportSummary> {
    let subs = parse_opml(body);

    // Feed URLs already subscribed, so we can report created-vs-existing.
    let existing: std::collections::HashSet<String> = {
        let conn = pool.open()?;
        list_shows(&conn)?.into_iter().map(|s| s.feed_url).collect()
    };

    let mut created = 0;
    for sub in &subs {
        if !existing.contains(&sub.feed_url) {
            created += 1;
        }
        let show_slug = slug::slugify(&sub.title);
        let skeleton = Show {
            id: 0,
            slug: show_slug.clone(),
            feed_url: sub.feed_url.clone(),
            title: sub.title.clone(),
            author: None,
            description: None,
            homepage_url: None,
            cover_path: None,
            accent_rgb: None,
            apple_podcasts_id: sub.apple_podcasts_id.clone(),
            last_fetched: None,
            last_modified: None,
            etag: None,
            fetch_interval: 3600,
            auth_user: None,
            auth_pass_ref: None,
            auto_download: false, // opt-in, not default (spec §5.3)
            keep_count: 0,
            priority: 0,
            folder_path: format!("{}/{}", slug::PODCASTS_DIR, show_slug),
        };
        let show_id = worker.get_or_create_show(skeleton).await?;
        if !sub.tags.is_empty() {
            worker.set_show_tags(show_id, sub.tags.clone()).await?;
        }
    }

    Ok(ImportSummary {
        total: subs.len(),
        created,
    })
}

/// Export every subscription (with its tags) to an OPML document.
pub async fn export_opml(pool: &ReadPool) -> Result<String> {
    let conn = pool.open()?;
    let shows = list_shows(&conn)?;
    let mut subs = Vec::with_capacity(shows.len());
    for show in shows {
        let tags = list_tags_for_show(&conn, show.id)?
            .into_iter()
            .map(|t| t.name)
            .collect();
        subs.push(OpmlSubscription {
            feed_url: show.feed_url,
            title: show.title,
            apple_podcasts_id: show.apple_podcasts_id,
            tags,
        });
    }
    Ok(write_opml(&subs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_fields() {
        let subs = vec![
            OpmlSubscription {
                feed_url: "https://a.example/feed.xml".to_string(),
                title: "Show & Tell".to_string(),
                apple_podcasts_id: Some("12345".to_string()),
                tags: vec!["news".to_string(), "tech".to_string()],
            },
            OpmlSubscription {
                feed_url: "https://b.example/feed.xml".to_string(),
                title: "Plain Show".to_string(),
                apple_podcasts_id: None,
                tags: vec![],
            },
        ];
        let xml = write_opml(&subs);
        let parsed = parse_opml(xml.as_bytes());
        assert_eq!(parsed, subs);
    }

    #[test]
    fn escapes_attribute_values() {
        let subs = vec![OpmlSubscription {
            feed_url: "https://x.example/feed?a=1&b=2".to_string(),
            title: "Quotes \"and\" <angles>".to_string(),
            apple_podcasts_id: None,
            tags: vec![],
        }];
        let xml = write_opml(&subs);
        assert!(xml.contains("&amp;"), "ampersand escaped: {xml}");
        assert!(xml.contains("&quot;"));
        assert_eq!(parse_opml(xml.as_bytes()), subs);
    }

    #[test]
    fn skips_outlines_without_feed_and_foreign_xml() {
        let xml = r#"<opml version="2.0"><body>
            <outline text="A Folder">
              <outline type="rss" text="Real Feed" xmlUrl="https://r.example/f.xml"/>
            </outline>
            <outline text="no url, just a label"/>
            <not-an-outline foo="bar"/>
        </body></opml>"#;
        let subs = parse_opml(xml.as_bytes());
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].feed_url, "https://r.example/f.xml");
        assert_eq!(subs[0].title, "Real Feed");
    }

    #[test]
    fn title_falls_back_to_text_then_url() {
        let xml = r#"<outline type="rss" xmlUrl="https://u.example/only-url.xml"/>"#;
        let subs = parse_opml(xml.as_bytes());
        assert_eq!(subs[0].title, "https://u.example/only-url.xml");
    }
}
