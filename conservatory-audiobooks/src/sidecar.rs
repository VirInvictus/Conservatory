//! Audiobookshelf sidecar reader (Phase 7a-ii).
//!
//! Three conventions, all optional, read from the book's folder:
//! - `metadata.opf` (Dublin Core, the Calibre/Audiobookshelf export) via
//!   `quick-xml`, the same event-loop idiom the podcast OPML parser uses;
//! - `desc.txt` -> a plain-text description;
//! - `reader.txt` -> the narrator(s).
//!
//! The single-purpose text files override their `.opf` field when present (they
//! are the more explicit signal). Everything is `Option` / `Vec`; an absent or
//! malformed sidecar yields empty fields, never an error.

use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::tags::split_people;

/// The sidecar view of a book, before merge with tags/folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidecarMeta {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    pub narrators: Vec<String>,
    pub series: Option<String>,
    pub series_sequence: Option<f64>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
}

/// Read whatever sidecars exist in `dir` into a [`SidecarMeta`].
pub fn read_sidecars(dir: &Path) -> SidecarMeta {
    let mut meta = std::fs::read_to_string(dir.join("metadata.opf"))
        .ok()
        .map(|s| parse_opf(&s))
        .unwrap_or_default();

    if let Ok(text) = std::fs::read_to_string(dir.join("desc.txt")) {
        let text = text.trim();
        if !text.is_empty() {
            meta.description = Some(text.to_string());
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("reader.txt")) {
        let names = split_people(text.trim());
        if !names.is_empty() {
            meta.narrators = names;
        }
    }
    meta
}

/// Parse a Calibre/Audiobookshelf `.opf` (OPF 2 / Dublin Core). Forgiving: an
/// unrecoverable XML error returns whatever was collected so far.
fn parse_opf(content: &str) -> SidecarMeta {
    let mut meta = SidecarMeta::default();
    let mut reader = Reader::from_str(content);

    // The element we are inside and the role/scheme captured from its open tag,
    // so the following text event lands in the right field.
    let mut current: Vec<u8> = Vec::new();
    let mut role: Option<String> = None;
    let mut scheme: Option<String> = None;
    let mut text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                current = e.local_name().as_ref().to_vec();
                role = attr(&e, b"role");
                scheme = attr(&e, b"scheme");
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                // `<meta name=.. content=../>` carries Calibre's series fields.
                if e.local_name().as_ref() == b"meta" {
                    apply_meta(&mut meta, &e);
                }
            }
            Ok(Event::Text(e)) => {
                if !current.is_empty()
                    && let Ok(t) = e.unescape()
                {
                    text.push_str(&t);
                }
            }
            Ok(Event::End(_)) => {
                apply_element(
                    &mut meta,
                    &current,
                    text.trim(),
                    role.as_deref(),
                    scheme.as_deref(),
                );
                current.clear();
                role = None;
                scheme = None;
                text.clear();
            }
            Err(err) => {
                tracing::warn!(?err, "opf parser: recoverable XML error; stopping");
                break;
            }
            _ => {}
        }
    }
    meta
}

/// Read one attribute by local name, unescaped.
fn attr(e: &quick_xml::events::BytesStart<'_>, key: &[u8]) -> Option<String> {
    for a in e.attributes().with_checks(false).flatten() {
        if a.key.local_name().as_ref() == key {
            let raw = String::from_utf8_lossy(&a.value);
            return Some(
                quick_xml::escape::unescape(&raw)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| raw.into_owned()),
            );
        }
    }
    None
}

/// Consume one Dublin Core element's text into the right field.
fn apply_element(
    meta: &mut SidecarMeta,
    name: &[u8],
    text: &str,
    role: Option<&str>,
    scheme: Option<&str>,
) {
    if text.is_empty() {
        return;
    }
    match name {
        b"title" => meta.title = Some(text.to_string()),
        b"subtitle" => meta.subtitle = Some(text.to_string()),
        b"creator" => match role {
            // opf:role aut = author, nrt = narrator (the Audiobookshelf mapping).
            Some("nrt") => meta.narrators.extend(split_people(text)),
            _ => meta.authors.extend(split_people(text)),
        },
        b"description" => meta.description = Some(text.to_string()),
        b"publisher" => meta.publisher = Some(text.to_string()),
        b"language" => meta.language = Some(text.to_string()),
        b"date" => meta.year = parse_year(text),
        b"identifier" => match scheme.map(str::to_ascii_uppercase).as_deref() {
            Some("ISBN") => meta.isbn = Some(text.to_string()),
            Some("ASIN" | "MOBI-ASIN") => meta.asin = Some(text.to_string()),
            _ => {}
        },
        _ => {}
    }
}

/// Apply a Calibre `<meta name="calibre:series.." content=..>` tag.
fn apply_meta(meta: &mut SidecarMeta, e: &quick_xml::events::BytesStart<'_>) {
    let name = attr(e, b"name");
    let content = attr(e, b"content");
    if let (Some(name), Some(content)) = (name, content) {
        match name.as_str() {
            "calibre:series" => meta.series = Some(content),
            "calibre:series_index" => meta.series_sequence = content.trim().parse().ok(),
            _ => {}
        }
    }
}

fn parse_year(s: &str) -> Option<i32> {
    s.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPF: &str = r#"<?xml version="1.0"?>
<package xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title>The Name of the Wind</dc:title>
    <dc:creator opf:role="aut">Patrick Rothfuss</dc:creator>
    <dc:creator opf:role="nrt">Nick Podehl</dc:creator>
    <dc:description>Kvothe tells his story.</dc:description>
    <dc:language>en</dc:language>
    <dc:publisher>Brilliance Audio</dc:publisher>
    <dc:date>2009-04-01</dc:date>
    <dc:identifier opf:scheme="ISBN">9781596007482</dc:identifier>
    <dc:identifier opf:scheme="ASIN">B0036WMOG2</dc:identifier>
    <meta name="calibre:series" content="The Kingkiller Chronicle"/>
    <meta name="calibre:series_index" content="1"/>
  </metadata>
</package>"#;

    #[test]
    fn parses_dublin_core_and_calibre_series() {
        let m = parse_opf(OPF);
        assert_eq!(m.title.as_deref(), Some("The Name of the Wind"));
        assert_eq!(m.authors, vec!["Patrick Rothfuss"]);
        assert_eq!(m.narrators, vec!["Nick Podehl"]);
        assert_eq!(m.description.as_deref(), Some("Kvothe tells his story."));
        assert_eq!(m.language.as_deref(), Some("en"));
        assert_eq!(m.publisher.as_deref(), Some("Brilliance Audio"));
        assert_eq!(m.year, Some(2009));
        assert_eq!(m.isbn.as_deref(), Some("9781596007482"));
        assert_eq!(m.asin.as_deref(), Some("B0036WMOG2"));
        assert_eq!(m.series.as_deref(), Some("The Kingkiller Chronicle"));
        assert_eq!(m.series_sequence, Some(1.0));
    }

    #[test]
    fn malformed_opf_does_not_panic() {
        let m = parse_opf("<package><metadata><dc:title>Half");
        // Whatever was captured before EOF is kept; no panic.
        assert!(m.title.is_none() || m.title.as_deref() == Some("Half"));
    }
}
