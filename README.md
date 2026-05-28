# Conservatory

**Calibre for audio.** A native GNOME library manager that owns and organizes your music and podcasts on disk, presented through a foobar2000 Columns UI browse surface and played through a libmpv daily-driver engine that runs both media types from a single queue.

> Status: **v0.0.1, Phase 1 underway.** The workspace skeleton exists; feature code is just starting. See `roadmap.md` for the phase plan and `spec.md` for the full design contract.

## What it is

Conservatory is the convergence of Brandon's music tooling (Lattice, deadbeef-cui) and his podcast project (Belfry) into one media app. Four commitments, in priority order:

1. **The database owns the library.** SQLite is the source of truth for organization and curated metadata; the app owns the on-disk layout and moves files to match it (`Genre / Album Artist / Album /` by default). This inverts the filesystem-canonical stance of Lattice and Belfry, the way Calibre takes a book and files it under its author tree. That trust is spent carefully: dry-run, undo journal, and embedded-tag write-back so files stay portable.
2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in deadbeef-cui), backed by the full Calibre-style search expression grammar.
3. **One engine, one queue, two media types.** Music tracks and podcast episodes share a single libmpv engine and a single play queue. Each item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for an episode.
4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen. Gapless, ReplayGain, crossfade, output-device selection, MPRIS, media keys.

## Absorbs Belfry

Conservatory absorbs Brandon's podcast client, Belfry. Belfry's Phase 1 work is not discarded: its single-writer SQLite worker is the exact pattern this app needs and migrates here, and its audio engine and Inbox to Queue to Played triage model become the Podcasts side. **Belfry is not retired until Conservatory reaches podcast parity** (spec §17, Phase 6).

## Workspace

Four crates, matching the Belfry / Atrium discipline that every non-GUI surface stays CLI-testable:

- `conservatory-core` — headless data layer (SQLite worker, import pipeline, playback, podcast fetch).
- `conservatory-search` — the Calibre-shaped search expression language.
- `conservatory-cli` — headless binary: import, organize, search, tag, queue, podcast ops, stats.
- `conservatory` — the GTK4 / libadwaita binary.

## Building

```sh
cargo build              # the workspace
cargo test               # unit + integration tests
cargo clippy -- -D warnings
```

Meson is the Flatpak packaging wrapper, wired in at a later phase. Direct development uses Cargo.

## License

GPL-3.0-or-later, forced by librubberband in the absorbed Smart Speed chain (spec §15). The design lineage and full license chain are documented in `ATTRIBUTIONS.md`.
