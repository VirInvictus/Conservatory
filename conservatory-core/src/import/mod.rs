//! The import pipeline (spec §5.4, roadmap Phase 2d): scan a folder → read tags
//! (1c) → resolve artists/albums/genres → derive shelf genre (2b) + accent (1c)
//! → render targets (2a) → move/copy into the managed tree (2c).
//!
//! Import runs in two passes. The **resolution pass** is in memory: it groups
//! drafts into albums, decides album artists, derives shelf genres, and renders
//! target paths (all pure, no DB writes), then pre-checks for conflicts. Only if
//! the plan is clear does the **persist pass** create rows and run the move job,
//! so a conflicting import leaves the database untouched.

pub mod resolve;
pub mod scan;

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::accent::{compute_accent, find_cover_bytes};
use crate::db::models::{Album, Track};
use crate::db::{ReadPool, WorkerHandle};
use crate::errors::Result;
use crate::import::resolve::{AlbumGroup, ArtistName};
use crate::mover::{self, Conflict, MoveKind, MoveMode, MoveOp};
use crate::path_template::{PathTemplate, TrackFields, find_collisions};
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
    folder_rel: Option<PathBuf>,
}

/// Import a folder (or a single file) into the library. See the module docs for
/// the two-pass shape and the conflict guarantee.
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
        let accent = album_accent(&group);
        let title = group.title.clone();

        planned_albums.push(PlannedAlbum {
            title,
            album_artist,
            shelf_genre,
            year,
            accent,
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
    let root = &opts.library_root;
    let dsts: Vec<PathBuf> = planned_tracks
        .iter()
        .map(|(.., rel)| root.join(rel))
        .collect();
    let mut conflicts = Vec::new();
    for (dst, ops) in find_collisions(&dsts) {
        conflicts.push(Conflict::DuplicateTarget { dst, ops });
    }
    for (i, dst) in dsts.iter().enumerate() {
        if dst.exists() {
            conflicts.push(Conflict::TargetExists {
                dst: dst.clone(),
                op: i,
            });
        }
    }
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
            rating: 0,
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
        ops.push(MoveOp {
            track_id: Some(track_id),
            album_id: Some(album_id),
            src,
            dst: root.join(&rel),
            db_old: Some(src_str),
            db_new: Some(rel.to_string_lossy().into_owned()),
        });
    }

    let tracks = ops.len();
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

/// The album accent from the first cover found among its drafts (embedded, else a
/// sibling cover file), median-cut per spec §7.4.
fn album_accent(group: &AlbumGroup) -> Option<u32> {
    for draft in &group.drafts {
        if let Some(bytes) = find_cover_bytes(&draft.source_path, draft)
            && let Ok(accent) = compute_accent(&bytes)
        {
            return Some(accent);
        }
    }
    None
}
