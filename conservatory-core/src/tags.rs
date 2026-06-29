//! Embedded-tag reader (spec §7.1, roadmap Phase 1c).
//!
//! Reads one audio file into a [`TrackDraft`]: the headless, pre-database
//! representation the Phase 2 import pipeline resolves into artists/albums/tracks.
//! `lofty` (signed off at 1c, ATTRIBUTIONS.md) provides the broad-format read.
//!
//! Raw multi-value genres are kept **verbatim** here (the §5.2 decoupling).
//! Splitting, case-folding, and aliasing into a single `shelf_genre` is Phase
//! 2b's resolver, never the reader's job.

use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::file::FileType;
use lofty::picture::Picture;
use lofty::prelude::{Accessor, AudioFile, ItemKey, TaggedFileExt};
use lofty::tag::{ItemValue, Tag, TagItem, TagType};

use crate::errors::Result;

/// Cover art extracted from a file's embedded tags, decoded later for the accent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedCover {
    pub data: Vec<u8>,
    pub mime: Option<String>,
}

/// Everything read from a single audio file before it is resolved into the DB.
///
/// Mirrors the eventual `Track`/`Album` split loosely: track-level fields plus
/// the album-level hints (album, album artist, year, cover) the resolver needs.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackDraft {
    pub source_path: PathBuf,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    /// Embedded sort-name tags, when present (`ARTISTSORT` / `ALBUMARTISTSORT`).
    /// The resolver prefers these over deriving a sort name from the display name.
    pub artist_sort: Option<String>,
    pub album_artist_sort: Option<String>,
    pub album: Option<String>,
    pub track_no: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_no: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<i32>,
    /// Raw multi-value genres, exactly as stored (the §5.2 decoupling).
    pub genres: Vec<String>,
    pub replaygain_track: Option<f64>,
    pub replaygain_album: Option<f64>,
    pub format: Option<String>,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub duration: Option<f64>,
    pub cover: Option<EmbeddedCover>,
}

/// Read a file's embedded tags and audio properties into a [`TrackDraft`].
///
/// An untagged but decodable file still yields a draft: the tag-derived fields
/// are `None`/empty while format, duration, bitrate, and sample rate come from
/// the audio properties.
pub fn read_track(path: &Path) -> Result<TrackDraft> {
    let tagged = lofty::read_from_path(path)?;
    let props = tagged.properties();
    // primary_tag is the format's canonical tag (e.g. ID3v2 over ID3v1); fall
    // back to whatever tag exists for oddly-tagged files.
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let duration = props.duration().as_secs_f64();
    let mut draft = TrackDraft {
        source_path: path.to_path_buf(),
        title: None,
        artist: None,
        album_artist: None,
        artist_sort: None,
        album_artist_sort: None,
        album: None,
        track_no: None,
        track_total: None,
        disc_no: None,
        disc_total: None,
        year: None,
        genres: Vec::new(),
        replaygain_track: None,
        replaygain_album: None,
        format: Some(format_label(tagged.file_type()).to_string()),
        bitrate: props.overall_bitrate(),
        sample_rate: props.sample_rate(),
        // 0.0 means lofty could not determine a duration; treat it as absent.
        duration: (duration > 0.0).then_some(duration),
        cover: None,
    };

    if let Some(tag) = tag {
        fill_from_tag(&mut draft, tag);
    }

    Ok(draft)
}

fn fill_from_tag(draft: &mut TrackDraft, tag: &Tag) {
    draft.title = tag.title().map(|c| c.to_string());
    draft.artist = tag.artist().map(|c| c.to_string());
    draft.album_artist = tag.get_string(&ItemKey::AlbumArtist).map(str::to_string);
    draft.artist_sort = tag
        .get_string(&ItemKey::TrackArtistSortOrder)
        .map(str::to_string);
    draft.album_artist_sort = tag
        .get_string(&ItemKey::AlbumArtistSortOrder)
        .map(str::to_string);
    draft.album = tag.album().map(|c| c.to_string());
    draft.track_no = tag.track();
    draft.track_total = tag.track_total();
    draft.disc_no = tag.disk();
    draft.disc_total = tag.disk_total();
    draft.year = tag.year().map(|y| y as i32);
    draft.genres = tag
        .get_strings(&ItemKey::Genre)
        .map(str::to_string)
        .collect();
    draft.replaygain_track = tag
        .get_string(&ItemKey::ReplayGainTrackGain)
        .and_then(parse_replaygain);
    draft.replaygain_album = tag
        .get_string(&ItemKey::ReplayGainAlbumGain)
        .and_then(parse_replaygain);
    draft.cover = tag.pictures().first().map(picture_to_cover);
}

/// Best-effort: does this file carry Opus `R128_TRACK_GAIN` / `R128_ALBUM_GAIN`
/// tags? Returns `(has_track, has_album)`. lofty surfaces the standard
/// `REPLAYGAIN_*` keys through [`ItemKey::ReplayGainTrackGain`] but not the
/// R128 convention, so the Phase 8c ReplayGain audit queries the raw keys here
/// to avoid falsely flagging an R128-only Opus file as gain-less. Any read
/// error yields `(false, false)`.
pub fn read_r128_presence(path: &Path) -> (bool, bool) {
    let Ok(tagged) = lofty::read_from_path(path) else {
        return (false, false);
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return (false, false);
    };
    let has = |key: &str| tag.get_string(&ItemKey::Unknown(key.to_string())).is_some();
    (has("R128_TRACK_GAIN"), has("R128_ALBUM_GAIN"))
}

