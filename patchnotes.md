# Patch Notes

## v0.0.9

Phase 3b shipped: the first GTK4/libadwaita code. `conservatory` is now a launching app with the deadbeef-cui "Columns UI" faceted browse (spec §3.3).

- **Facet logic (`conservatory-core/src/db/facets.rs`, headless + tested):** `facet_rows` (distinct values of Genre / Album Artist / Album with `COUNT(DISTINCT track)`, narrowed by upstream selections) and `facet_tracks` (the leaf set). Genre is multi-valued: a track tagged `Electronic; Ambient` counts under both rows (the §5.2 decoupling). The CLAUDE.md hard rule keeps the logic in core; the GTK binary only renders. `debug-facets <db>` exercises it headless.
- **GTK browse window (`conservatory/src/ui/`, programmatic):** an `adw::ApplicationWindow` with a row of facet panes (`ListView` + `MultiSelection`, each topped by an `[All (N)]` row, memoized counts) and a minimal leaf track list. Selecting facet rows narrows the downstream panes and the leaf (the cascade); the real sortable track list lands at 3c.
- **Coalescing:** ported Viaduct's `CoalescingQueue` (interval + max-interval flush, dedup) to debounce selection changes into one cascade recompute per multi-select drag, never per row (spec §2.1).
- **CI:** the `libgtk-4-dev` / `libadwaita-1-dev` install lands in both jobs.

Deferred: user-reconfigurable + persisted pane order (Phase 10 config); the sortable track list + filter bar (3c); `BatchUpdate` / live deltas (until an in-GUI writer, 5a).

## v0.0.8

Phase 3a shipped: the `conservatory-search` expression engine and a CLI `search` verb (the first piece of Phase 3, GTK browse).

- **Grammar pipeline (`conservatory-search`):** `lex` → `parse` (typed AST + extracted `sort:` specs) → `eval` (in-memory) + `sql_translate` (all-or-nothing SQL `WHERE`, so the two paths never diverge), with `rank` (bm25 + recency). Structure ported from `atrium-search`, semantics from CalibreQuarry, FTS plumbing from Viaduct; an independent implementation. Storage-agnostic (`SqlValue`, no rusqlite, no core); deps `regex` + `chrono` only.
- **Grammar:** the music field set (`artist`/`albumartist`/`album`/`title`, `genre` vs `shelfgenre`, `year`/`added`, `rating`/`bitrate`/`duration`/`format`, `is:played`/`is:starred`/`is:queued`), match modifiers (substring/`=`/`~`regex/`?`fuzzy), boolean + ranges + date keywords/precision + presence, `sort:` as metadata. The parser is **forgiving** (degrades to substring, never errors). `vl:` perspectives expand at parse time with a cycle guard.
- **CLI:** `search <db> '<expr>' [--format tsv|json|human]` — SQL fast path when the whole expression translates, else the in-memory evaluator; bare-text hits ranked by bm25 + recency. New core reads `search_rows` / `search_track_ids` / `fts_rank` (the consumer maps `SqlValue` → a core `SqlParam`, keeping core search-free).
- **Tests:** parse round-trip, per-field eval, per-node SQL, `vl:` cycle guard, and SQL-vs-eval **parity** over a 2,000-track fixture; hand-verified against the real imported albums.
- **Deferred:** persistent Perspective storage + UI (3c); `is:queued` matches nothing until the queue table lands (4b); podcast/audiobook fields (6/7).

## v0.0.7

Phase 2d shipped: the import pipeline and real CLI verbs. **The manager is usable headless** (the Phase 2 exit): point the CLI at a folder and get an organized, database-owned library.

