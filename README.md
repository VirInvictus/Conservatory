<p align="center">
  <img src="logo.svg" alt="Conservatory" width="240">
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Language-Rust-blue" alt="Language: Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg" alt="License: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/GNOME-50%2B-4a86cf" alt="GNOME 50+">
  <img src="https://img.shields.io/badge/status-v0.0.23%20%C2%B7%20Phase%205%20complete-orange" alt="Status: v0.0.23, Phase 5 complete">
</p>

---

# Conservatory

**Calibre for audio.**

A native GNOME library manager that owns and organizes your music and podcasts on disk, presented through a foobar2000 Columns UI browse surface and played through a libmpv daily-driver engine that runs both media types from a single queue. v0.0.13: the manager is usable headless (import, organize, shelf-genre resolution, and crash-safe file moves with dry-run + undo), the GTK Columns UI browse window is a working library browser (sortable track list, filter bar wired to the Calibre-style search grammar, saved Perspectives), and it now plays music: double-click a track to play the visible list through the threaded libmpv engine (gapless + ReplayGain), with a persistent Now-bar transport at the bottom. The workspace is structured around compile-time plugins with music as the native program.

## Why this exists

Linux has players and it has taggers, but it has nothing that manages a music collection as a database the way Calibre manages books. deadbeef and friends play files in place but leave organization to you. Lattice (my own) audits and reports but treats the filesystem as canonical and never writes. Beets organizes from a terminal but is not a daily-driver player. And podcasts live in a separate app entirely, with their own queue, their own playback engine, their own idea of what "next" means. Conservatory is the one app that owns the library, browses it like foobar2000, plays it like a real player, and puts a podcast episode and an album track in the same queue.

Four commitments, in priority order:

1. **The database owns the library.** SQLite is the source of truth for organization and curated metadata; the app owns the on-disk layout and moves files to match it (`Genre / Album Artist / Album /` by default). This inverts the filesystem-canonical stance of Lattice and Belfry, the way Calibre takes a book and files it under its author tree. That trust is spent carefully: dry-run preview, an undo journal, and embedded-tag write-back so files stay portable and self-describing.
2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in deadbeef-cui), backed by the full Calibre-style search expression grammar.
3. **One engine, one queue, two media types.** Music tracks and podcast episodes share a single libmpv engine and a single play queue. Each item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for an episode.
4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen, replacing deadbeef. Gapless, head-staged ReplayGain, a DSP chain (EQ, compressor/limiter, leveler), output-device selection, MPRIS, media keys. Required, not optional: because Conservatory moves files, any external player's in-place references go stale the moment a library is re-shelved.

## Absorbs Belfry

Conservatory absorbs Brandon's podcast client, Belfry. Belfry's Phase 1 work is not discarded: its single-writer SQLite worker is the exact pattern this app needs and migrated here, and its audio engine (Smart Speed, Voice Boost) and Inbox → Queue → Played triage model became the Podcasts side. The one casualty is Belfry's filesystem-canonical design; in Conservatory, podcasts become app-managed downloads, acceptable for ephemeral episodes in a way it would not be for a curated music collection. **Conservatory reached podcast parity at v0.0.52 (Phase 6c, the sleep timer the last piece), so Belfry is now retired** (spec §16.8): its repo is archived and the podcast subsystem lives entirely here.

**Author's Note:** I'm a college student in my late thirties with no professional industry experience yet; Conservatory is one in a string of native Linux desktop apps I'm building to learn the craft and assemble a portfolio. I came from foobar2000 and Directory Opus, and I keep a large Calibre library. What Calibre does for my books, nothing does for my music. Conservatory is the manager-and-player I want to exist. I work on Fedora 44 on a ThinkPad T14s AMD Gen 6; that's the environment it'll be tested against. I welcome contributions but can only honestly support my own setup.

## Status

v0.0.23, Phase 5 complete. Phases 1 (data layer), 2 (import/organize), 3 (browse), and 4 (playback) are done — a daily-driver music player — and Phase 5 adds the full editing/maintenance surface: edit metadata (CLI + GUI), write it back into the files, scan ReplayGain, and manage cover art. The managed tree is laid out as `Music/ | Audiobooks/ | Podcasts/` under the library root (spec §5.1):

