//! Conservatory audiobook plugin.
//!
//! A compile-time plugin: a feature-gated workspace crate, compiled into the
//! binaries when their `audiobooks` feature is on (the default; spec §2.2).
//!
//! The plugin boundary is code and dependencies, not the database: the book
//! schema lives in `conservatory-core`'s single migration ledger, and the
//! unified queue, libmpv host, file mover, path-template engine, and the
//! spoken-word profile shared with podcasts are core. A book is one queue
//! entry; chapters are intra-item navigation (spec §6.1).
//!
//! **Phase 7a-ii (this layer): the reader.** [`read_book`] turns a folder or a
//! single audio file into a [`BookDraft`]: the headless, pre-database view the
//! Phase 7a-iii import pipeline resolves into `books` / `book_people` / `series`
//! / `book_chapters` rows. It is the audiobook analogue of
//! `conservatory-core`'s `read_track` + import resolve. No DB writes, no file
//! moves, no covers/accent (all 7a-iii).
//!
//! Metadata comes from three sources, merged by precedence
//! **sidecar > embedded tags > folder structure**: the explicit
//! Audiobookshelf sidecars win, the embedded tags are the common case, and the
//! on-disk layout is a last-resort fallback. Chapters come from embedded M4B
//! markers (via `ffprobe`), a one-file-per-chapter folder, or a whole-file
//! single chapter.

pub mod chapters;
pub mod edit;
pub mod error;
pub mod ffprobe;
pub mod folder;
pub mod import;
pub mod reorg;
pub mod sidecar;
pub mod tags;

use std::path::{Path, PathBuf};

pub use chapters::ChapterDraft;
pub use edit::{BookEdit, SeriesEdit};
pub use error::{ReadError, Result};
pub use import::{BookImportOptions, BookImportReport, import_book};
pub use reorg::{BookReorgPlan, apply_book_edit, apply_book_reorg, plan_book_reorg};

/// A person (author or narrator) with a Calibre-style sort name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonDraft {
    pub name: String,
    pub sort_name: String,
}

impl PersonDraft {
    /// Build a person from a display name, deriving the sort name.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.trim().to_string(),
            sort_name: person_sort_name(name),
        }
    }
}

/// Everything read from a book before it is resolved into the database. Every
/// field is best-effort; an untitled, untagged folder still yields a draft with
/// its chapters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookDraft {
    /// The book's source folder (or the file's parent), for the importer.
    pub source_dir: PathBuf,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub authors: Vec<PersonDraft>,
    pub narrators: Vec<PersonDraft>,
    pub series: Option<String>,
    pub series_sequence: Option<f64>,
    pub year: Option<i32>,
    pub publisher: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    /// Embedded or sidecar cover bytes; the accent is computed at import (7a-iii).
    pub cover: Option<Vec<u8>>,
    pub chapters: Vec<ChapterDraft>,
}

/// Read a folder (or a single audio file) into a [`BookDraft`].
///
/// The folder is treated as one book: its audio files become the chapters (a
/// multi-file book) or the single file's embedded markers do (an M4B). Metadata
/// merges sidecar over tags over folder inference.
pub fn read_book(path: &Path) -> Result<BookDraft> {
    let files = collect_audio(path)?;
    if files.is_empty() {
        return Err(ReadError::NoAudio(path.display().to_string()));
    }
    let book_dir: PathBuf = if path.is_dir() {
        path.to_path_buf()
    } else {
        files[0]
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };

    // The three sources.
    let tags = tags::read_book_tags(&files[0])?;
    let sidecar = sidecar::read_sidecars(&book_dir);
    let folder = folder::infer(&book_dir);
    let folder_author: Vec<String> = folder.author.into_iter().collect();

    // Merge by precedence: sidecar > tags > folder (per field).
    let title = sidecar.title.or(tags.title).or(folder.title);
    let series = sidecar.series.or(tags.series).or(folder.series);
    let series_sequence = sidecar
        .series_sequence
        .or(tags.series_sequence)
        .or(folder.sequence);

    let authors = first_nonempty(vec![sidecar.authors, tags.authors, folder_author]);
    let narrators = first_nonempty(vec![sidecar.narrators, tags.narrators]);

    let cover = tags.cover.or_else(|| sidecar_cover(&book_dir));

    let chapters = chapters::resolve_chapters(&files, title.as_deref())?;

    Ok(BookDraft {
        source_dir: book_dir,
        title,
        subtitle: sidecar.subtitle.or(tags.subtitle),
        authors: people(authors),
        narrators: people(narrators),
        series,
        series_sequence,
        year: sidecar.year.or(tags.year),
        publisher: sidecar.publisher.or(tags.publisher),
        isbn: sidecar.isbn.or(tags.isbn),
        asin: sidecar.asin.or(tags.asin),
        description: sidecar.description.or(tags.description),
        language: sidecar.language.or(tags.language),
        cover,
        chapters,
    })
}

