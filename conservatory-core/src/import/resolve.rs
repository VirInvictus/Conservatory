//! Turn a pile of [`TrackDraft`]s into album groups with resolved artists, the
//! pure half of import (spec §5.4). DB writes and path rendering live in `mod`.

use crate::tags::TrackDraft;

/// A resolved artist: the display `name` plus the `sort` name (the unique key and
/// the path component, the Calibre author_sort trick).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistName {
    pub name: String,
    pub sort: String,
}

/// Derive a sort name from a display name: move a leading article to the end
/// (`"The Tuss"` -> `"Tuss, The"`). Person-name inversion is deliberately *not*
/// attempted (bands are not people); `sort_name` is editable later. The reader
/// prefers an embedded `ARTISTSORT` tag over this (see `decide_*`).
pub fn derive_sort_name(name: &str) -> String {
    let trimmed = name.trim();
    for article in ["The ", "A ", "An "] {
        if let Some(rest) = strip_prefix_ci(trimmed, article) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return format!("{rest}, {}", article.trim());
            }
        }
    }
    trimmed.to_string()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// One album's worth of drafts, grouped from the scanned files.
#[derive(Debug)]
pub struct AlbumGroup {
    pub title: Option<String>,
    pub drafts: Vec<TrackDraft>,
}

/// Group drafts into albums. Key: `(album-artist-or-track-artist, album title)`,
/// case-folded; drafts with no album title each become their own single (keyed by
/// source path) so unrelated loose files do not merge.
pub fn group_albums(drafts: Vec<TrackDraft>) -> Vec<AlbumGroup> {
    let mut keys: Vec<String> = Vec::new();
    let mut groups: Vec<AlbumGroup> = Vec::new();

    for draft in drafts {
        let key = match &draft.album {
            Some(album) => {
                let artist = draft
                    .album_artist
                    .as_deref()
                    .or(draft.artist.as_deref())
                    .unwrap_or("");
                format!("{}\u{0}{}", artist.to_lowercase(), album.to_lowercase())
            }
            // No album tag: unique per file so loose singles stay separate.
            None => format!("\u{0}\u{0}{}", draft.source_path.display()),
        };
        match keys.iter().position(|k| k == &key) {
            Some(i) => groups[i].drafts.push(draft),
            None => {
                keys.push(key);
                groups.push(AlbumGroup {
                    title: draft.album.clone(),
                    drafts: vec![draft],
                });
            }
        }
    }
    groups
}

/// Decide an album's album-artist: the shared `album_artist` tag, else the shared
/// track artist, else `None` (a compilation -> Various Artists). The sort name
/// prefers an embedded sort tag, falling back to [`derive_sort_name`].
pub fn decide_album_artist(group: &AlbumGroup) -> Option<ArtistName> {
    if let Some(name) = shared(group, |d| d.album_artist.as_deref()) {
        let sort = group
            .drafts
            .iter()
            .find_map(|d| d.album_artist_sort.clone())
            .unwrap_or_else(|| derive_sort_name(&name));
        return Some(ArtistName { name, sort });
    }
    if let Some(name) = shared(group, |d| d.artist.as_deref()) {
        let sort = group
            .drafts
            .iter()
            .find_map(|d| d.artist_sort.clone())
            .unwrap_or_else(|| derive_sort_name(&name));
        return Some(ArtistName { name, sort });
    }
    None
}

/// The track artist for one draft: its `artist` tag with an embedded or derived
/// sort name. `None` when the file has no artist tag.
pub fn track_artist(draft: &TrackDraft) -> Option<ArtistName> {
    let name = draft.artist.clone()?;
    let sort = draft
        .artist_sort
        .clone()
        .unwrap_or_else(|| derive_sort_name(&name));
    Some(ArtistName { name, sort })
}

/// `Some(value)` if every draft shares the same non-empty value for `field`.
fn shared(group: &AlbumGroup, field: impl Fn(&TrackDraft) -> Option<&str>) -> Option<String> {
    let mut iter = group.drafts.iter().map(|d| field(d).unwrap_or(""));
    let first = iter.next().unwrap_or("");
    if first.is_empty() {
        return None;
    }
    if group.drafts.iter().all(|d| field(d).unwrap_or("") == first) {
        Some(first.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn draft(artist: Option<&str>, album_artist: Option<&str>, album: Option<&str>) -> TrackDraft {
        TrackDraft {
            source_path: PathBuf::from(format!(
                "/x/{}-{}.flac",
                artist.unwrap_or("?"),
                album.unwrap_or("?")
            )),
            title: Some("t".into()),
            artist: artist.map(String::from),
            album_artist: album_artist.map(String::from),
            artist_sort: None,
            album_artist_sort: None,
            album: album.map(String::from),
            track_no: None,
            track_total: None,
            disc_no: None,
            disc_total: None,
            year: None,
            genres: vec![],
            replaygain_track: None,
            replaygain_album: None,
            format: Some("flac".into()),
            bitrate: None,
            sample_rate: None,
            duration: None,
            cover: None,
        }
    }

    #[test]
    fn sort_name_moves_leading_article() {
        assert_eq!(derive_sort_name("The Tuss"), "Tuss, The");
        assert_eq!(derive_sort_name("an Album"), "Album, An");
        assert_eq!(derive_sort_name("Aphex Twin"), "Aphex Twin"); // "A" only as a word
        assert_eq!(derive_sort_name("Boards of Canada"), "Boards of Canada");
    }

    #[test]
    fn groups_by_artist_and_album() {
        let drafts = vec![
            draft(Some("BoC"), None, Some("Geogaddi")),
            draft(Some("BoC"), None, Some("Geogaddi")),
            draft(Some("Autechre"), None, Some("Amber")),
        ];
        let groups = group_albums(drafts);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].drafts.len(), 2);
    }

    #[test]
    fn shared_album_artist_wins() {
        let g = AlbumGroup {
            title: Some("Comp".into()),
            drafts: vec![
                draft(Some("A"), Some("VA Host"), Some("Comp")),
                draft(Some("B"), Some("VA Host"), Some("Comp")),
            ],
        };
        assert_eq!(decide_album_artist(&g).unwrap().name, "VA Host");
    }

    #[test]
    fn differing_artists_with_no_album_artist_is_various() {
        let g = AlbumGroup {
            title: Some("Comp".into()),
            drafts: vec![
                draft(Some("A"), None, Some("Comp")),
                draft(Some("B"), None, Some("Comp")),
            ],
        };
        assert_eq!(decide_album_artist(&g), None); // Various Artists
    }

    #[test]
    fn single_artist_album_uses_that_artist() {
        let g = AlbumGroup {
            title: Some("Geogaddi".into()),
            drafts: vec![draft(Some("BoC"), None, Some("Geogaddi"))],
        };
        assert_eq!(decide_album_artist(&g).unwrap().name, "BoC");
    }
}
