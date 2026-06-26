//! Show-notes sanitization (Phase 6c-iii-c).
//!
//! Feed `<description>` / `<itunes:summary>` text is HTML, often with markup,
//! tracking pixels, and the odd `<script>`. Conservatory renders notes in a
//! plain GTK `Label`, so the notes are cleaned to readable text **at ingest**
//! ([`crate::refresh`]) and the clean text is what the DB stores: the triage
//! pane, the Now Playing drawer, and the CLI all read it without re-cleaning.
//!
//! `ammonia` does the load-bearing work (it strips `<script>`/`<style>` bodies
//! and copes with malformed/nested HTML where a regex tag-strip would not). We
//! configure it to allow no tags at all, so the output is the text content with
//! the four structural characters re-escaped; a small pass decodes those and
//! tidies whitespace.

use std::collections::HashSet;

/// Clean HTML show notes to readable plain text for a GTK `Label`. Empty in →
/// empty out.
pub fn sanitize_notes(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    // Preserve block breaks as newlines before the tags are stripped, so
    // paragraphs do not run together once ammonia removes the markup.
    let pre = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n")
        .replace("</P>", "\n");

    // No allowed tags: ammonia drops every element (and `<script>`/`<style>`
    // bodies), leaving the text content. It decodes named/numeric entities to
    // their characters while parsing and only re-escapes the four structural
    // ones on output.
    let stripped = ammonia::Builder::new()
        .tags(HashSet::new())
        .clean(&pre)
        .to_string();

    // Decode the four ammonia re-escapes (`&amp;` last so an already-decoded
    // `&lt;` is not re-processed).
    let decoded = stripped
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");

    collapse_blank_lines(decoded.trim())
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
    fn strips_tags_to_text() {
        assert_eq!(sanitize_notes("<p>Hello <b>world</b></p>"), "Hello world");
    }

    #[test]
    fn decodes_structural_entities() {
        // `a < b && c > d` round-trips through ammonia's re-escaping.
        assert_eq!(
            sanitize_notes("a &lt; b &amp;&amp; c &gt; d"),
            "a < b && c > d"
        );
    }

    #[test]
    fn paragraph_breaks_become_newlines() {
        assert_eq!(sanitize_notes("<p>One</p><p>Two</p>"), "One\nTwo");
        assert_eq!(sanitize_notes("Line one<br>Line two"), "Line one\nLine two");
    }

    #[test]
    fn drops_script_bodies() {
        let cleaned = sanitize_notes("<p>Real notes</p><script>alert('x')</script>");
        assert_eq!(cleaned, "Real notes");
        assert!(!cleaned.contains("alert"));
    }

    #[test]
    fn survives_malformed_html() {
        // Unbalanced tags must not panic and must not leak markup.
        let cleaned = sanitize_notes("<p>Notes <a href=\"http://x\">link</p> trailing");
        assert!(cleaned.contains("Notes"));
        assert!(cleaned.contains("link"));
        assert!(!cleaned.contains('<'));
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
}
