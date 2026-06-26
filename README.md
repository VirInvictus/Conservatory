<p align="center">
  <img src="logo.svg" alt="Conservatory" width="240">
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Language-Rust-blue" alt="Language: Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg" alt="License: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/GNOME-50%2B-4a86cf" alt="GNOME 50+">
  <img src="https://img.shields.io/badge/status-v0.0.52%20%C2%B7%20Phase%206%20complete-orange" alt="Status: v0.0.52, Phase 6 complete">
</p>

---

# Conservatory

**Calibre for audio.**

A native GNOME library manager that owns and organizes your music and podcasts on disk, presented through a foobar2000 Columns UI browse surface and played through a libmpv daily-driver engine that runs them from a single queue. As of v0.0.52 it is a daily-driver music player and a full podcast client in one app: import and organize a music library into a database-owned tree, browse it with a Calibre-style search grammar, play it with gapless playback, head-staged ReplayGain, and a real DSP chain (EQ, compressor/limiter, leveler), and run podcasts (fetch, Inbox → Queue → Played triage, Smart Speed, Voice Boost, chapters, a sleep timer) from the *same* unified queue, with full MPRIS / media-key integration throughout. Audiobooks, the third media type, are the next phase. Music is the native program; podcasts and audiobooks are compile-time plugins.

## Why this exists

Linux has players and it has taggers, but it has nothing that manages a music collection as a database the way Calibre manages books. deadbeef and friends play files in place but leave organization to you. Lattice (my own) audits and reports but treats the filesystem as canonical and never writes. Beets organizes from a terminal but is not a daily-driver player. And podcasts live in a separate app entirely, with their own queue, their own playback engine, their own idea of what "next" means. Conservatory is the one app that owns the library, browses it like foobar2000, plays it like a real player, and puts a podcast episode and an album track in the same queue.

Four commitments, in priority order:

1. **The database owns the library.** SQLite is the source of truth for organization and curated metadata; the app owns the on-disk layout and moves files to match it (`Genre / Album Artist / Album /` by default). This inverts the filesystem-canonical stance of Lattice and Belfry, the way Calibre takes a book and files it under its author tree. That trust is spent carefully: dry-run preview, an undo journal, and embedded-tag write-back so files stay portable and self-describing.
2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in deadbeef-cui), backed by the full Calibre-style search expression grammar.
3. **One engine, one queue, three media types.** Music tracks, podcast episodes, and (at Phase 7) audiobooks share a single libmpv engine and a single play queue. Each item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for a spoken-word item. A mixed listening queue (an album track next to a podcast episode) is the standout feature, and the reason Belfry was absorbed rather than kept separate.
4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen, replacing deadbeef. Gapless, head-staged ReplayGain, a DSP chain (EQ, compressor/limiter, leveler), output-device selection, MPRIS, media keys. Required, not optional: because Conservatory moves files, any external player's in-place references go stale the moment a library is re-shelved.

## Absorbs Belfry

Conservatory absorbs Brandon's podcast client, Belfry. Belfry's Phase 1 work is not discarded: its single-writer SQLite worker is the exact pattern this app needs and migrated here, and its audio engine (Smart Speed, Voice Boost) and Inbox → Queue → Played triage model became the Podcasts side. The one casualty is Belfry's filesystem-canonical design; in Conservatory, podcasts become app-managed downloads, acceptable for ephemeral episodes in a way it would not be for a curated music collection. **Conservatory reached podcast parity at v0.0.52 (Phase 6c, the sleep timer the last piece), so Belfry is now retired** (spec §16.8): its repo is archived and the podcast subsystem lives entirely here.

**Author's Note:** I'm a college student in my late thirties with no professional industry experience yet; Conservatory is one in a string of native Linux desktop apps I'm building to learn the craft and assemble a portfolio. I came from foobar2000 and Directory Opus, and I keep a large Calibre library. What Calibre does for my books, nothing does for my music. Conservatory is the manager-and-player I want to exist. I work on Fedora 44 on a ThinkPad T14s AMD Gen 6; that's the environment it'll be tested against. I welcome contributions but can only honestly support my own setup.

