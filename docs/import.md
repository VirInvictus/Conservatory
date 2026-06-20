# Import Pipeline Reference

> **Status: implemented (music) at Phase 2d.** `conservatory-core/src/import/`. This expands spec §5.4.

Point Conservatory at a folder (or a file) and get an organized, database-owned library. The pipeline composes the earlier sub-phases: scan → read tags (1c) → resolve → derive shelf genre (2b) + accent (1c) → render targets (2a) → move/copy via the journaled mover (2c).

## Two passes

Import runs in two passes so a conflicting import leaves the database untouched:

1. **Resolution pass (in memory, no DB writes):** scan the folder for audio files, read each into a `TrackDraft`, group drafts into albums, decide each album's artist, derive its shelf genre, and render every track's target path. Then pre-check for conflicts (duplicate targets, existing destinations). If anything conflicts, return a report listing them and stop: nothing is created.
2. **Persist pass:** create artists/albums/tracks, link genres, then run the move job (2c) to relocate the files into the managed tree and update the DB paths.

Because import inserts tracks at their *source* path and then runs the mover, an import is a journaled, undoable, crash-safe job exactly like an organize (the mover updates `file_path`/`folder_path` on completion).

## Resolution rules

- **Album grouping:** by `(album-artist-or-track-artist, album title)`, case-folded. Files with no album tag each become their own single (keyed by path) so loose files do not merge.
- **Album artist:** the shared `album_artist` tag if all tracks agree; else the shared track artist; else `None` → Various Artists (a compilation).
- **Artist identity** is the `sort_name` (the unique key, the Calibre author_sort trick). The sort name prefers an embedded `ARTISTSORT` / `ALBUMARTISTSORT` tag, falling back to a derivation that moves a leading article to the end (`"The Tuss"` → `"Tuss, The"`). Person-name inversion is deliberately not attempted; `sort_name` is editable later.
- **Album identity** is `(album_artist_id, title)`: re-importing or adding tracks to an existing album reuses it (`get_or_create_album`).
- **Shelf genre** is derived from the tracks' raw genres via the §5.2 chain (docs/genre-normalization.md) and stored single-valued; raw `track_genres` are kept untouched.
- **Accent** is computed from the first cover found among the album's drafts (embedded, else a sibling cover file) and stored in `albums.accent_rgb`.

## CLI

- `import <db> <source> <root> [--move] [--format tsv|json|human]` — **copies by default** (originals untouched); `--move` consumes them. Recovers any crash-interrupted job first.
- `organize <db> <root> [--apply] [--copy] [--undo <id>] [--format ...]` — re-render the managed tree from the DB and move files to match (dry-run by default).
- `shelf-genre-set <db> <album-id> <value>` — set an album's shelf genre; run `organize` afterward to move it.

`--json` emits a compact numeric summary (a richer serde-backed JSON is deferred until the dependency is signed off).

## Out of scope (for now)

`tag set` and bulk editing (Phase 5a); materializing `cover.jpg` into the managed tree and setting `albums.cover_path` (the accent is computed and stored, but the cover file is not yet copied in — a small §7.4 follow-up); pruning directories left empty after a move; config-driven `library_root` (Phase 10).
