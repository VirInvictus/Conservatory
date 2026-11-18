# Import Pipeline Reference

> **Status: implemented (music) at Phase 2d.** `conservatory-core/src/import/`. This expands spec §5.4.

Point Conservatory at a folder (or a file) and get an organized, database-owned library. The pipeline composes the earlier sub-phases: scan → read tags (1c) → resolve → derive shelf genre (2b) + accent (1c) → render targets (2a) → move/copy via the journaled mover (2c).

## Two passes

Import runs in two passes so a conflicting import leaves the database untouched:

1. **Resolution pass (in memory, read-only):** scan the folder for audio files, read each into a `TrackDraft`, group drafts into albums, decide each album's artist, derive its shelf genre, and render every track's target path. Then pre-check for conflicts (duplicate targets, existing destinations). If anything conflicts, return a report listing them and stop: nothing is created. (The pass does read the database once per album group: the matched-album lookup below. It still writes nothing, so the conflict guarantee is unchanged.)
2. **Persist pass:** create artists/albums/tracks, link genres, then run the move job (2c) to relocate the files into the managed tree and update the DB paths.

Because import inserts tracks at their *source* path and then runs the mover, an import is a journaled, undoable, crash-safe job exactly like an organize (the mover updates `file_path`/`folder_path` on completion).

## Resolution rules

- **Album grouping:** by `(album-artist-or-track-artist, album title)`, case-folded. Files with no album tag each become their own single (keyed by path) so loose files do not merge.
- **Album artist:** the shared `album_artist` tag if all tracks agree; else the shared track artist; else `None` → Various Artists (a compilation).
- **Artist identity** is the `sort_name` (the unique key, the Calibre author_sort trick). The sort name prefers an embedded `ARTISTSORT` / `ALBUMARTISTSORT` tag, falling back to a derivation that moves a leading article to the end (`"The Tuss"` → `"Tuss, The"`). Person-name inversion is deliberately not attempted; `sort_name` is editable later.
- **Album identity** is `(album_artist_id, title)`: re-importing or adding tracks to an existing album reuses it (`get_or_create_album`). A matched group renders from the **row's** fields, not its own: the row wins, and the incoming tags fill only the row's NULL holes (the writer backfills the matched row with the same `COALESCE`). This is what keeps a merged album in one folder: two source copies of one album — the same rip shelved under two genre folders, one tagged with a year and one without — resolve to a single row, and the tree an import leaves is the tree that row renders, so a fresh `organize` plans nothing new (issue #7).
- **`.lrc` lyrics** move with their audio in move mode: the import journals each claimed sidecar as a pure file op beside its track (no DB meaning), so the source folder ends empty, a crash rolls forward, and undo brings the lyrics back with the audio. The lyrics reader derives `Song.lrc` from the managed audio path, so nothing is stored in the database (issue #9). Copy mode is unchanged: sources stay, and no `.lrc` lands in the managed tree.
- **Shelf genre** is derived from the tracks' raw genres via the §5.2 chain (docs/genre-normalization.md) and stored single-valued; raw `track_genres` are kept untouched.
- **Accent** is computed from the first cover found among the album's drafts (embedded, else a sibling cover file) and stored in `albums.accent_rgb`.

## CLI

- `import <db> <source> <root> [--move] [--format tsv|json|human]` — **copies by default** (originals untouched); `--move` consumes them. Recovers any crash-interrupted job first.
- `organize <db> <root> [--apply] [--copy] [--undo <id>] [--format ...]` — re-render the managed tree from the DB and move files to match (dry-run by default).
- `shelf-genre-set <db> <album-id> <value>` — set an album's shelf genre; run `organize` afterward to move it.

`--json` emits a compact numeric summary. serde and serde_json are already workspace dependencies, so a richer curated-layer JSON export is a recorded roadmap feature, not a dependency question.

## Out of scope (for now)

Pruning directories left empty after a move. (The section's earlier entries all shipped: `tag set` and bulk editing at Phase 5a, sidecar-cover materialization at Phase 5d, config-driven `library_root` at Phase 10.)
