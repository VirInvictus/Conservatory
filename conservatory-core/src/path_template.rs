//! Path-template engine (spec §5.1, docs/path-template.md, roadmap Phase 2a).
//!
//! The database is truth; the on-disk tree is a *render* of a configurable
//! template (the Calibre "save to disk" / beets `paths:` model). This module is
//! pure: it turns DB-derived [`TrackFields`] into a relative target path. It
//! never moves files (that is the Phase 2c mover) and never touches raw tags
//! (only the single-valued `shelf_genre` reaches the filesystem, the §5.2
//! decoupling).
//!
//! An **album is the unit that moves**: one shelf genre and one album artist
//! drive the directory even when track-level tags disagree, and a compilation
//! (no album artist) buckets under **Various Artists**. Rendering is infallible
//! once a template parses: every missing field has a fallback or collapses.

use std::path::PathBuf;

use crate::errors::{Error, Result};

/// The default music template (spec §5.1).
pub const DEFAULT_MUSIC_TEMPLATE: &str =
    "{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}";

/// Fallbacks for the structural folder levels, so a component is never empty.
const UNKNOWN_GENRE: &str = "Unknown";
const VARIOUS_ARTISTS: &str = "Various Artists";
const UNKNOWN_ALBUM: &str = "Unknown Album";
const UNTITLED: &str = "Untitled";

/// Per-component byte cap (common filesystem `NAME_MAX`), applied per component,
/// not to the whole path (docs/path-template.md "Sanitization").
const COMPONENT_MAX_BYTES: usize = 255;

/// The DB-derived values a single track contributes to its rendered path.
///
/// `albumartist` is the album artist's `sort_name` already resolved by the
/// caller; `None` means a compilation and renders as Various Artists. `artist`
/// is the track artist's display name (may differ from the album artist).
#[derive(Debug, Default, Clone)]
pub struct TrackFields<'a> {
    pub shelf_genre: Option<&'a str>,
    pub albumartist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub title: Option<&'a str>,
    pub artist: Option<&'a str>,
    /// File extension (from `tracks.format`); appended to the leaf component.
    pub ext: Option<&'a str>,
}

/// The token names the template understands (`{ext}` is appended, not a token).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    ShelfGenre,
    AlbumArtist,
    Album,
    Year,
    Track,
    Disc,
    Title,
    Artist,
}

impl Field {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "shelf_genre" => Self::ShelfGenre,
            "albumartist" => Self::AlbumArtist,
            "album" => Self::Album,
            "year" => Self::Year,
            "track" => Self::Track,
            "disc" => Self::Disc,
            "title" => Self::Title,
            "artist" => Self::Artist,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
enum Segment {
    Literal(String),
    /// A field token with an optional zero-pad width (`{track:02}` => `pad: 2`).
    Token {
        field: Field,
        pad: usize,
    },
}

type Component = Vec<Segment>;

/// A parsed path template: a sequence of path components (split on `/`), each a
/// sequence of literal and token segments.
#[derive(Debug, Clone)]
pub struct PathTemplate {
    components: Vec<Component>,
}

impl PathTemplate {
    /// Parse a template string. Returns an error for unbalanced braces, unknown
    /// token names, or a malformed format spec (config is validated, not
    /// silently mangled).
    pub fn parse(template: &str) -> Result<Self> {
        let components = template
            .split('/')
            .filter(|c| !c.is_empty())
            .map(parse_component)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { components })
    }

    /// The default music template, parsed.
    pub fn default_music() -> Self {
        // The constant is a valid template; a parse failure here is a bug.
        Self::parse(DEFAULT_MUSIC_TEMPLATE).expect("default template parses")
    }

    /// Render the relative target path for a track. Infallible: missing fields
    /// fall back or collapse, and components are sanitized and never empty.
    pub fn render(&self, fields: &TrackFields) -> PathBuf {
        let last = self.components.len().saturating_sub(1);
        let mut path = PathBuf::new();
        for (i, component) in self.components.iter().enumerate() {
            let mut name = render_component(component, fields);
            if i == last
                && let Some(ext) = fields.ext.filter(|e| !e.is_empty())
            {
                name.push('.');
                name.push_str(&sanitize_component(ext).to_lowercase());
            }
            path.push(name);
        }
        path
    }
}

fn parse_component(text: &str) -> Result<Component> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = text.char_indices().peekable();

    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' => {
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }
                let mut body = String::new();
                let mut closed = false;
                for (_, c) in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    body.push(c);
                }
                if !closed {
                    return Err(template_err(text, "unclosed '{'"));
                }
                segments.push(parse_token(&body, text)?);
            }
            '}' => return Err(template_err(text, "unexpected '}'")),
            _ => literal.push(ch),
        }
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

fn parse_token(body: &str, component: &str) -> Result<Segment> {
    let (name, spec) = match body.split_once(':') {
        Some((name, spec)) => (name, Some(spec)),
        None => (body, None),
    };
    let field = Field::parse(name)
        .ok_or_else(|| template_err(component, &format!("unknown token {name:?}")))?;
    let pad = match spec {
        None => 0,
        Some(spec) if spec.chars().all(|c| c.is_ascii_digit()) && !spec.is_empty() => {
            spec.parse().unwrap_or(0)
        }
        Some(spec) => {
            return Err(template_err(
                component,
                &format!("bad format spec {spec:?}"),
            ));
        }
    };
    Ok(Segment::Token { field, pad })
}

