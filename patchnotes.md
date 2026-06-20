# Patch Notes

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
