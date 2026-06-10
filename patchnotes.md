# Patch Notes

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