- **Import pipeline (`src/import/`):** scan a folder → read tags → resolve artists/albums/genres → derive shelf genre + accent → render targets → move/copy into the managed tree. Runs in two passes: an in-memory resolution + conflict pre-check, then (only if clear) the persist + move, so a conflicting import leaves the database untouched. Import inserts at the source path and runs the journaled mover, so it is undoable and crash-safe like organize.
- **Resolver:** album grouping by `(artist, title)`; album artist from the shared album-artist tag, else shared track artist, else Various Artists; artist identity by `sort_name` (embedded `ARTISTSORT` preferred, else a leading-article derivation); album identity `(album_artist_id, title)` so re-imports reuse the album.
- **CLI:** `import <db> <source> <root>` (copies by default; `--move` to consume), `organize` (re-render from the DB; dry-run/`--apply`/`--undo`), `shelf-genre-set`. Output `--tsv` (default) / `--json` / `--human`. The old `debug-organize` is promoted to `organize`.
- **Worker:** `get_or_create_artist`/`get_or_create_album`/`set_album_shelf_genre`. The tag reader now also reads embedded sort-name tags.
- **Tests:** `tests/import.rs` end-to-end (import into a managed tree, copy-vs-move, re-import refusal, shelf-genre-set → organize) plus resolver/scan unit tests; hand-verified against two real albums (mp3 + opus).

## v0.0.6

Phase 2c shipped: the crash-safe file mover. This is the trust-critical, release-blocking subsystem (spec §5.4); moving the user's files is the headline risk.

