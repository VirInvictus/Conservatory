//! Show-notes sanitization (Phase 6c-iii-c; links preserved since 16.5f).
//!
//! Feed `<description>` / `<itunes:summary>` text is HTML, often with markup,
//! tracking pixels, and the odd `<script>`. The notes are cleaned **at ingest**
//! ([`crate::refresh`]) and the clean form is what the DB stores: the triage
//! pane and the CLI read it without re-cleaning.
//!
//! `ammonia` does the load-bearing work (it strips `<script>`/`<style>` bodies
//! and copes with malformed/nested HTML where a regex tag-strip would not).
//! Since 16.5f the allowlist keeps the inline subset Pango can render (`a
//! href`, bold, italics), so links survive ingest; every other element is
//! stripped to its text with the structural characters left entity-escaped,
//! which is exactly what `Label::set_markup` needs. [`notes_to_markup`]
//! converts the stored subset to Pango markup at render time, and escapes
//! legacy plain-text rows whole (stored before the format change; they heal on
//! their next feed refresh, since the episode upsert rewrites `description`).

use std::collections::{HashMap, HashSet};

/// Clean HTML show notes to the stored form: readable text with the Pango
/// inline subset (`a href` / `b` / `strong` / `i` / `em`) preserved and
/// everything else stripped. Empty in → empty out.
pub fn sanitize_notes(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    // Preserve block breaks as newlines before the tags are stripped, so
    // paragraphs, headings, and list items do not run together once ammonia
    // removes the markup (real feeds are lowercase; `</P>` is the one common
    // capitalised straggler).
    let mut pre = html.to_string();
    for close in [
        "<br>", "<br/>", "<br />", "</p>", "</P>", "</h1>", "</h2>", "</h3>", "</h4>", "</h5>",
        "</h6>", "</li>", "</ul>", "</ol>", "</div>", "</tr>",
    ] {
        pre = pre.replace(close, "\n");
    }

    // The Pango-renderable inline subset survives; ammonia drops every other
    // element (and `<script>`/`<style>` bodies), leaving entity-escaped text.
    // `link_rel(None)` keeps `<a>` down to its bare `href` (Pango rejects
    // unknown attributes like the default `rel="noopener noreferrer"`).
    let stripped = ammonia::Builder::new()
        .tags(HashSet::from(["a", "b", "strong", "i", "em"]))
        .tag_attributes(HashMap::from([("a", HashSet::from(["href"]))]))
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .link_rel(None)
        .clean(&pre)
        .to_string();

    collapse_blank_lines(stripped.trim())
}

/// Convert stored notes to Pango markup for `Label::set_markup` (16.5f):
/// `strong`/`em` map to Pango's `b`/`i`, `a href` and the escaped entities
/// pass through. A legacy plain-text row (pre-16.5f: raw `&` / `<` characters)
/// is escaped whole instead, so `set_markup` never sees invalid markup. Pure.
pub fn notes_to_markup(notes: &str) -> String {
    if is_stored_markup(notes) {
        notes
            .replace("<strong>", "<b>")
            .replace("</strong>", "</b>")
            .replace("<em>", "<i>")
            .replace("</em>", "</i>")
    } else {
        escape_text(notes)
    }
}

/// Whether `notes` is the post-16.5f stored subset: every `<` opens an allowed
/// tag and every `&` an entity. Legacy rows (fully decoded text) fail on their
/// first raw `&` or `<`.
fn is_stored_markup(notes: &str) -> bool {
    const ALLOWED_AFTER_LT: [&str; 11] = [
        "a href=\"",
        "a>",
        "/a>",
        "b>",
        "/b>",
        "i>",
        "/i>",
        "em>",
        "/em>",
        "strong>",
        "/strong>",
    ];
    let bytes = notes.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => {
                let rest = &notes[i + 1..];
                if !ALLOWED_AFTER_LT.iter().any(|t| rest.starts_with(t)) {
                    return false;
                }
                // Skip to the tag's closing '>' (ammonia entity-escapes any
                // '>' inside an attribute value, so this is the real end).
                match rest.find('>') {
                    Some(j) => i += j + 2,
                    None => return false,
                }
            }
            b'&' => {
                // An entity: short, alphanumeric-or-# body, ';'-terminated.
                let rest = &notes[i + 1..];
                match rest.find(';') {
                    Some(j)
                        if (1..=8).contains(&j)
                            && rest[..j]
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '#') =>
                    {
                        i += j + 2;
                    }
                    _ => return false,
                }
            }
            _ => i += 1,
        }
    }
    true
}

