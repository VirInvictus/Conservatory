//! Pure metadata field projections (Phase 13c): the `(label, value)` row builder
//! the track properties inspector renders. Pure, so unit-tested directly without
//! a GTK display (the spec §16.13 CLI-testable rule).
//!
//! The Now Playing drawer once shared this home (its own `track_fields` /
//! `episode_fields` / `book_fields`), but the full-bleed spectrum rebuild dropped
//! its metadata grid, so only the inspector's projection remains here.

use std::path::Path;

use conservatory_core::db::{Album, Track};
use conservatory_core::format_size;

use crate::playqueue::fmt_secs;

/// Whether a projected row holds a filesystem path or an opaque id, i.e. a value
/// that reads better in a monospace face (the `.tech` class, Phase 13d). The one
/// source of truth for which property rows go mono.
pub(crate) fn is_tech_field(label: &str) -> bool {
    matches!(label, "Location" | "File" | "MB recording" | "MB release")
}

/// Push a non-empty `(label, value)` onto `out`; skips empty values so absent
/// fields do not render blank rows.
pub(crate) fn push(out: &mut Vec<(String, String)>, label: &str, value: impl Into<String>) {
    let value = value.into();
    if !value.is_empty() {
        out.push((label.to_string(), value));
    }
}

/// The property rows for the selected `track` plus its `album` context and the
/// resolved `artist` name; `file_size` is the on-disk size (stat'd by the
/// caller, since it is not stored). Pure, so it is unit-tested directly.
pub fn inspector_fields(
    track: &Track,
    album: Option<&Album>,
    artist: Option<&str>,
    file_size: Option<u64>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Title", track.title.clone());
    push(&mut out, "Artist", artist.unwrap_or_default());
    if let Some(al) = album {
        push(&mut out, "Album", al.title.clone());
        if let Some(y) = al.year {
            push(&mut out, "Year", y.to_string());
        }
        push(
            &mut out,
            "Genre",
            al.shelf_genre.clone().unwrap_or_default(),
        );
    }
    match (track.track_no, track.disc_no) {
        (Some(t), Some(d)) => push(&mut out, "Track", format!("{t} (disc {d})")),
        (Some(t), None) => push(&mut out, "Track", t.to_string()),
        _ => {}
    }
    if let Some(len) = track.duration {
        push(&mut out, "Duration", fmt_secs(len));
    }
    push(&mut out, "Format", track.format.clone().unwrap_or_default());
    if let Some(br) = track.bitrate.filter(|b| *b > 0) {
        push(&mut out, "Bitrate", format!("{} kbps", br / 1000));
    }
    if let Some(sr) = track.sample_rate.filter(|s| *s > 0) {
        push(
            &mut out,
            "Sample rate",
            format!("{:.1} kHz", sr as f64 / 1000.0),
        );
    }
    if let Some(size) = file_size {
        push(&mut out, "File size", format_size(size));
    }
    match (track.replaygain_track, track.replaygain_album) {
        (Some(t), Some(a)) => push(
            &mut out,
            "ReplayGain",
            format!("{t:+.2} dB track / {a:+.2} dB album"),
        ),
        (Some(t), None) => push(&mut out, "ReplayGain", format!("{t:+.2} dB track")),
        (None, Some(a)) => push(&mut out, "ReplayGain", format!("{a:+.2} dB album")),
        (None, None) => {}
    }
    if track.rating > 0 {
        push(&mut out, "Rating", "★".repeat(track.rating as usize));
    }
    if track.play_count > 0 {
        push(&mut out, "Plays", track.play_count.to_string());
    }
    if let Some(lp) = track.last_played {
        push(&mut out, "Last played", lp.date_naive().to_string());
    }
    if let Some(added) = track.added_at {
        push(&mut out, "Added", added.date_naive().to_string());
    }
    push(&mut out, "Location", track.file_path.clone());
    if let Some(id) = &track.musicbrainz_recording_id {
        push(&mut out, "MB recording", id.clone());
    }
    if let Some(id) = album.and_then(|a| a.musicbrainz_release_id.as_ref()) {
        push(&mut out, "MB release", id.clone());
    }
    let cover = album
        .and_then(|a| a.cover_path.as_deref())
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "none".to_string());
    push(&mut out, "Cover", cover);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            id: 1,
            album_id: Some(1),
            artist_id: Some(1),
            title: "Xtal".into(),
            track_no: Some(1),
            disc_no: Some(1),
            duration: Some(294.0),
            file_path: "Music/Electronic/Aphex Twin/SAW/01 - Xtal.flac".into(),
            format: Some("flac".into()),
            bitrate: Some(900_000),
            sample_rate: Some(44_100),
            replaygain_track: Some(-6.5),
            replaygain_album: Some(-6.0),
            rating: 4,
            play_count: 3,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: Some("rec-123".into()),
            added_at: None,
        }
    }

    fn album() -> Album {
        Album {
            id: 1,
            title: "Selected Ambient Works 85-92".into(),
            album_artist_id: Some(1),
            shelf_genre: Some("Electronic".into()),
            year: Some(1992),
            release_date: None,
            musicbrainz_release_id: Some("rel-456".into()),
            cover_path: Some("Music/Electronic/Aphex Twin/SAW/cover.jpg".into()),
            accent_rgb: Some(0x0033_6699),
            folder_path: "Music/Electronic/Aphex Twin/SAW".into(),
            added_at: None,
        }
    }

    #[test]
    fn inspector_fields_render_technical_detail() {
        let rows = inspector_fields(
            &track(),
            Some(&album()),
            Some("Aphex Twin"),
            Some(4_200_000),
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Title"], "Xtal");
        assert_eq!(map["Artist"], "Aphex Twin");
        assert_eq!(map["Album"], "Selected Ambient Works 85-92");
        assert_eq!(map["Year"], "1992");
        assert_eq!(map["Genre"], "Electronic");
        assert_eq!(map["Track"], "1 (disc 1)");
        assert_eq!(map["Duration"], "4:54");
        assert_eq!(map["Format"], "flac");
        assert_eq!(map["Bitrate"], "900 kbps");
        assert_eq!(map["Sample rate"], "44.1 kHz");
        assert_eq!(map["File size"], format_size(4_200_000));
        assert_eq!(map["Rating"], "★★★★");
        assert_eq!(map["Plays"], "3");
        assert!(map["ReplayGain"].contains("track"));
        assert_eq!(
            map["Location"],
            "Music/Electronic/Aphex Twin/SAW/01 - Xtal.flac"
        );
        assert_eq!(map["MB recording"], "rec-123");
        assert_eq!(map["MB release"], "rel-456");
        assert_eq!(map["Cover"], "cover.jpg");
    }

    #[test]
    fn inspector_fields_skip_absent_optional_values() {
        let mut t = track();
        t.bitrate = None;
        t.sample_rate = None;
        t.replaygain_track = None;
        t.replaygain_album = None;
        t.rating = 0;
        t.play_count = 0;
        t.musicbrainz_recording_id = None;
        let rows = inspector_fields(&t, None, None, None);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        // The present fields still render.
        assert_eq!(map["Title"], "Xtal");
        assert_eq!(map["Cover"], "none");
        // The absent ones leave no blank rows.
        for absent in [
            "Artist",
            "Album",
            "Bitrate",
            "Sample rate",
            "File size",
            "ReplayGain",
            "Rating",
            "Plays",
            "MB recording",
        ] {
            assert!(!map.contains_key(absent), "{absent} should be skipped");
        }
    }
}
