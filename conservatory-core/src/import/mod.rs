//! The import pipeline (spec §5.4, roadmap Phase 2d): scan a folder → read tags
//! (1c) → resolve artists/albums/genres → derive shelf genre (2b) + accent (1c)
//! → render targets (2a) → move/copy into the managed tree (2c).
//!
//! Import runs in two passes. The **resolution pass** is in memory: it groups
//! drafts into albums, decides album artists, derives shelf genres, and renders
//! target paths (all pure, no DB writes), then pre-checks for conflicts (the
//! mover's plan: duplicate targets, existing targets, and sources vanished
//! since the scan). Only if the plan is clear does the **persist pass** create
//! rows and run the move job, so a conflicting import leaves the database
//! untouched.

pub mod resolve;
pub mod scan;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::accent::{CoverSource, compute_accent, find_cover_source};
use crate::db::models::{Album, Track};
use crate::db::{ReadPool, WorkerHandle};
use crate::errors::Result;
use crate::import::resolve::ArtistName;
use crate::mover::{self, Conflict, MoveKind, MoveMode, MoveOp};
use crate::path_template::{PathTemplate, TrackFields};
use crate::shelf_genre::{AlbumGenreInput, GenreVocab, resolve_shelf_genre};
use crate::tags::read_track;

/// How an import runs.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The managed library root the rendered tree hangs off.
    pub library_root: PathBuf,
    /// Copy (leave originals) or move (consume them). The CLI defaults to copy.
    pub mode: MoveMode,
}

/// What an import did (or, when blocked, why it did nothing).
#[derive(Debug, Default)]
pub struct ImportReport {
    pub files_scanned: usize,
    pub skipped_unreadable: usize,
    pub artists: usize,
    pub albums: usize,
    pub tracks: usize,
    pub job_id: Option<i64>,
    /// Non-empty means the import was refused; no rows were created.
    pub conflicts: Vec<Conflict>,
}

struct PlannedAlbum {
    title: Option<String>,
    album_artist: Option<ArtistName>,
    shelf_genre: String,
    year: Option<i32>,
    accent: Option<u32>,
    /// The cover bytes (embedded or sibling), written to disk after the move
    /// (Phase 5d). The accent is derived from the same bytes.
    cover: Option<Vec<u8>>,
    /// The sibling cover file the bytes came from, when they are not embedded.
    /// A move-mode import journals it into the managed tree like any other
    /// file (the 2026-09-11 functional-pass finding: an unjournaled sidecar
    /// was left behind in an otherwise-consumed source folder); copy mode
    /// keeps the source and writes the canonical copy as before. `None` for
    /// embedded covers, and for a sidecar another album group already claimed
    /// (two groups can share one source directory; the first group wins).
    sidecar: Option<PathBuf>,
    folder_rel: Option<PathBuf>,
}

/// Import a folder (or a single file) into the library. See the module docs for
/// the two-pass shape and the conflict guarantee.
/// The import pre-check over `(source, destination)` pairs: the mover's own
/// [`mover::plan`], so the early refusal (before any DB write) sees exactly
/// what `apply` would refuse. Pure: stats only.
fn precheck_conflicts(provisional: Vec<(PathBuf, PathBuf)>) -> Vec<Conflict> {
    let ops = provisional
        .into_iter()
        .map(|(src, dst)| MoveOp {
            track_id: None,
            album_id: None,
            book_id: None,
            src,
            dst,
            db_old: None,
            db_new: None,
        })
        .collect();
    mover::plan(ops).conflicts
}