fn picture_to_cover(pic: &Picture) -> EmbeddedCover {
    EmbeddedCover {
        data: pic.data().to_vec(),
        mime: pic.mime_type().map(|m| m.as_str().to_string()),
    }
}

/// The curated descriptive fields written back into a file's tags (Phase 5b,
/// spec §5.5). The rebuildable layer only: rating, shelf genre, play counts, and
/// starred stay DB-only (§5.6) and are never embedded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagWrite {
    pub title: String,
    pub track_artist: Option<String>,
    pub track_artist_sort: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub album_artist_sort: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genres: Vec<String>,
}

/// Write the curated DB metadata into a file's embedded tags (spec §5.5).
///
/// Writes the format's canonical (primary) tag authoritatively, creating it if
/// the file had none. The legacy ID3v1 is dropped so the primary tag is the
/// single source. Idempotent; re-derivable from the DB, so there is no undo
/// journal (re-running fixes any mistake).
///
/// Caveat: a stray APEv2 block on an MPEG file is *not* removed here, because
/// lofty does not write APE on MPEG. Reliable APE stripping needs byte-level
/// surgery (the Lattice `apestrip` technique) and is deferred to a later phase.
pub fn write_track_tags(path: &Path, w: &TagWrite) -> Result<()> {
    let mut tagged = lofty::read_from_path(path)?;
    let primary = tagged.file_type().primary_tag_type();

    // Drop the legacy ID3v1 (lofty does manage it on save) so the canonical
    // primary tag is the single source for the fields we are writing.
    if primary != TagType::Id3v1 {
        tagged.remove(TagType::Id3v1);
    }
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(primary));
    }
    let tag = tagged
        .primary_tag_mut()
        .expect("primary tag inserted above");

    set_text(tag, ItemKey::TrackTitle, Some(w.title.as_str()));
    set_text(tag, ItemKey::TrackArtist, w.track_artist.as_deref());
    set_text(
        tag,
        ItemKey::TrackArtistSortOrder,
        w.track_artist_sort.as_deref(),
    );
    set_text(tag, ItemKey::AlbumTitle, w.album.as_deref());
    set_text(tag, ItemKey::AlbumArtist, w.album_artist.as_deref());
    set_text(
        tag,
        ItemKey::AlbumArtistSortOrder,
        w.album_artist_sort.as_deref(),
    );

    match w.year {
        Some(y) if y > 0 => tag.set_year(y as u32),
        _ => tag.remove_year(),
    }
    match w.track_no {
        Some(n) => tag.set_track(n),
        None => tag.remove_track(),
    }
    match w.disc_no {
        Some(n) => tag.set_disk(n),
        None => tag.remove_disk(),
    }

    // Genres are multi-value: clear the key, then push each (the §5.2 raw side).
    let _ = tag.take(&ItemKey::Genre);
    for g in &w.genres {
        tag.push(TagItem::new(ItemKey::Genre, ItemValue::Text(g.clone())));
    }

    tagged.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// Set a text item, or clear the key when the value is `None`.
fn set_text(tag: &mut Tag, key: ItemKey, value: Option<&str>) {
    match value {
        Some(v) => {
            tag.insert_text(key, v.to_string());
        }
        None => {
            let _ = tag.take(&key);
        }
    }
}

/// A short, stable format label for the draft / DB `format` column.
fn format_label(ft: FileType) -> &'static str {
    match ft {
        FileType::Flac => "flac",
        FileType::Mpeg => "mp3",
        FileType::Mp4 => "m4a",
        FileType::Opus => "opus",
        FileType::Vorbis => "ogg",
        FileType::Speex => "spx",
        FileType::Wav => "wav",
        FileType::Aac => "aac",
        FileType::Ape => "ape",
        FileType::Mpc => "mpc",
        FileType::WavPack => "wv",
        _ => "unknown",
    }
}

/// ReplayGain is stored as a string like `-7.50 dB`. Parse the leading signed
/// decimal and ignore any unit suffix (with or without a space).
fn parse_replaygain(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(s.len());
    s[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_parses_with_and_without_unit() {
        assert_eq!(parse_replaygain("-7.50 dB"), Some(-7.5));
        assert_eq!(parse_replaygain("-7.5dB"), Some(-7.5));
        assert_eq!(parse_replaygain("+3.2 dB"), Some(3.2));
        assert_eq!(parse_replaygain("0.00"), Some(0.0));
        assert_eq!(parse_replaygain(""), None);
        assert_eq!(parse_replaygain("loud"), None);
    }

    #[test]
    fn format_labels_cover_the_target_formats() {
        assert_eq!(format_label(FileType::Flac), "flac");
        assert_eq!(format_label(FileType::Mpeg), "mp3");
        assert_eq!(format_label(FileType::Mp4), "m4a");
        assert_eq!(format_label(FileType::Opus), "opus");
    }
}