/// Escape text for Pango markup (the legacy-row path).
fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Collapse runs of 3+ newlines down to a paragraph break (2), so notes with
/// heavy spacing read cleanly.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut newlines = 0;
    for ch in text.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_disallowed_tags_keeps_inline_subset() {
        // Bold survives (16.5f); the paragraph wrapper does not.
        assert_eq!(
            sanitize_notes("<p>Hello <b>world</b></p>"),
            "Hello <b>world</b>"
        );
        // A table wrapper strips to text.
        assert_eq!(
            sanitize_notes("<table><tr><td>cell</td></tr></table>"),
            "cell"
        );
    }

    #[test]
    fn links_survive_with_bare_href() {
        assert_eq!(
            sanitize_notes(
                r#"<p>See <a href="https://x.example/ep" class="btn" target="_blank">the notes</a></p>"#
            ),
            r#"See <a href="https://x.example/ep">the notes</a>"#
        );
        // Disallowed schemes lose the link but keep the text.
        let cleaned = sanitize_notes(r#"<a href="javascript:alert(1)">click</a>"#);
        assert!(cleaned.contains("click"));
        assert!(!cleaned.contains("javascript"));
    }

    #[test]
    fn structural_entities_stay_escaped_for_markup() {
        // The stored form is Pango-ready: & < > remain entities.
        assert_eq!(
            sanitize_notes("a &lt; b &amp;&amp; c &gt; d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
    }

    #[test]
    fn paragraph_breaks_become_newlines() {
        assert_eq!(sanitize_notes("<p>One</p><p>Two</p>"), "One\nTwo");
        assert_eq!(sanitize_notes("Line one<br>Line two"), "Line one\nLine two");
    }

    #[test]
    fn breaks_headings_and_list_items() {
        // The Cortex shape: an <h4> heading ahead of a <p> must not run together.
        assert_eq!(
            sanitize_notes("<h4>State of the Workflow</h4><p>Myke talks.</p>"),
            "State of the Workflow\nMyke talks."
        );
        assert_eq!(
            sanitize_notes("<ul><li>One</li><li>Two</li></ul>"),
            "One\nTwo"
        );
    }

    #[test]
    fn drops_script_bodies() {
        let cleaned = sanitize_notes("<p>Real notes</p><script>alert('x')</script>");
        assert_eq!(cleaned, "Real notes");
        assert!(!cleaned.contains("alert"));
    }

    #[test]
    fn survives_malformed_html() {
        // Unbalanced tags must not panic; ammonia balances the anchor.
        let cleaned = sanitize_notes("<p>Notes <a href=\"http://x\">link</p> trailing");
        assert!(cleaned.contains("Notes"));
        assert!(cleaned.contains("link"));
        assert!(cleaned.contains("trailing"));
    }

    #[test]
    fn collapses_excess_blank_lines() {
        assert_eq!(sanitize_notes("<p>A</p><p></p><p></p><p>B</p>"), "A\n\nB");
    }

    #[test]
    fn empty_in_empty_out() {
        assert_eq!(sanitize_notes(""), "");
        assert_eq!(sanitize_notes("   "), "");
    }

    #[test]
    fn markup_maps_strong_em_and_passes_links() {
        assert_eq!(
            notes_to_markup(
                r#"<strong>Big</strong> <em>soft</em> <a href="https://x.example">go</a>"#
            ),
            r#"<b>Big</b> <i>soft</i> <a href="https://x.example">go</a>"#
        );
        // Escaped entities in the stored form pass through untouched.
        assert_eq!(notes_to_markup("a &lt; b &amp; c"), "a &lt; b &amp; c");
    }

    #[test]
    fn markup_escapes_legacy_plain_text_rows() {
        // Pre-16.5f rows carry raw structural characters; they must never
        // reach set_markup unescaped.
        assert_eq!(
            notes_to_markup("Fish & Chips <3 tonight"),
            "Fish &amp; Chips &lt;3 tonight"
        );
        // A lone raw ampersand mid-URL is the classic legacy shape.
        assert_eq!(
            notes_to_markup("see example.com/?a=1&b=2"),
            "see example.com/?a=1&amp;b=2"
        );
    }
}