pub async fn import_folder(
    worker: &WorkerHandle,
    pool: &ReadPool,
    source: &Path,
    opts: &ImportOptions,
) -> Result<ImportReport> {
    let files = scan::scan(source)?;
    let files_scanned = files.len();

    let mut drafts = Vec::new();
    let mut skipped_unreadable = 0;
    for file in files {
        match read_track(&file) {
            Ok(draft) => drafts.push(draft),
            Err(_) => skipped_unreadable += 1,
        }
    }
    if drafts.is_empty() {
        return Ok(ImportReport {
            files_scanned,
            skipped_unreadable,
            ..Default::default()
        });
    }

    let vocab = {
        let conn = pool.open()?;
        GenreVocab::load(&conn)?
    };
    let template = PathTemplate::default_music();

    // --- Resolution pass (in memory) ---
    let mut planned_albums: Vec<PlannedAlbum> = Vec::new();
    // (album index, draft, track artist, rendered relative path)
    let mut planned_tracks: Vec<(usize, crate::tags::TrackDraft, Option<ArtistName>, PathBuf)> =
        Vec::new();
    // Sidecar covers claimed for journaling, across album groups (two groups
    // can share one source directory; one physical file, one claim).
    let mut claimed_sidecars: HashSet<PathBuf> = HashSet::new();

    for group in resolve::group_albums(drafts) {
        let album_idx = planned_albums.len();
        let album_artist = resolve::decide_album_artist(&group);
        let track_genres: Vec<Vec<String>> =
            group.drafts.iter().map(|d| d.genres.clone()).collect();
        let shelf_genre = resolve_shelf_genre(
            &AlbumGenreInput {
                track_genres: &track_genres,
                ..Default::default()
            },
            &vocab,
        );
        let year = group.drafts.iter().find_map(|d| d.year);
        let cover_src = group
            .drafts
            .iter()
            .find_map(|d| find_cover_source(&d.source_path, d));
        let accent = cover_src
            .as_ref()
            .and_then(|c| compute_accent(c.bytes()).ok());
        let (cover, sidecar) = match cover_src {
            None => (None, None),
            Some(CoverSource::Embedded(bytes)) => (Some(bytes), None),
            Some(CoverSource::Sidecar { path, bytes }) => {
                // Only a move-mode import consumes the sidecar; copy mode
                // leaves the source in place and writes the canonical copy.
                let claimable =
                    opts.mode == MoveMode::Move && claimed_sidecars.insert(path.clone());
                (Some(bytes), if claimable { Some(path) } else { None })
            }
        };
        let title = group.title.clone();

        planned_albums.push(PlannedAlbum {
            title,
            album_artist,
            shelf_genre,
            year,
            accent,
            cover,
            sidecar,
            folder_rel: None,
        });

        for draft in group.drafts {
            let track_artist = resolve::track_artist(&draft);
            let album = &planned_albums[album_idx];
            let fields = TrackFields {
                shelf_genre: Some(&album.shelf_genre),
                albumartist: album.album_artist.as_ref().map(|a| a.sort.as_str()),
                album: album.title.as_deref(),
                year: album.year,
                track_no: draft.track_no,
                disc_no: draft.disc_no,
                title: draft.title.as_deref(),
                artist: draft.artist.as_deref(),
                ext: draft.format.as_deref(),
            };
            let rel = template.render(&fields);
            if planned_albums[album_idx].folder_rel.is_none() {
                planned_albums[album_idx].folder_rel = rel.parent().map(Path::to_path_buf);
            }
            planned_tracks.push((album_idx, draft, track_artist, rel));
        }
    }

    // --- Conflict pre-check (before any DB write) ---
    // The mover's own plan over the provisional move list (audio + claimed
    // sidecars), so the refusal covers everything `apply` would refuse:
    // duplicate targets, targets that already exist, and a source that
    // vanished between the scan and the persist pass. A vanished file used to
    // slip past this check and fail the job after the rows were written.
    let root = &opts.library_root;
    let mut provisional: Vec<(PathBuf, PathBuf)> = planned_tracks
        .iter()
        .map(|(_, draft, _, rel)| (draft.source_path.clone(), root.join(rel)))
        .collect();
    for pa in &planned_albums {
        if let (Some(sidecar), Some(folder_rel)) = (&pa.sidecar, &pa.folder_rel) {
            let name = sidecar
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            provisional.push((sidecar.clone(), root.join(folder_rel).join(name)));
        }
    }
    let conflicts = precheck_conflicts(provisional);
    if !conflicts.is_empty() {
        return Ok(ImportReport {
            files_scanned,
            skipped_unreadable,
            conflicts,
            ..Default::default()
        });
    }

    // --- Persist pass (create rows, then move) ---
    let now = Utc::now();
    let mut album_ids = Vec::with_capacity(planned_albums.len());
    let mut artist_ids = std::collections::HashSet::new();

    for pa in &planned_albums {
        let album_artist_id = match &pa.album_artist {
            Some(a) => {
                let id = worker
                    .get_or_create_artist(a.name.clone(), a.sort.clone(), None)
                    .await?;
                artist_ids.insert(id);
                Some(id)
            }
            None => None,
        };
        let folder_path = pa
            .folder_rel
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let album = Album {
            id: 0,
            title: pa.title.clone().unwrap_or_else(|| "Unknown Album".into()),
            album_artist_id,
            shelf_genre: Some(pa.shelf_genre.clone()),
            year: pa.year,
            release_date: None,
            musicbrainz_release_id: None,
            cover_path: None,
            accent_rgb: pa.accent,
            folder_path,
            added_at: Some(now),
        };
        album_ids.push(worker.get_or_create_album(album).await?);
    }

    let mut ops = Vec::with_capacity(planned_tracks.len());
    for (album_idx, draft, track_artist, rel) in planned_tracks {
        let artist_id = match track_artist {
            Some(a) => {
                let id = worker.get_or_create_artist(a.name, a.sort, None).await?;
                artist_ids.insert(id);
                Some(id)
            }
            None => None,
        };
        let album_id = album_ids[album_idx];
        let src = draft.source_path.clone();
        let src_str = src.to_string_lossy().into_owned();
        let track = Track {
            id: 0,
            album_id: Some(album_id),
            artist_id,
            title: draft.title.clone().unwrap_or_else(|| "Untitled".into()),
            track_no: draft.track_no.map(|n| n as i32),
            disc_no: draft.disc_no.map(|n| n as i32),
            duration: draft.duration,
            file_path: src_str.clone(), // source for now; the mover sets the managed path
            format: draft.format.clone(),
            bitrate: draft.bitrate.map(|b| b as i32),
            sample_rate: draft.sample_rate.map(|s| s as i32),
            replaygain_track: draft.replaygain_track,
            replaygain_album: draft.replaygain_album,
            rating: draft.rating.unwrap_or(0),
            play_count: 0,
            last_played: None,
            starred: false,
            musicbrainz_recording_id: None,
            added_at: Some(now),
        };
        let track_id = worker.insert_track(track).await?;
        for genre in &draft.genres {
            let genre_id = worker.get_or_create_genre(genre.clone()).await?;
            worker.link_track_genre(track_id, genre_id).await?;
        }
        // People credits (19b-iii): resolve into the shared artists rows, so a
        // credited name shares one namespace and sort discipline with artists.
        for credit in &draft.credits {
            let artist_id = worker
                .get_or_create_artist(
                    credit.name.clone(),
                    crate::names::derive_sort_name(&credit.name),
                    None,
                )
                .await?;
            worker
                .link_track_credit(track_id, artist_id, credit.role)
                .await?;
        }
        ops.push(MoveOp {
            track_id: Some(track_id),
            album_id: Some(album_id),
            book_id: None,
            src,
            dst: root.join(&rel),
            db_old: Some(src_str),
            db_new: Some(rel.to_string_lossy().into_owned()),
        });
    }

    let tracks = ops.len();

    // Sidecar covers ride the same journal in move mode (one op per claimed
    // sidecar): the file is consumed like the audio, so the source folder is
    // left clean, a crash rolls forward, and undo moves it back. The op is
    // cover-shaped (`album_id` set, no `track_id`): the journal rewrites
    // `albums.cover_path` under its guard on undo.
    for (idx, pa) in planned_albums.iter().enumerate() {
        if let (Some(sidecar), Some(folder_rel)) = (&pa.sidecar, &pa.folder_rel) {
            let name = sidecar
                .file_name()
                .map(|n| n.to_os_string())
                .unwrap_or_default();
            let rel = folder_rel.join(&name);
            ops.push(MoveOp {
                track_id: None,
                album_id: Some(album_ids[idx]),
                book_id: None,
                src: sidecar.clone(),
                dst: root.join(&rel),
                // No managed cover existed before the import: the post-move
                // write below records the pointer, and undo clears it.
                db_old: None,
                db_new: Some(rel.to_string_lossy().into_owned()),
            });
        }
    }

    let job_id = mover::apply(
        worker,
        pool,
        MoveKind::Import,
        opts.mode,
        root,
        now.timestamp(),
        ops,
    )
    .await?;

    // Cover to disk (Phase 5d): the move has created the album folders, so write
    // each album's cover.jpg and record its path. Best-effort: a cover failure
    // never fails an otherwise-successful import (covers are re-derivable).
    // The computed accent rides along: for a *matched* (pre-existing) album the
    // insert above never ran, so this is the only place the accent can land
    // (the sweep fix; previously a NULL accent survived every re-import).
    for (idx, pa) in planned_albums.iter().enumerate() {
        let Some(folder_rel) = &pa.folder_rel else {
            continue;
        };
        // A journaled sidecar already sits in the album folder (the move moved
        // it): point the album at the moved file (accent rides along) instead
        // of writing a second, canonical copy beside it.
        if let Some(sidecar) = &pa.sidecar {
            let name = sidecar
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let cover_path = folder_rel.join(&name).to_string_lossy().into_owned();
            if let Err(e) = worker
                .set_album_cover_path(album_ids[idx], Some(cover_path), pa.accent)
                .await
            {
                tracing::warn!(album_id = album_ids[idx], error = %e, "cover path not recorded");
            }
            continue;
        }
        if let Some(bytes) = &pa.cover {
            let folder = folder_rel.to_string_lossy();
            if let Ok(cover_path) = crate::covers::sync_album_cover(root, &folder, bytes, None)
                && let Err(e) = worker
                    .set_album_cover_path(album_ids[idx], Some(cover_path), pa.accent)
                    .await
            {
                // A cover failure never fails the import (covers re-derive), but a
                // DB-write failure means the worker is wedged, so surface it.
                tracing::warn!(album_id = album_ids[idx], error = %e, "cover path not recorded");
            }
        }
    }

    Ok(ImportReport {
        files_scanned,
        skipped_unreadable,
        artists: artist_ids.len(),
        albums: album_ids.len(),
        tracks,
        job_id: Some(job_id),
        conflicts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn the_precheck_reports_a_source_that_vanished() {
        // The vanished-file path: the scan read the tag, the file is gone by
        // the persist pass, and the pre-check refuses before any DB write
        // (the music importer used to have no MissingSource check at all).
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("here.flac"), b"audio").unwrap();

        let conflicts = precheck_conflicts(vec![
            (
                dir.path().join("gone.flac"),
                dir.path().join("tree/gone.flac"),
            ),
            (
                dir.path().join("here.flac"),
                dir.path().join("tree/here.flac"),
            ),
        ]);

        assert!(
            matches!(&conflicts[..], [Conflict::MissingSource { .. }]),
            "got: {conflicts:?}"
        );
    }

    #[test]
    fn the_precheck_reports_duplicate_and_existing_targets() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.flac"), b"a").unwrap();
        std::fs::write(dir.path().join("b.flac"), b"b").unwrap();
        std::fs::create_dir_all(dir.path().join("tree")).unwrap();
        std::fs::write(dir.path().join("tree/taken.flac"), b"taken").unwrap();

        let conflicts = precheck_conflicts(vec![
            (
                dir.path().join("a.flac"),
                dir.path().join("tree/taken.flac"),
            ),
            (dir.path().join("a.flac"), dir.path().join("tree/x.flac")),
            (dir.path().join("b.flac"), dir.path().join("tree/x.flac")),
        ]);

        assert_eq!(conflicts.len(), 2, "got: {conflicts:?}");
    }
}