fn template_err(component: &str, why: &str) -> Error {
    Error::Template(format!("{why} in component {component:?}"))
}

/// Substitute a component's tokens, collapse empty-group artifacts, and sanitize.
fn render_component(component: &Component, fields: &TrackFields) -> String {
    let mut out = String::new();
    for segment in component {
        match segment {
            Segment::Literal(text) => out.push_str(text),
            Segment::Token { field, pad } => out.push_str(&token_value(*field, *pad, fields)),
        }
    }
    let cleaned = collapse_artifacts(&out);
    let sanitized = sanitize_component(&cleaned);
    if sanitized.is_empty() {
        UNKNOWN_GENRE.to_string()
    } else {
        sanitized
    }
}

/// The substituted text for one token. Structural levels fall back to a bucket
/// so the folder is never empty; optional pieces (year, track, disc, artist)
/// return empty and let the surrounding literals collapse.
fn token_value(field: Field, pad: usize, f: &TrackFields) -> String {
    match field {
        Field::ShelfGenre => f.shelf_genre.unwrap_or(UNKNOWN_GENRE).to_string(),
        Field::AlbumArtist => f.albumartist.unwrap_or(VARIOUS_ARTISTS).to_string(),
        Field::Album => f.album.unwrap_or(UNKNOWN_ALBUM).to_string(),
        Field::Title => f.title.unwrap_or(UNTITLED).to_string(),
        Field::Artist => f.artist.unwrap_or_default().to_string(),
        Field::Year => f.year.map(|y| y.to_string()).unwrap_or_default(),
        Field::Track => pad_num(f.track_no, pad),
        Field::Disc => pad_num(f.disc_no, pad),
    }
}

fn pad_num(value: Option<u32>, pad: usize) -> String {
    match value {
        Some(n) => format!("{n:0>width$}", width = pad),
        None => String::new(),
    }
}

/// Collapse the cosmetic debris an empty optional leaves behind: empty bracket
/// pairs (`()`/`[]`), runs of whitespace, and a dangling leading/trailing
/// separator (the ` - ` in `{track} - {title}` when the track is absent).
fn collapse_artifacts(s: &str) -> String {
    let mut s = s.to_string();
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        s = remove_empty_pairs(&s, open, close);
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | ',' | '.' | '_'))
        .to_string()
}

