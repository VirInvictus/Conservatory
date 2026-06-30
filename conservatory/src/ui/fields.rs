//! Pure metadata field projections (Phase 13c): the `(label, value)` row builders
//! the inspector and the Now Playing drawer render. They were duplicated across
//! `inspector.rs` and `now_playing_panel.rs` (each with its own copy of `push`);
//! co-locating them here removes the duplication and keeps one home for the
//! "project a record to display rows" logic. All pure, so unit-tested directly
//! without a GTK display (the spec §16.13 CLI-testable rule).

use std::path::Path;

use conservatory_core::db::{Album, Book, Episode, NowPlaying, Show, Track};
use conservatory_core::format_size;

use crate::playqueue::fmt_secs;

/// Whether a projected row holds a filesystem path or an opaque id, i.e. a value
/// that reads better in a monospace face (the `.tech` class, Phase 13d). The one
/// source of truth for which property rows go mono, shared by both grid fills.
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

/// The drawer rows for a playing track: display fields from `np` (joined
/// title/artist/album) plus the technical detail from `track`/`album`. Pure.
pub fn track_fields(
    np: &NowPlaying,
    track: &Track,
    album: Option<&Album>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Artist", np.artist.clone().unwrap_or_default());
    push(&mut out, "Album", np.album.clone().unwrap_or_default());
    if let Some(al) = album {
        if let Some(y) = al.year {
            push(&mut out, "Year", y.to_string());
        }
        push(
            &mut out,
            "Genre",
            al.shelf_genre.clone().unwrap_or_default(),
        );
    }
    if let Some(len) = np.length {
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
    push(&mut out, "File", track.file_path.clone());
    out
}

/// The drawer rows for a playing episode: show / date / runtime / source plus
/// the show notes. `streaming` reflects whether it is playing from the network.
/// Pure.
pub fn episode_fields(
    np: &NowPlaying,
    ep: &Episode,
    show: Option<&Show>,
    streaming: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Show", np.artist.clone().unwrap_or_default());
    if let Some(d) = ep.pub_date {
        push(&mut out, "Published", d.format("%Y-%m-%d").to_string());
    }
    if let Some(len) = np.length {
        push(&mut out, "Duration", fmt_secs(len));
    }
    push(
        &mut out,
        "Source",
        if streaming { "Streaming" } else { "Downloaded" },
    );
    if let Some(bytes) = ep.file_size.filter(|b| *b > 0) {
        push(
            &mut out,
            "Size",
            format!("{:.1} MB", bytes as f64 / 1_048_576.0),
        );
    }
    match (ep.season, ep.episode_number) {
        (Some(s), Some(e)) => push(&mut out, "Season/Episode", format!("S{s} E{e}")),
        (None, Some(e)) => push(&mut out, "Episode", e.to_string()),
        _ => {}
    }
    if let Some(t) = ep.episode_type.as_deref().filter(|t| *t != "full") {
        push(&mut out, "Type", t);
    }
    if let Some(sh) = show {
        push(&mut out, "Author", sh.author.clone().unwrap_or_default());
    }
    push(
        &mut out,
        "Notes",
        ep.description.clone().unwrap_or_default(),
    );
    out
}

/// The Now Playing field projection for an audiobook (Phase 7c-iii), the book
/// twin of [`track_fields`] / [`episode_fields`]. `np` carries the title, the
/// first author (`artist`), the series (`album`), and the total duration;
/// `narrators` and the chapter count / single-file flag come from the book's
/// own reads. Empty optionals are omitted. Pure (testable without the DB).
pub fn book_fields(
    np: &NowPlaying,
    book: &Book,
    narrators: &[String],
    chapter_count: usize,
    single_file: bool,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    push(&mut out, "Author", np.artist.clone().unwrap_or_default());
    if !narrators.is_empty() {
        push(&mut out, "Narrator", narrators.join(", "));
    }
    if let Some(series) = np.album.clone().filter(|s| !s.is_empty()) {
        let value = match book.series_sequence {
            Some(seq) => format!("{series} (Book {})", fmt_sequence(seq)),
            None => series,
        };
        push(&mut out, "Series", value);
    }
    if let Some(sub) = book.subtitle.clone().filter(|s| !s.is_empty()) {
        push(&mut out, "Subtitle", sub);
    }
    if let Some(y) = book.year {
        push(&mut out, "Year", y.to_string());
    }
    if let Some(pub_) = book.publisher.clone().filter(|s| !s.is_empty()) {
        push(&mut out, "Publisher", pub_);
    }
    if let Some(len) = np.length {
        push(&mut out, "Duration", fmt_secs(len));
    }
    if chapter_count > 0 {
        push(&mut out, "Chapters", chapter_count.to_string());
    }
    push(
        &mut out,
        "Format",
        if single_file {
            "Single file"
        } else {
            "Multi-file"
        },
    );
    if let Some(desc) = book.description.clone().filter(|s| !s.is_empty()) {
        push(&mut out, "Description", desc);
    }
    out
}

/// A decimal series sequence trimmed to its shortest exact form ("1" not "1.0",
/// "1.5" kept), matching the path-template render (spec §5.7).
fn fmt_sequence(seq: f64) -> String {
    if seq.fract() == 0.0 {
        format!("{}", seq as i64)
    } else {
        format!("{seq}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

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

    fn np(title: &str, artist: &str, album: Option<&str>) -> NowPlaying {
        NowPlaying {
            title: title.into(),
            artist: Some(artist.into()),
            album: album.map(str::to_string),
            length: Some(294.0),
            album_cover_path: None,
            album_accent_rgb: None,
        }
    }

    fn book(series_seq: Option<f64>) -> Book {
        Book {
            id: 7,
            title: "The Way of Kings".into(),
            subtitle: Some("Book One".into()),
            series_id: Some(1),
            series_sequence: series_seq,
            year: Some(2010),
            publisher: Some("Macmillan Audio".into()),
            isbn: None,
            asin: None,
            description: Some("Epic.".into()),
            language: Some("en".into()),
            shelf_genre: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "Audiobooks/Brandon Sanderson/...".into(),
            rating: 0,
            starred: false,
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

    #[test]
    fn track_fields_render_technical_detail() {
        let al = Album {
            id: 1,
            title: "Selected Ambient Works 85-92".into(),
            album_artist_id: Some(1),
            shelf_genre: Some("Electronic".into()),
            year: Some(1992),
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: None,
            folder_path: "Music/Electronic/Aphex Twin/...".into(),
            added_at: None,
        };
        let rows = track_fields(
            &np("Xtal", "Aphex Twin", Some("SAW 85-92")),
            &track(),
            Some(&al),
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Artist"], "Aphex Twin");
        assert_eq!(map["Year"], "1992");
        assert_eq!(map["Genre"], "Electronic");
        assert_eq!(map["Format"], "flac");
        assert_eq!(map["Bitrate"], "900 kbps");
        assert_eq!(map["Sample rate"], "44.1 kHz");
        assert_eq!(map["Rating"], "★★★★");
        assert_eq!(map["Plays"], "3");
        assert!(map["ReplayGain"].contains("track"));
        assert!(map["File"].ends_with("Xtal.flac"));
    }

    #[test]
    fn episode_fields_mark_streaming_and_skip_empties() {
        let ep = Episode {
            id: 5,
            show_id: 9,
            guid: "g".into(),
            title: "180: ...".into(),
            description: Some("Notes here.".into()),
            pub_date: Some(Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap()),
            duration: Some(5162),
            file_size: Some(73_400_320),
            audio_url: Some("https://cdn/ep.mp3".into()),
            audio_path: None,
            folder_path: "Podcasts/cortex/...".into(),
            mime_type: Some("audio/mpeg".into()),
            season: None,
            episode_number: Some(180),
            episode_type: Some("full".into()),
        };
        let rows = episode_fields(&np("180: ...", "Cortex", None), &ep, None, true);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Show"], "Cortex");
        assert_eq!(map["Published"], "2026-06-22");
        assert_eq!(map["Source"], "Streaming");
        assert_eq!(map["Episode"], "180");
        assert_eq!(map["Notes"], "Notes here.");
        // "full" type and absent season are skipped.
        assert!(!map.contains_key("Type"));
        assert!(!map.contains_key("Season/Episode"));
    }

    #[test]
    fn episode_fields_downloaded_label() {
        let ep = Episode {
            id: 6,
            show_id: 9,
            guid: "g".into(),
            title: "t".into(),
            description: None,
            pub_date: None,
            duration: None,
            file_size: None,
            audio_url: None,
            audio_path: Some("Podcasts/x/y.mp3".into()),
            folder_path: "Podcasts/x".into(),
            mime_type: None,
            season: Some(2),
            episode_number: Some(4),
            episode_type: Some("bonus".into()),
        };
        let rows = episode_fields(&np("t", "Show", None), &ep, None, false);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Source"], "Downloaded");
        assert_eq!(map["Season/Episode"], "S2 E4");
        assert_eq!(map["Type"], "bonus");
        assert!(!map.contains_key("Notes")); // empty description skipped
    }

    #[test]
    fn book_fields_render_author_series_and_format() {
        let mut np = np(
            "The Way of Kings",
            "Brandon Sanderson",
            Some("The Stormlight Archive"),
        );
        np.length = Some(2730.0);
        let rows = book_fields(&np, &book(Some(1.0)), &["Kate Reading".into()], 45, false);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Author"], "Brandon Sanderson");
        assert_eq!(map["Narrator"], "Kate Reading");
        // The integral sequence renders without a trailing ".0".
        assert_eq!(map["Series"], "The Stormlight Archive (Book 1)");
        assert_eq!(map["Subtitle"], "Book One");
        assert_eq!(map["Year"], "2010");
        assert_eq!(map["Publisher"], "Macmillan Audio");
        assert_eq!(map["Chapters"], "45");
        assert_eq!(map["Format"], "Multi-file");
        assert_eq!(map["Description"], "Epic.");
    }

    #[test]
    fn book_fields_single_file_and_decimal_sequence() {
        let np = np(
            "Edgedancer",
            "Brandon Sanderson",
            Some("The Stormlight Archive"),
        );
        // A decimal sequence keeps its fraction; an M4B is a single file.
        let rows = book_fields(&np, &book(Some(2.5)), &[], 0, true);
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Series"], "The Stormlight Archive (Book 2.5)");
        assert_eq!(map["Format"], "Single file");
        // No narrators / no chapters → those rows are omitted.
        assert!(!map.contains_key("Narrator"));
        assert!(!map.contains_key("Chapters"));
    }
}