## Status

**v0.0.52. Phases 1 through 6 are complete: a daily-driver music player and a full podcast client in one app, with audiobooks (Phase 7) the remaining media type.** The managed tree is laid out as `Music/ | Podcasts/ | Audiobooks/` under the library root (spec §5.1). Each phase below left a usable artifact; [`roadmap.md`](roadmap.md) carries the sub-phase detail and [`patchnotes.md`](patchnotes.md) the per-release notes.

- **Phase 1: data layer.** Single-writer SQLite worker, read-only pool, numbered migrations, the music schema with FTS5, the embedded-tag reader (`lofty`), and median-cut cover accents.
- **Phase 2: the manager (headless).** Point the CLI at a folder and get an organized, database-owned library: tag read → shelf-genre resolution → path-template render → crash-safe move (dry-run preview, undo journal, roll-forward recovery). Verbs: `import`, `organize`, `shelf-genre-set`.
- **Phase 3: the browser.** `conservatory-search` (the Calibre-style grammar: lex → parse → eval + all-or-nothing SQL translate, bm25 + recency rank) behind a deadbeef-cui faceted browse window: hierarchical Genre → Album Artist → Album panes, a sortable multi-select track list, an always-on filter bar (`Ctrl+F`), and saved Perspectives.
- **Phase 4: the player.** The threaded libmpv engine and the unified queue: double-click to play the visible list, a persistent Now-bar transport, a drag-and-drop queue drawer (`Ctrl+U`), launch-resume paused at the cursor, MPRIS2 + media keys + a suspend inhibitor, and live output-device selection. **A daily-driver music player.**
- **Phase 5: editing & write-back.** Bulk metadata editing (CLI `tag set` / `tag replace`, and the `Ctrl+E` dialog with a move preview for path-affecting edits); embedded-tag write-back so the tree stays self-describing and a wipe-and-reimport reconstructs it (§5.6); in-app ReplayGain scan (`rsgain`, all formats including Opus); and cover-art-to-disk feeding the Now-bar and MPRIS art.
- **Phase 5.5: the audio engine.** A labelled `af`-chain built once per item and mutated live: head-staged per-track ReplayGain (fixing mpv #8267), a 10-band ISO graphic EQ with live sliders and persisted presets, a DSP rack (compressor, brick-wall limiter, leveler), and output backend + resampler control, all surfaced in a foobar2000-style Sound preferences page.
- **Phase 6: podcasts (absorbing Belfry).** The full podcast client: conditional-GET fetch, `feed-rs` + the `podcast:` namespace handler, OPML round-trip, libsecret credentials, and downloads; the Inbox → Queue → Played triage tab with tags; episodes in the *same* queue as music tracks, resuming to the second; per-show speed, Smart Speed (silence-skip) and Voice Boost; chapters, time-saved accounting, a Now Playing surface, and a sleep timer. **Podcast parity reached; Belfry retired** (spec §16.8).

Next: **Phase 7 (audiobooks)**, the third media type, reusing the spoken-word engine (variable speed, Smart Speed, Voice Boost, chapters, sleep timer) with per-book resume; or the independent **Phase 8** library audits (integrity, duplicates, health reports, playlist export).

- [`spec.md`](spec.md): the design contract.
- [`roadmap.md`](roadmap.md): the phased plan, broken into independently shippable sub-phases.
- [`patchnotes.md`](patchnotes.md): release notes (newest at top).
- [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md): design lineage, dependency licenses, and the GPL-3 chain analysis.
- [`docs/`](docs/): design references: [schema](docs/schema.md), [import](docs/import.md), [path templates](docs/path-template.md), [genre normalization](docs/genre-normalization.md), [file mover](docs/mover.md), [cover accent](docs/accent.md), [search grammar](docs/search-grammar.md), [libmpv profiles](docs/libmpv-profiles.md), [keymap](docs/keymap.md).

## Workspace

Six crates, matching the Belfry / Atrium discipline that every non-GUI surface stays CLI-testable. Music is the native program; podcasts and audiobooks are **compile-time plugins**: feature-gated crates, on by default, with all schema staying in core's single migration ledger (spec §2.2).

- `conservatory-core`: headless data layer and the music-native engine: SQLite worker, all migrations, import pipeline, file mover, playback host and profiles, the unified queue.
- `conservatory-search`: the Calibre-shaped search expression language (see [`docs/search-grammar.md`](docs/search-grammar.md)); deliberately feature-free.
- `conservatory-podcasts`: plugin crate for the absorbed Belfry podcast subsystem (Phase 6).
- `conservatory-audiobooks`: plugin crate for the audiobook subsystem (Phase 7).
- `conservatory-cli`: headless binary: import, organize, search, tag, queue, podcast ops, stats.
- `conservatory`: the GTK4 / libadwaita binary.

Both binaries take `--no-default-features` for a music-only build (no podcast or audiobook code compiled in), which CI keeps green alongside the full build.

## Stack

- **Rust 2024 Edition**
- **GTK 4.16+ / libadwaita 1.7+**
- **SQLite** via `rusqlite` (bundled, FTS5): single-writer worker, read-only pool, WAL mode
- **`tokio`** runtime; **`reqwest`** for podcast fetch; **`feed-rs`** + **`quick-xml`** for feeds (Phase 6)
- **Tag read/write** via `lofty` (signed off over `symphonia`, spec §7.1); **`image`** for cover decode and accent extraction
- **libmpv** via `libmpv2`, with the chain riding ffmpeg's `volume` (ReplayGain) / `equalizer` (EQ) / `acompressor` / `alimiter` / `dynaudnorm` (DSP) / `silenceremove` (Smart Speed) filters and `scaletempo2` for variable speed
- **`oo7`** for libsecret credential storage (HTTP Basic per-show auth)
- **`zbus`** for MPRIS2 and the suspend inhibitor
- **Meson** wrapper over Cargo for Flatpak packaging
- **Memory budget:** < 200 MB idle, < 300 MB active on a 50k-track library (see [`spec.md`](spec.md) §13)

## Building

```bash
# Native (development)
cargo build --workspace
cargo test --workspace

# CI gate (matches the portfolio)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Day to day:

```bash
# Import a folder into a database-owned library (copies by default)
cargo run -p conservatory-cli -- import library.db /path/to/album ~/Music/Conservatory

# Search it with the full grammar
cargo run -p conservatory-cli -- search library.db 'genre:ambient AND year:>=1990'

# Subscribe to a podcast and pull its episodes
cargo run -p conservatory-cli -- podcast add library.db https://example.com/feed.xml
cargo run -p conservatory-cli -- podcast refresh library.db

# Launch the browse + playback window (second arg is the library root)
cargo run -p conservatory -- library.db ~/Music/Conservatory
```

System build dependencies (Fedora 44):

```bash
sudo dnf install gtk4-devel libadwaita-devel mpv-libs-devel sqlite-devel
# The af-chain rides the full (GPL) ffmpeg build: RPM Fusion's ffmpeg-libs, not ffmpeg-free-libs
sudo dnf install --setopt=install_weak_deps=False ffmpeg-libs
```

## License

GPL-3.0-or-later. The license is forced by the GPL libraries the player links, not by a call Conservatory makes: libmpv links a GPL ffmpeg build (the `volume` / `equalizer` / `acompressor` / `dynaudnorm` / `silenceremove` filters the `af`-chain rides) and librubberband (GPL-2-or-later) where the build carries it. As of Phase 6c the app no longer invokes the `rubberband` filter itself (Smart Speed is `silenceremove`, variable speed is `scaletempo2`), but the obligation flows from linking the stack (spec §15). See [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) for the full chain.