- **Phase 1** — single-writer SQLite worker, read-only pool, numbered migrations, the music schema with FTS5, the embedded-tag reader (`lofty`), and median-cut cover accents.
- **Phase 2 — the manager is usable headless.** Point the CLI at a folder and get an organized, database-owned library: tag read → resolve → shelf-genre derivation → path-template render → crash-safe move (dry-run preview, undo journal, roll-forward recovery). Verbs: `import`, `organize`, `shelf-genre-set`.
- **Phase 3a** — `conservatory-search`, the Calibre-style expression grammar (lex → parse → eval + all-or-nothing SQL translate, bm25 + recency ranking), exposed as `conservatory-cli search`.
- **Phase 3b** — the first GTK4/libadwaita code: the deadbeef-cui faceted browse window (Genre → Album Artist → Album panes + a track table), with facet logic kept headless in core.
- **Phase 3c — a working library browser.** A sortable, multi-select track list (Artist | Album | Genre | Title | Duration | Rating); the always-on filter bar (`Ctrl+F`) wired to the grammar, intersected with the facets; and Perspectives (named saved searches) in a sidebar, persisted through the single-writer worker now stood up in the GUI.
- **Phase 4a — the libmpv playback host + music profile.** Plays a track through libmpv with gapless and ReplayGain (mpv-native, read-only; no EQ/DSP yet), persists position on the 30 s insurance interval, and resumes the saved cursor across a restart. The engine, profile resolution, and state logic live in core.
- **Phase 4b-i — the unified queue + threaded engine (headless).** A `queue` table and a threaded `Player` (the libmpv host moved onto its own thread behind a `Send` handle) that advances item to item, applying each track's profile, persisting position and play counts, and resuming from the cursor. `conservatory-cli queue add|list|remove|clear` and `play <db> <root>` drive it; `is:queued` search is now live.
- **Phase 4b-ii-a — the player in the GUI + Now-bar.** Double-click (or press Enter on) a track to play the visible list from there; a persistent bottom Now-bar shows what's playing with a working transport (play/pause, prev/next, seek, volume), polled from the engine. Launch with `conservatory <db> <root>`.
- **Phase 4b-ii-b — the drag-and-drop queue drawer.** A right-side slide-in drawer (`Ctrl+U`) lists the playing queue with the current track highlighted; drag rows to reorder (or `Alt+↑/↓`), `Delete` to remove, `Ctrl+Shift+C` to clear. Editing the queue never restarts the current track, and the DB queue stays the source of truth.
- **Phase 4b-ii-c — queue polish.** The saved queue resumes paused at the cursor on launch (reopen and pick up where you left off), and `Ctrl+Enter` appends the browse selection to the queue.
- **Phase 4c-i — MPRIS2 + suspend inhibitor.** Serves `org.mpris.MediaPlayer2` so the keyboard media keys, the GNOME media overlay, and the lock screen drive playback and show the track; a logind inhibitor keeps the machine awake while playing.
- **Phase 4c-ii — output-device picker.** A header menu lists the audio sinks (PipeWire/Pulse/ALSA, plus `auto`) and switches mpv's output live. **Phase 4 — the daily-driver player — is complete.**
- **Phase 5a-i — headless metadata editing.** Edit fields across a search selection from the CLI: `tag set <db> '<expr>' field=value...` and `tag replace <db> '<expr>' field find replace`. Track, album, and raw-genre fields; path-affecting edits (album / album artist / year / shelf genre) re-shelve files through the Phase 2c mover (dry-run + undo).
- **Phase 5a-ii — the bulk-edit dialog.** Select tracks and press `Ctrl+E` (or the header pencil) to edit fields across the selection; path-affecting edits move files behind a confirm-with-preview. **Phase 5a — metadata editing — is complete.**
- **Phase 5b — embedded-tag write-back.** `embed-tags <db> '<expr>' --root <root> [--apply]` writes the curated DB metadata back into the files (dry-run shows the per-file diffs first), so the managed tree stays self-describing and a wipe-and-reimport reconstructs it (§5.6); in the GUI, the header save button embeds the selection behind a confirm.
- **Phase 5c — ReplayGain scan.** `replaygain scan <db> '<expr>' --root <root> --apply` computes and writes ReplayGain (via `rsgain`, all formats incl. Opus) and refreshes the DB so playback normalizes untagged albums.
- **Phase 5d — cover art to disk.** Import writes each album's `cover.jpg` and records it; edits/organize move it with the album; the Now-bar shows the thumbnail and MPRIS exposes `mpris:artUrl`. `set-cover <db> <album_id> <image> --root` sets a cover. **Phase 5 is complete.**

(This Status list stops at Phase 5; Phase 5.5 (the audio engine: EQ, DSP, output quality) and Phase 6 (podcasts, absorbing Belfry) have since shipped, the latter through v0.0.52, at which point Belfry was retired. A fuller Status refresh is pending.) Next on the roadmap: Phase 7 (audiobooks) or the independent Phase 8 audits.

- [`spec.md`](spec.md) — the design contract.
- [`roadmap.md`](roadmap.md) — the phased plan, broken into independently shippable sub-phases.
- [`patchnotes.md`](patchnotes.md) — release notes (newest at top).
- [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) — design lineage, dependency licenses, and the GPL-3-via-rubberband chain.
- [`docs/`](docs/) — design references: [schema](docs/schema.md), [import](docs/import.md), [path templates](docs/path-template.md), [genre normalization](docs/genre-normalization.md), [file mover](docs/mover.md), [cover accent](docs/accent.md), [search grammar](docs/search-grammar.md), [libmpv profiles](docs/libmpv-profiles.md), [keymap](docs/keymap.md).

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
- **Tag read/write** via `lofty` (signed off over `symphonia`, spec §7.1); **`image`** for cover decode and accent extraction
- **libmpv** via `libmpv2` + ffmpeg's `silenceremove` / `acompressor` / `equalizer` / `loudnorm` / `rubberband` filters
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

The headless manager works today:

```bash
# Import a folder into a database-owned library (copies by default)
cargo run -p conservatory-cli -- import library.db /path/to/album ~/Music/Conservatory

# Search it with the full grammar
cargo run -p conservatory-cli -- search library.db 'genre:ambient AND year:>=1990'

# Launch the (work-in-progress) browse window
cargo run -p conservatory -- library.db
```

System build dependencies (Fedora 44):

```bash
sudo dnf install gtk4-devel libadwaita-devel mpv-libs-devel sqlite-devel
# For Smart Speed (rubberband filter): RPM Fusion's ffmpeg-libs (not ffmpeg-free-libs)
sudo dnf install --setopt=install_weak_deps=False ffmpeg-libs
```

## License

GPL-3.0-or-later. The license is forced by librubberband's GPL-2-or-later via the absorbed Smart Speed chain (spec §15); no relaxation is possible without replacing rubberband. See [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) for the full chain.