- **Mover engine (`src/mover/`):** `plan` (pure dry-run preview with conflict detection), `apply` (journal-first, then execute), `undo` (revert a completed job), and `recover` (roll-forward replay of interrupted jobs at startup). The journal is a SQLite ledger (migration `0002`: `move_jobs` + `move_operations`), written before any file is touched and durable via WAL. Recovery rolls forward (completes the move the user asked for); replay is idempotent.
- **Per-file primitive (`mover::fsops`):** same-filesystem `rename` fast path; cross-filesystem copy → fsync → verify → delete (modeled on Atrium's atomic write). Idempotent: a file already at its target is a no-op, which is what makes crash replay safe.
- **Conflict policy:** duplicate targets, missing sources, and existing destinations refuse the whole job; nothing moves. Copy-vs-move is a per-job choice.
- **DB consistency:** completing an operation updates `tracks.file_path` and `albums.folder_path` in the same transaction as marking it done; undo reverts both the tree and the DB.
- **Worker + CLI:** new journal commands on the single writer (file I/O stays off it); `debug-organize <db> <root> [--apply] [--copy] [--undo <id>]`.
- **Tests:** the release-blocking suite (`tests/mover.rs`): move/undo round-trip, simulated mid-move crash rolling forward, conflict refusal, copy mode, tree↔DB consistency; plus `fsops` unit tests.

## v0.0.5

Phase 2b shipped: the shelf-genre resolver that decides each album's filed-under genre.

- **Resolver (`src/shelf_genre.rs`):** `normalize` splits raw tags on `;` `/` `,`, case-folds for matching, and maps through the alias vocabulary, keeping canonical/original casing in the output. `resolve_shelf_genre` runs the spec §5.2 priority chain (manual override → single album-level tag → most-common normalized track genre, ties broken by `genre_priority` rank then first-seen → `Unknown`). `resolve_album` is the DB-driven entry point; raw `track_genres` are read but never mutated (the §5.2 decoupling).
- **Genre vocabulary (spec §16.4, now settled):** empty and user-built. Conservatory ships no default alias map or priority list; the schema can seed one (beets `lastgenre` or MusicBrainz) later without a migration.
- **DB + CLI:** `album_track_genres` reads an album's per-track genres; `debug-shelf-genre <db>` derives and compares against the stored value (the headless usable artifact).

## v0.0.4

Phase 2a shipped: the path-template engine that renders the on-disk tree from the database.

- **Path-template engine (`src/path_template.rs`):** `PathTemplate::parse` validates a template (unbalanced braces, unknown tokens, malformed format specs are errors); `render(&TrackFields)` is infallible once parsed. The default music template renders `{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. An album resolves to one path; compilations bucket under Various Artists (spec §5.1). Per-field fallbacks keep structural folders non-empty; optional pieces (year, track, disc) collapse with their surrounding literals.
- **Sanitization (docs/path-template.md):** per-component path-separator replacement, reserved-device-name escaping, trailing dot/space trimming, whitespace collapse, and a per-component byte cap. Raw tags never reach the filesystem; the embedded tag keeps the true value (spec §5.5).
- **Collision detection:** `find_collisions` groups tracks that render to the same target, for the Phase 2c mover to refuse or disambiguate.
- **DB + CLI:** `track_render_rows` joins tracks with their album/artist context; `debug-paths <db>` renders a whole library and reports collisions (the headless usable artifact).

## v0.0.3

Phase 1c shipped: the engine can read a real audio file.

- **Tag reader (`src/tags.rs`):** `read_track` reads embedded tags and audio properties into a `TrackDraft` (title, artists, album, track/disc numbers and totals, year, raw multi-value genres, ReplayGain, format, bitrate, sample rate, duration, embedded cover). Raw genres are kept verbatim, decoupled from the eventual shelf genre (spec §5.2). Built on `lofty`, signed off over `symphonia` (spec §7.1) so one library also serves the Phase 5b write-back.
- **Cover accent (`src/accent.rs`):** `compute_accent` decodes a cover and derives a packed-RGB accent via a median-cut quantizer ranked by vibrancy, a faithful port of Hermitage (spec §7.4, docs/accent.md). `find_cover_bytes` prefers the embedded picture, falling back to a sibling cover file. `image` signed off with jpeg + png features.
- **CLI:** `debug-tags <file>` reads a file into a draft and prints it with the accent (the headless usable artifact).
- **Tests + fixtures:** per-format integration tests (flac/mp3/opus/m4a) over the first committed binary fixtures in the workspace, plus deterministic accent unit tests over in-memory covers. Fixtures regenerate via the `gen_audio_fixtures` example (ffmpeg + lofty); CI reads the committed files and stays hermetic.

## v0.0.2

Phase 1a + 1b shipped, and the workspace restructured around compile-time plugins.

- **Phase 1a, the writer:** single-writer SQLite worker (panic-catch-and-restart loop, per-op `oneshot` replies, ported in shape from `belfry-core` / Viaduct), read-only connection pool, WAL + pragma discipline, and the numbered `user_version` migration runner. CI (fmt / clippy / test) landed with it.
- **Phase 1b, the music data model:** migration `0001` (artists / albums / tracks / genres / `track_genres` / `genre_aliases` / `genre_priority`, plus `track_fts` and `album_fts` with sync triggers), the domain models, insert and read helpers, a synthetic fixture builder, and the `debug-roundtrip` / `debug-fixture` CLI smoke verbs.
- **Plugin restructure (spec §2.2, §16.13):** music is the native program; podcasts and audiobooks become compile-time plugins. New feature-gated workspace crates `conservatory-podcasts` (filled at Phase 6) and `conservatory-audiobooks` (filled at Phase 7), stubbed now so the wiring exists from day one. Both binaries gain `podcasts` / `audiobooks` features (default on) and report their enabled plugin set; CI gains a music-only (`--no-default-features`) job. The boundary rule: plugins are code and dependencies, not the database; all schema stays in core's single migration ledger and the unified queue stays a core commitment.

## v0.0.1

First commit. Project bootstrapped out of the design spec.

- Cargo workspace with the four planned crates (`conservatory-core`, `conservatory-search`, `conservatory-cli`, `conservatory`), all building as dependency-light skeletons.
- Portfolio scaffolding: `README.md`, `roadmap.md`, this file, `CLAUDE.md`, `ATTRIBUTIONS.md`, `VERSION`, GPL-3.0-or-later `LICENSE`, `.gitignore`, and a Meson packaging stub.
- Build deferral lifted. The spec previously parked the build behind an Atrium shipping milestone; that decision was reversed and the build now proceeds concurrently with Atrium, with hard phasing as the mitigation (spec §16.1, §17).
- Belfry retirement remains gated on podcast parity (Phase 6); nothing in Belfry has been removed.
