<p align="center">
  <img src="logo.svg" alt="Conservatory" width="240">
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Language-Rust-blue" alt="Language: Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg" alt="License: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/GNOME-50%2B-4a86cf" alt="GNOME 50+">
  <img src="https://img.shields.io/badge/status-v0.0.2%20%C2%B7%20Phase%201-orange" alt="Status: v0.0.2, Phase 1">
</p>

---

# Conservatory

**Calibre for audio.**

A native GNOME library manager that owns and organizes your music and podcasts on disk, presented through a foobar2000 Columns UI browse surface and played through a libmpv daily-driver engine that runs both media types from a single queue. v0.0.2: the database spine (single-writer SQLite worker, read pool, numbered migrations, the music schema) is shipped, and the workspace is structured around compile-time plugins with music as the native program.

## Why this exists

Linux has players and it has taggers, but it has nothing that manages a music collection as a database the way Calibre manages books. deadbeef and friends play files in place but leave organization to you. Lattice (my own) audits and reports but treats the filesystem as canonical and never writes. Beets organizes from a terminal but is not a daily-driver player. And podcasts live in a separate app entirely, with their own queue, their own playback engine, their own idea of what "next" means. Conservatory is the one app that owns the library, browses it like foobar2000, plays it like a real player, and puts a podcast episode and an album track in the same queue.

Four commitments, in priority order:

1. **The database owns the library.** SQLite is the source of truth for organization and curated metadata; the app owns the on-disk layout and moves files to match it (`Genre / Album Artist / Album /` by default). This inverts the filesystem-canonical stance of Lattice and Belfry, the way Calibre takes a book and files it under its author tree. That trust is spent carefully: dry-run preview, an undo journal, and embedded-tag write-back so files stay portable and self-describing.
2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in deadbeef-cui), backed by the full Calibre-style search expression grammar.
3. **One engine, one queue, two media types.** Music tracks and podcast episodes share a single libmpv engine and a single play queue. Each item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for an episode.
4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen, replacing deadbeef. Gapless, ReplayGain, crossfade, output-device selection, MPRIS, media keys. Required, not optional: because Conservatory moves files, any external player's in-place references go stale the moment a library is re-shelved.

## Absorbs Belfry

Conservatory absorbs Brandon's podcast client, Belfry. Belfry's Phase 1 work is not discarded: its single-writer SQLite worker is the exact pattern this app needs and migrates here, and its audio engine (Smart Speed, Voice Boost) and Inbox → Queue → Played triage model become the Podcasts side. The one casualty is Belfry's filesystem-canonical design; in Conservatory, podcasts become app-managed downloads, acceptable for ephemeral episodes in a way it would not be for a curated music collection. **Belfry is not retired until Conservatory reaches podcast parity** (spec §16.8, roadmap Phase 6c).

**Author's Note:** I'm a college student in my late thirties with no professional industry experience yet; Conservatory is one in a string of native Linux desktop apps I'm building to learn the craft and assemble a portfolio. I came from foobar2000 and Directory Opus, and I keep a large Calibre library. What Calibre does for my books, nothing does for my music. Conservatory is the manager-and-player I want to exist. I work on Fedora 44 on a ThinkPad T14s AMD Gen 6; that's the environment it'll be tested against. I welcome contributions but can only honestly support my own setup.

## Status

v0.0.2, Phase 1 underway (1a and 1b shipped, 1c next). The database layer is real: single-writer worker, read-only pool, numbered migrations, the music schema with FTS5, and CLI smoke verbs over all of it.

- [`spec.md`](spec.md) — the design contract.
- [`roadmap.md`](roadmap.md) — the six-phase plan, broken into independently shippable sub-phases.
- [`patchnotes.md`](patchnotes.md) — release notes (newest at top).
- [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) — design lineage, dependency licenses, and the GPL-3-via-rubberband chain.
- [`docs/`](docs/) — design references: [schema](docs/schema.md), [path templates](docs/path-template.md), [genre normalization](docs/genre-normalization.md), [search grammar](docs/search-grammar.md), [libmpv profiles](docs/libmpv-profiles.md), [keymap](docs/keymap.md).

## Workspace

Six crates, matching the Belfry / Atrium discipline that every non-GUI surface stays CLI-testable. Music is the native program; podcasts and audiobooks are **compile-time plugins**: feature-gated crates, on by default, with all schema staying in core's single migration ledger (spec §2.2).

- `conservatory-core` — headless data layer and the music-native engine: SQLite worker, all migrations, import pipeline, file mover, playback host and profiles, the unified queue.
- `conservatory-search` — the Calibre-shaped search expression language (see [`docs/search-grammar.md`](docs/search-grammar.md)); deliberately feature-free.
- `conservatory-podcasts` — plugin crate: the absorbed Belfry podcast subsystem (Phase 6).
- `conservatory-audiobooks` — plugin crate: the audiobook subsystem (Phase 7).
- `conservatory-cli` — headless binary: import, organize, search, tag, queue, podcast ops, stats.
- `conservatory` — the GTK4 / libadwaita binary.

Both binaries take `--no-default-features` for a music-only build (no podcast or audiobook code compiled in), which CI keeps green alongside the full build.

## Stack

- **Rust 2024 Edition**
- **GTK 4.16+ / libadwaita 1.7+**
- **SQLite** via `rusqlite` (bundled, FTS5): single-writer worker, read-only pool, WAL mode
- **`tokio`** runtime; **`reqwest`** for podcast fetch; **`feed-rs`** + **`quick-xml`** for feeds (Phase 6)
- **Tag read/write** via `lofty` (and/or `symphonia`); **`image`** for cover decode and accent extraction
- **libmpv** via `libmpv2` + ffmpeg's `silenceremove` / `acompressor` / `equalizer` / `loudnorm` / `rubberband` filters
- **`oo7`** for libsecret credential storage (HTTP Basic per-show auth)
- **`zbus`** for MPRIS2 and the suspend inhibitor
- **Meson** wrapper over Cargo for Flatpak packaging
- **Memory budget:** < 200 MB idle, < 300 MB active on a 50k-track library (see [`spec.md`](spec.md) §13)

## Building (placeholder; real build instructions firm up as phases land)

The workspace skeleton currently builds clean but does nothing user-facing yet.

```bash
# Native (development)
cargo build --workspace
cargo test --workspace

# CI gate (matches the portfolio)
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

System build dependencies (Fedora 44):

```bash
sudo dnf install gtk4-devel libadwaita-devel mpv-libs-devel sqlite-devel
# For Smart Speed (rubberband filter): RPM Fusion's ffmpeg-libs (not ffmpeg-free-libs)
sudo dnf install --setopt=install_weak_deps=False ffmpeg-libs
```

## License

GPL-3.0-or-later. The license is forced by librubberband's GPL-2-or-later via the absorbed Smart Speed chain (spec §15); no relaxation is possible without replacing rubberband. See [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) for the full chain.