fn remove_empty_pairs(s: &str, open: char, close: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == open {
            // Look past inner whitespace for an immediate close.
            let mut inner = String::new();
            let mut emptied = false;
            while let Some(&n) = chars.peek() {
                if n == close {
                    chars.next();
                    emptied = true;
                    break;
                } else if n.is_whitespace() {
                    inner.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            if !emptied {
                out.push(open);
                out.push_str(&inner);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Make a single path component filesystem-safe (docs/path-template.md). The
/// embedded tag keeps the true value (spec §5.5); only the on-disk name changes.
fn sanitize_component(raw: &str) -> String {
    // Replace path separators and control characters; collapse whitespace.
    let replaced: String = raw
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut out = replaced.split_whitespace().collect::<Vec<_>>().join(" ");

    // Trailing dots and spaces are illegal/awkward (Windows, and trailing dot
    // hides files); strip them.
    out = out.trim_end_matches(['.', ' ']).to_string();

    // Reserved device names (Windows) get a trailing underscore even though the
    // user is on Linux: moved files should stay portable.
    if is_reserved_name(&out) {
        out.push('_');
    }

    truncate_bytes(&out, COMPONENT_MAX_BYTES)
}

fn is_reserved_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or(name);
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// Truncate to at most `max` bytes on a char boundary, then re-trim trailing
/// dots/spaces the cut may have exposed.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end_matches(['.', ' ']).to_string()
}

/// Group rendered paths that collide (two tracks rendering to the same target).
/// Returns one entry per colliding path with the indices that produced it. The
/// Phase 2c mover uses this to refuse or disambiguate before moving anything.
pub fn find_collisions(paths: &[PathBuf]) -> Vec<(PathBuf, Vec<usize>)> {
    let mut seen: std::collections::HashMap<&PathBuf, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        seen.entry(p).or_default().push(i);
    }
    let mut collisions: Vec<(PathBuf, Vec<usize>)> = seen
        .into_iter()
        .filter(|(_, idx)| idx.len() > 1)
        .map(|(p, idx)| (p.clone(), idx))
        .collect();
    collisions.sort_by(|a, b| a.0.cmp(&b.0));
    collisions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> TrackFields<'static> {
        TrackFields {
            shelf_genre: Some("Electronic"),
            albumartist: Some("Boards of Canada"),
            album: Some("Geogaddi"),
            year: Some(2002),
            track_no: Some(3),
            disc_no: Some(1),
            title: Some("Music Is Math"),
            artist: Some("Boards of Canada"),
            ext: Some("flac"),
        }
    }

    fn render(f: &TrackFields) -> String {
        PathTemplate::default_music()
            .render(f)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn default_template_full_render() {
        assert_eq!(
            render(&fields()),
            "Electronic/Boards of Canada/Geogaddi (2002)/03 - Music Is Math.flac"
        );
    }

    #[test]
    fn compilation_buckets_to_various_artists() {
        let mut f = fields();
        f.albumartist = None;
        f.album = Some("Artificial Intelligence");
        f.year = Some(1992);
        f.track_no = Some(1);
        f.title = Some("I.A.O.");
        assert_eq!(
            render(&f),
            // The trailing dot in "I.A.O." is stripped as a filesystem-safety
            // rule before the extension is appended; the tag keeps the true value.
            "Electronic/Various Artists/Artificial Intelligence (1992)/01 - I.A.O.flac"
        );
    }

    #[test]
    fn missing_year_collapses_the_paren_group() {
        let mut f = fields();
        f.year = None;
        assert_eq!(
            render(&f),
            "Electronic/Boards of Canada/Geogaddi/03 - Music Is Math.flac"
        );
    }

    #[test]
    fn missing_track_collapses_the_separator() {
        let mut f = fields();
        f.track_no = None;
        assert_eq!(
            render(&f),
            "Electronic/Boards of Canada/Geogaddi (2002)/Music Is Math.flac"
        );
    }

    #[test]
    fn missing_structural_fields_fall_back_to_buckets() {
        let f = TrackFields {
            ext: Some("mp3"),
            ..Default::default()
        };
        assert_eq!(
            render(&f),
            "Unknown/Various Artists/Unknown Album/Untitled.mp3"
        );
    }

    #[test]
    fn zero_pad_width_is_honoured() {
        let mut f = fields();
        f.track_no = Some(7);
        assert!(render(&f).contains("/07 - "));
        f.track_no = Some(123);
        assert!(render(&f).contains("/123 - ")); // wider than pad: not truncated
    }

    #[test]
    fn path_separators_in_values_are_replaced() {
        let mut f = fields();
        f.artist = Some("AC/DC");
        f.albumartist = Some("AC/DC");
        f.title = Some("Hells/Bells");
        let rendered = render(&f);
        assert!(rendered.contains("/AC_DC/"), "{rendered}");
        assert!(rendered.contains("Hells_Bells"), "{rendered}");
    }

    #[test]
    fn extension_is_lowercased_and_only_on_the_leaf() {
        let mut f = fields();
        f.ext = Some("FLAC");
        let rendered = render(&f);
        assert!(rendered.ends_with("Music Is Math.flac"), "{rendered}");
    }

    #[test]
    fn no_extension_when_format_absent() {
        let mut f = fields();
        f.ext = None;
        assert!(render(&f).ends_with("03 - Music Is Math"));
    }

    #[test]
    fn reserved_device_name_is_escaped() {
        let mut f = fields();
        f.album = Some("CON");
        f.year = None;
        let rendered = render(&f);
        assert!(rendered.contains("/CON_/"), "{rendered}");
    }

    #[test]
    fn trailing_dot_and_space_are_trimmed() {
        let mut f = fields();
        f.album = Some("Album. ");
        f.year = None;
        let rendered = render(&f);
        assert!(rendered.contains("/Album/"), "{rendered}");
    }

    #[test]
    fn long_component_is_capped_per_component() {
        let long = "x".repeat(400);
        let mut f = fields();
        f.title = Some(&long);
        let rendered = PathTemplate::default_music().render(&f);
        let leaf = rendered.file_name().unwrap().to_string_lossy();
        // leaf = "03 - xxxx....flac"; the title portion is byte-capped.
        assert!(leaf.len() <= COMPONENT_MAX_BYTES + ".flac".len() + "03 - ".len());
        assert!(leaf.ends_with(".flac"));
    }

    #[test]
    fn parse_rejects_unknown_token() {
        assert!(PathTemplate::parse("{bogus}/{title}").is_err());
    }

    #[test]
    fn parse_rejects_unbalanced_brace() {
        assert!(PathTemplate::parse("{title/{album}").is_err());
        assert!(PathTemplate::parse("title}").is_err());
    }

    #[test]
    fn parse_rejects_bad_format_spec() {
        assert!(PathTemplate::parse("{track:ab}").is_err());
    }

    #[test]
    fn custom_template_with_disc() {
        let t = PathTemplate::parse("{albumartist}/{album}/{disc}-{track:02} {title}").unwrap();
        let f = fields();
        assert_eq!(
            t.render(&f).to_string_lossy(),
            "Boards of Canada/Geogaddi/1-03 Music Is Math.flac"
        );
    }

    #[test]
    fn collisions_are_detected_and_grouped() {
        let paths = vec![
            PathBuf::from("a/01 - x.flac"),
            PathBuf::from("a/01 - x.flac"),
            PathBuf::from("a/02 - y.flac"),
        ];
        let collisions = find_collisions(&paths);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].0, PathBuf::from("a/01 - x.flac"));
        assert_eq!(collisions[0].1, vec![0, 1]);
    }
}