/// The book's audio files, sorted by path. A single file yields a one-element
/// list; a folder is walked recursively. Audiobook extensions add `m4b` to the
/// music set (core's scanner omits it).
fn collect_audio(path: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if path.is_file() {
        if is_audiobook_audio(path) {
            out.push(path.to_path_buf());
        }
        return Ok(out);
    }
    walk(path, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if is_audiobook_audio(&path) {
            out.push(path);
        }
    }
    Ok(())
}

const AUDIO_EXTS: &[&str] = &["m4b", "m4a", "mp3", "opus", "ogg", "flac", "aac", "wav"];

fn is_audiobook_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// A sibling `cover.jpg`/`cover.png` (Audiobookshelf convention), read as bytes.
fn sidecar_cover(dir: &Path) -> Option<Vec<u8>> {
    const NAMES: &[&str] = &["cover.jpg", "cover.jpeg", "cover.png", "folder.jpg"];
    for name in NAMES {
        if let Ok(bytes) = std::fs::read(dir.join(name)) {
            return Some(bytes);
        }
    }
    None
}

fn people(names: Vec<String>) -> Vec<PersonDraft> {
    names.iter().map(|n| PersonDraft::new(n)).collect()
}

/// The first non-empty list among the precedence-ordered sources.
fn first_nonempty(sources: Vec<Vec<String>>) -> Vec<String> {
    sources
        .into_iter()
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// Derive a Calibre-style person sort name ("Patrick Rothfuss" -> "Rothfuss,
/// Patrick"). This is *not* `conservatory-core`'s `derive_sort_name`, which moves
/// a leading article for band/album names; book people sort last-name-first
/// (spec §4.5, the `book_people.sort_name` "Sanderson, Brandon" example). A name
/// already in "Last, First" form, or a single token, is left as-is.
pub fn person_sort_name(name: &str) -> String {
    let name = name.trim();
    if name.contains(',') {
        return name.to_string();
    }
    match name.rsplit_once(char::is_whitespace) {
        Some((rest, last)) if !rest.trim().is_empty() && !last.is_empty() => {
            format!("{last}, {}", rest.trim())
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_sort_name_last_first() {
        assert_eq!(person_sort_name("Patrick Rothfuss"), "Rothfuss, Patrick");
        assert_eq!(person_sort_name("Neil Gaiman"), "Gaiman, Neil");
        assert_eq!(person_sort_name("Nick Podehl"), "Podehl, Nick");
        // Already sorted, or a single token, is untouched.
        assert_eq!(person_sort_name("Sanderson, Brandon"), "Sanderson, Brandon");
        assert_eq!(person_sort_name("Madonna"), "Madonna");
        // A three-part name puts the last token first.
        assert_eq!(person_sort_name("Ursula K Le Guin"), "Guin, Ursula K Le");
    }

    #[test]
    fn first_nonempty_picks_precedence() {
        assert_eq!(
            first_nonempty(vec![vec![], vec!["b".into()], vec!["c".into()]]),
            vec!["b".to_string()]
        );
        assert!(first_nonempty(vec![vec![], vec![]]).is_empty());
    }
}
