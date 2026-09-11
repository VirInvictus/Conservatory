//! Pure metadata field projections (Phase 13c): the `(label, value)` row builder
//! the track properties inspector renders. Pure, so unit-tested directly without
//! a GTK display (the spec §16.13 CLI-testable rule).
//!
//! The Now Playing drawer once shared this home (its own `track_fields` /
//! `episode_fields` / `book_fields`), but the full-bleed spectrum rebuild dropped
//! its metadata grid, so only the inspector's projection remains here.

use std::path::Path;

use conservatory_core::db::{Album, Track, TrackCreditRow};
use conservatory_core::{Assignment, format_size, parse_assignment};

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

/// Collect the bulk-edit dialog's per-field state (key, ticked, entered text)
/// into parsed assignments (Phase 16.5a). Unticked fields are skipped;
/// ticked-but-empty fields clear (the 16c clear path: year / shelf genre go
/// NULL, genres empty, rating 0; identity fields report an error); every
/// parse failure is reported so the caller can reject the whole set rather
/// than apply a partly-valid edit. Pure, so unit-tested directly.
pub(crate) fn collect_assignments(
    fields: &[(String, bool, String)],
) -> (Vec<Assignment>, Vec<String>) {
    let mut assignments = Vec::new();
    let mut errors = Vec::new();
    for (key, ticked, value) in fields {
        if !*ticked {
            continue;
        }
        match parse_assignment(&format!("{key}={value}")) {
            Ok(a) => assignments.push(a),
            Err(e) => errors.push(e.to_string()),
        }
    }
    (assignments, errors)
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

/// The shared value across a selection, or `"multiple values"` when they
/// differ (the bulk-edit collapse, the same wording). All-`None` agrees on
/// `None` (the caller skips the row); mixed `None`/`Some` is a difference.
fn commons(values: impl Iterator<Item = Option<String>>) -> Option<String> {
    let mut first: Option<Option<String>> = None;
    for v in values {
        match &first {
            None => first = Some(v),
            Some(f) if *f != v => return Some("multiple values".to_string()),
            _ => {}
        }
    }
    first.flatten()
}

/// The aggregate property rows over a multi-selection (the :778 follow-on;
/// the channels half needs a schema decision and stays out of scope). The
/// rule is the bulk-edit commons (shared value, else "multiple values"),
/// with sums only where a sum is meaningful: duration, file size, plays.
/// The per-row `artist` name is resolved by the caller; the title, track /
/// disc numbers, and location are per-track identity and do not aggregate,
/// so they are left out. Pure, so it is unit-tested directly.
pub fn inspector_aggregate(
    rows: &[(&Track, Option<&Album>, Option<&str>)],
    total_size: Option<u64>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let artist = commons(rows.iter().map(|(_, _, a)| a.map(|s| s.to_string())));
    push(&mut out, "Artist", artist.unwrap_or_default());
    let album = commons(rows.iter().map(|(_, al, _)| al.map(|a| a.title.clone())));
    push(&mut out, "Album", album.unwrap_or_default());
    let year = commons(
        rows.iter()
            .map(|(_, al, _)| al.and_then(|a| a.year).map(|y| y.to_string())),
    );
    if let Some(y) = year {
        push(&mut out, "Year", y);
    }
    let genre = commons(
        rows.iter()
            .map(|(_, al, _)| al.and_then(|a| a.shelf_genre.clone())),
    );
    push(&mut out, "Genre", genre.unwrap_or_default());
    let total: f64 = rows.iter().filter_map(|(t, _, _)| t.duration).sum();
    if total > 0.0 {
        push(&mut out, "Duration", fmt_secs(total));
    }
    let format = commons(rows.iter().map(|(t, _, _)| t.format.clone()));
    push(&mut out, "Format", format.unwrap_or_default());
    let bitrate = commons(rows.iter().map(|(t, _, _)| {
        t.bitrate
            .filter(|b| *b > 0)
            .map(|b| format!("{} kbps", b / 1000))
    }));
    if let Some(br) = bitrate {
        push(&mut out, "Bitrate", br);
    }
    let sample_rate = commons(rows.iter().map(|(t, _, _)| {
        t.sample_rate
            .filter(|s| *s > 0)
            .map(|s| format!("{:.1} kHz", s as f64 / 1000.0))
    }));
    if let Some(sr) = sample_rate {
        push(&mut out, "Sample rate", sr);
    }
    if let Some(size) = total_size {
        push(&mut out, "File size", format_size(size));
    }
    let rg = commons(
        rows.iter()
            .map(|(t, _, _)| match (t.replaygain_track, t.replaygain_album) {
                (Some(t), Some(a)) => Some(format!("{t:+.2} dB track / {a:+.2} dB album")),
                (Some(t), None) => Some(format!("{t:+.2} dB track")),
                (None, Some(a)) => Some(format!("{a:+.2} dB album")),
                (None, None) => None,
            }),
    );
    if let Some(rg) = rg {
        push(&mut out, "ReplayGain", rg);
    }
    let rating = commons(
        rows.iter()
            .map(|(t, _, _)| (t.rating > 0).then(|| "★".repeat(t.rating as usize))),
    );
    if let Some(r) = rating {
        push(&mut out, "Rating", r);
    }
    let plays: u32 = rows.iter().map(|(t, _, _)| t.play_count).sum();
    if plays > 0 {
        push(&mut out, "Plays", plays.to_string());
    }
    let last_played = commons(
        rows.iter()
            .map(|(t, _, _)| t.last_played.map(|lp| lp.date_naive().to_string())),
    );
    if let Some(lp) = last_played {
        push(&mut out, "Last played", lp);
    }
    let added = commons(
        rows.iter()
            .map(|(t, _, _)| t.added_at.map(|a| a.date_naive().to_string())),
    );
    if let Some(added) = added {
        push(&mut out, "Added", added);
    }
    let cover = commons(rows.iter().map(|(_, al, _)| {
        al.and_then(|a| a.cover_path.as_deref())
            .map(Path::new)
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
    }));
    if let Some(cover) = cover {
        push(&mut out, "Cover", cover);
    }
    out
}

/// The credits section of the inspector (19b-iii): one row per role, names
/// joined in the read's role-then-sort order. Empty when the track has none,
/// consistent with the other absent-field skips. Unknown role tokens (a future
/// version wrote them) show raw rather than being dropped.
pub fn credit_fields(credits: &[TrackCreditRow]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in credits {
        match out.last_mut() {
            // Same role as the previous row: append to its name list.
            Some((label, names)) if label.as_str() == c.role.as_str() => {
                names.push_str(", ");
                names.push_str(&c.name);
            }
            _ => out.push((c.role.as_str().to_string(), c.name.clone())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use conservatory_core::Field;

    #[test]
    fn collect_assignments_skips_unticked_fields_and_clears_empty_ones() {
        let fields = vec![
            ("title".to_string(), true, "Xtal".to_string()),
            ("album".to_string(), false, "ignored".to_string()),
            // Ticked-but-empty is the 16c clear path: year clears (empty
            // assignment), rating clears to 0.
            ("year".to_string(), true, "   ".to_string()),
            ("rating".to_string(), true, String::new()),
        ];
        let (assignments, errors) = collect_assignments(&fields);
        assert!(errors.is_empty());
        assert_eq!(assignments.len(), 3);
        assert_eq!(assignments[0].value, "Xtal");
        assert_eq!(assignments[1].field, Field::Year);
        assert!(assignments[1].value.trim().is_empty());
        assert_eq!(assignments[2].field, Field::Rating);
    }

    #[test]
    fn collect_assignments_rejects_clearing_identity_fields() {
        let fields = vec![("title".to_string(), true, "  ".to_string())];
        let (assignments, errors) = collect_assignments(&fields);
        assert!(assignments.is_empty());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("cannot be cleared"));
    }

    #[test]
    fn collect_assignments_reports_every_parse_failure() {
        let fields = vec![
            ("year".to_string(), true, "abc".to_string()),
            ("rating".to_string(), true, "9".to_string()),
            ("title".to_string(), true, "kept".to_string()),
        ];
        let (assignments, errors) = collect_assignments(&fields);
        // The valid field still parses; the caller rejects the whole set on
        // any error, so both failures must be reported.
        assert_eq!(assignments.len(), 1);
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("year"));
        assert!(errors[1].contains("rating"));
    }

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

    fn credit_row(role: &str, name: &str) -> TrackCreditRow {
        TrackCreditRow {
            role: role.into(),
            name: name.into(),
            sort_name: String::new(),
        }
    }

    #[test]
    fn credit_fields_group_by_role_in_read_order() {
        // The read delivers role-then-sort order; consecutive same-role rows
        // collapse into one comma-joined value.
        let rows = credit_fields(&[
            credit_row("Composer", "Gavin Bryars"),
            credit_row("Composer", "Biosphere"),
            credit_row("Producer", "Aphex Twin"),
        ]);
        assert_eq!(
            rows,
            vec![
                (
                    "Composer".to_string(),
                    "Gavin Bryars, Biosphere".to_string()
                ),
                ("Producer".to_string(), "Aphex Twin".to_string()),
            ]
        );
        assert!(credit_fields(&[]).is_empty());
    }

    #[test]
    fn aggregate_shows_commons_and_sums() {
        let mut t2 = track();
        t2.id = 2;
        t2.title = "Hedges".into();
        t2.duration = Some(100.0);
        t2.play_count = 4;
        let rows = inspector_aggregate(
            &[
                (&track(), Some(&album()), Some("Aphex Twin")),
                (&t2, Some(&album()), Some("Aphex Twin")),
            ],
            Some(6_000_000),
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        // Shared values render once; sums total; per-track identity is absent.
        assert_eq!(map["Artist"], "Aphex Twin");
        assert_eq!(map["Album"], "Selected Ambient Works 85-92");
        assert_eq!(map["Year"], "1992");
        assert_eq!(map["Duration"], fmt_secs(394.0));
        assert_eq!(map["File size"], format_size(6_000_000));
        assert_eq!(map["Rating"], "★★★★");
        assert_eq!(map["Plays"], "7");
        assert_eq!(map["Format"], "flac");
        assert!(!map.contains_key("Title"), "titles do not aggregate");
        assert!(!map.contains_key("Location"), "locations do not aggregate");
    }

    #[test]
    fn aggregate_marks_differing_values() {
        let mut t2 = track();
        t2.id = 2;
        t2.rating = 2;
        let mut al2 = album();
        al2.year = Some(1994);
        al2.cover_path = Some("Music/Electronic/Aphex Twin/SAW II/folder.jpg".into());
        let rows = inspector_aggregate(
            &[
                (&track(), Some(&album()), Some("Aphex Twin")),
                (&t2, Some(&al2), Some("Aphex Twin")),
            ],
            None,
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert_eq!(map["Year"], "multiple values");
        assert_eq!(map["Rating"], "multiple values");
        // Differing albums: the cover row reads as differing, no accent either.
        assert_eq!(map["Cover"], "multiple values");
    }

    #[test]
    fn aggregate_all_unrated_skips_the_rating_row() {
        let mut t1 = track();
        t1.rating = 0;
        t1.play_count = 0;
        let mut t2 = track();
        t2.id = 2;
        t2.rating = 0;
        t2.play_count = 0;
        let rows = inspector_aggregate(
            &[(&t1, Some(&album()), None), (&t2, Some(&album()), None)],
            None,
        );
        let map: std::collections::HashMap<_, _> = rows.iter().cloned().collect();
        assert!(!map.contains_key("Rating"));
        assert!(!map.contains_key("Plays"));
    }
}
