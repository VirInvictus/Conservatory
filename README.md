<p align="center">
  <img src="logo.svg" alt="Conservatory" width="240">
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Language-Rust-blue" alt="Language: Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-GPL--3.0--or--later-blue.svg" alt="License: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/GTK-4.14%2B-4a86cf" alt="GTK 4.14+">
  <img src="https://img.shields.io/badge/status-v0.3.6%20%C2%B7%20daily%20driver-brightgreen" alt="Status: v0.3.6, daily driver">
</p>

---

# Conservatory

**Calibre for audio.** A native GTK4 desktop app that owns and organizes your music, podcasts, and audiobooks on disk, browses them like foobar2000, and plays them from one queue.

The database is the source of truth: Conservatory files your audio into a tidy tree, the way Calibre files your books, and plays it back through a libmpv engine good enough to replace deadbeef as your daily player. A music track, a podcast episode, and an audiobook chapter can sit next to each other in the same queue. That mixed queue is the whole point.

## Table of contents

- [What you get](#what-you-get)
- [Why it exists](#why-it-exists)
- [Installing and building](#installing-and-building)
- [Quick start](#quick-start)
- [Keyboard shortcuts](#keyboard-shortcuts)
- [How it is built](#how-it-is-built)
- [Project status](#project-status)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## What you get

**A library manager.** Point it at a folder and get an organized, database-owned library. It reads the embedded tags, resolves a single shelf genre, renders a path from a template (`Genre / Album Artist / Album /` by default), and moves the files to match. Every move is dry-run previewed, journaled for undo, and roll-forward recoverable after a crash. Curated metadata is written back into the files, so the tree stays portable and a wipe-and-reimport rebuilds it.

**A Columns UI browser.** The music surface is a faceted, hierarchical browser in the foobar2000 / deadbeef-cui style: Genre into Album Artist into Album panes (the number of panes is configurable), a sortable multi-select track list with album-art thumbnails, and an always-on filter bar driven by the full Calibre-style search grammar (`genre:ambient AND year:>=1990`, hierarchical tags, virtual libraries, boolean logic). Saved Perspectives keep your favourite views a click away.

**A real player, not a previewer.**

- Gapless playback and head-staged per-track ReplayGain
- A live audio chain: a 10-band graphic EQ with presets, plus a DSP rack (compressor, brick-wall limiter, leveler)
- Output-device and resampler selection
- A persistent transport bar, a drag-and-drop queue, launch-resume paused where you left off
- MPRIS2, media keys, headset buttons, and a sleep-suspend inhibitor while playing

**A podcast client.** A full one, not a bolt-on. Conditional-GET fetching, OPML import and export, per-show credentials, and downloads; an Inbox into Queue into Played triage tab; episodes that play in the same queue as your music and resume to the second; per-show speed, Smart Speed (silence skipping), and Voice Boost; chapters, time-saved accounting, and a sleep timer.

**Audiobooks.** The third tab, modeled on Cozy and Audiobookshelf: a cover-grid shelf, per-book resume, chapter navigation, bulk metadata editing, and the same spoken-word engine the podcasts use (variable speed, Smart Speed, Voice Boost). A whole book is one item in the queue; an M4B or a folder of chapter files both work.

**Library health tools.** A read-only audit suite (run from the CLI): integrity and decode checks (`verify`), four-tier duplicate detection (`duplicates`), tag and ReplayGain and cover-art audits (`audit`), library statistics (`stats`), stray APE-tag detection and stripping (`apestrip`), and `.m3u` playlist export and import.

**A look you will not want to close.** A fixed Kanagawa Dragon theme, album art across the browse and an accent colour pulled from each cover, an informative now-playing bar, a Now Playing drawer with a real-time spectrum visualizer, and a Preferences window backed by a plain `config.toml`.

## Why it exists

Linux has players and it has taggers, but nothing manages a music collection as a database the way Calibre manages books. deadbeef and friends play files in place and leave the organizing to you. Beets organizes from a terminal but is not a daily-driver player. And podcasts live in a separate app entirely, with their own queue, their own engine, their own idea of what "next" means. Conservatory is the one app that owns the library, browses it like foobar2000, plays it like a real player, and puts a podcast episode and an album track in the same queue.

Four commitments, in priority order:

1. **The database owns the library.** SQLite is the source of truth for organization and curated metadata; the app owns the on-disk layout and moves files to match it. That trust is spent carefully: dry-run preview, an undo journal, and embedded-tag write-back so files stay self-describing.
2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database, backed by the full Calibre-style search expression grammar.
3. **One engine, one queue, three media types.** Music tracks, podcast episodes, and audiobooks share a single libmpv engine and a single play queue. Each item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for a spoken-word one.
4. **A daily-driver player.** For libraries Conservatory manages, it is the place you listen. This is not optional: because Conservatory moves files, an external player's in-place references go stale the moment a library is re-shelved.

> Conservatory absorbed Brandon's earlier podcast client, **Belfry**: its single-writer SQLite worker, its Smart Speed / Voice Boost engine, and its Inbox into Queue into Played triage all live here now. Belfry is retired; the podcast story lives entirely in Conservatory.

## Installing and building

Conservatory runs from source. There is no packaged release yet.

System build dependencies (Fedora 44):

```bash
sudo dnf install gtk4-devel mpv-libs-devel sqlite-devel pipewire-devel
# The audio chain rides the full (GPL) ffmpeg build: RPM Fusion's ffmpeg-libs, not ffmpeg-free-libs
sudo dnf install --setopt=install_weak_deps=False ffmpeg-libs
```

Build and test:

```bash
cargo build --workspace
cargo test --workspace

# The CI gate (matches the portfolio):
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

A music-only build (no podcast or audiobook code compiled in) is `--no-default-features`; CI keeps it green alongside the full build.

## Quick start

```bash
# Import a folder into a database-owned library (copies by default; --move consumes the originals)
cargo run -p conservatory-cli -- import library.db /path/to/album ~/Music/Conservatory

# Search it with the full grammar
cargo run -p conservatory-cli -- search library.db 'genre:ambient AND year:>=1990'

# Subscribe to a podcast and pull its episodes
cargo run -p conservatory-cli -- podcast add library.db https://example.com/feed.xml
cargo run -p conservatory-cli -- podcast refresh library.db

# Audit the library
cargo run -p conservatory-cli -- verify library.db
cargo run -p conservatory-cli -- duplicates library.db

# Launch the browse + playback window (the second argument is the library root)
cargo run -p conservatory -- library.db ~/Music/Conservatory
```

The CLI mirrors the GUI: every non-graphical action has a verb, which is how the engine stays testable. Run `conservatory-cli --help` for the full set.

## Keyboard shortcuts

A few of the most useful; the [full keymap](docs/keymap.md) has the rest.

| Key | Action |
|-----|--------|
| `Ctrl+F` | Focus the filter bar (the full search grammar) |
| `Alt+1` / `Alt+2` / `Alt+3` | Switch Music / Podcasts / Audiobooks |
| `Ctrl+U` | Show or hide the queue drawer |
| `Ctrl+P` | Show or hide the track-properties inspector |
| `Ctrl+I` | Show or hide the Now Playing drawer (with the spectrum) |
| `Ctrl+E` | Edit metadata for the selection (bulk editor) |
| `Ctrl+Enter` | Append the selection to the queue |
| `Ctrl+M` | Stop after the current item |
| `Ctrl+J` | Jump to the playing track |
| `S` | Sleep-timer menu |
| `Ctrl+,` | Preferences |

## How it is built

Six crates, on the discipline that every non-GUI surface stays CLI-testable. Music is the native program; podcasts and audiobooks are **compile-time plugins** (feature-gated crates, on by default), with all schema living in core's single migration ledger.

- `conservatory-core`: the headless data layer and the music engine: SQLite worker, all migrations, the import pipeline, the file mover, the playback host and profiles, and the unified queue.
- `conservatory-search`: the Calibre-shaped search expression language (see [`docs/search-grammar.md`](docs/search-grammar.md)).
- `conservatory-podcasts`: the absorbed Belfry podcast subsystem.
- `conservatory-audiobooks`: the audiobook subsystem.
- `conservatory-cli`: the headless binary.
- `conservatory`: the GTK4 binary, on plain GTK4 with its own flat Kanagawa Dragon stylesheet (spec §2.4).

**Stack:** Rust 2024, GTK 4.14+ (plain GTK4, with an owned flat Kanagawa Dragon stylesheet in place of a theming toolkit), SQLite via `rusqlite` (bundled, FTS5, single-writer worker + read pool + WAL), `tokio`. libmpv via `libmpv2`, with the audio chain riding ffmpeg's `volume` / `equalizer` / `acompressor` / `alimiter` / `dynaudnorm` / `silenceremove` filters and `scaletempo2` for variable speed. `lofty` for tag read and write, `image` for cover decode and accent extraction, `reqwest` + `feed-rs` + `quick-xml` + `ammonia` for podcasts, `oo7` for libsecret credentials, `zbus` for MPRIS2. The spectrum visualizer taps Conservatory's own output node through `pipewire` (not the whole device, so other apps do not move the bars) and runs its own FFT with `realfft`. Meson wraps Cargo for Flatpak packaging. Memory budget: under 200 MB idle, under 300 MB active on a 50k-track library (spec §13).

## Project status

**v0.3.6. A daily-driver music player, a full podcast client, and an audiobook player in one app.** The managed tree is laid out as `Music/ | Podcasts/ | Audiobooks/` under the library root.

Shipped, by phase (the [roadmap](roadmap.md) carries the sub-phase detail, the [patchnotes](patchnotes.md) the per-release notes):

- **1 to 3:** the data layer, the headless manager (import / organize / move), and the Columns UI browser with the search grammar.
- **4 and 5:** the libmpv player and unified queue; bulk editing, embedded write-back, and ReplayGain scanning.
- **5.5:** the audio engine (EQ, DSP rack, output quality) behind a Sound preferences page.
- **6:** podcasts, absorbing Belfry (fetch, triage, Smart Speed, Voice Boost, chapters, sleep timer).
- **7:** audiobooks (shelf, import, per-book resume, chapter navigation, bulk edit).
- **8:** the library maintenance suite (integrity, duplicates, audits, stats, APE stripping, playlists).
- **10:** the `config.toml`-backed Preferences window.
- **11:** Columns UI polish (the properties inspector, the status bar, the Now Playing drawer, transport conveniences).
- **12:** the visual identity (Kanagawa Dragon theme, album art across the browse, an enriched now-bar, the spectrum visualizer).
- **13:** the sleekness pass (a layout fix, empty states, toasts, an internal tidy, bundled typography, and deadbeef-cui browser parity).
- **14:** a `--debug` diagnostic mode (SQL, IO, network, and memory to stderr on filterable channels). See [`docs/debugging.md`](docs/debugging.md).
- **17:** player table-stakes (shuffle, repeat, context-aware ReplayGain, queue-vs-playlist clarity).
- **18** (the `0.2.0` milestone): accent-insensitive search and configurable browse columns.
- **26** (the `0.3.0` milestone): the Hyprland-native redesign onto plain GTK4 with an owned flat Kanagawa Dragon stylesheet, no external theming toolkit; see [`docs/hyprland.md`](docs/hyprland.md).
- **9** (optional, off by default): listening-history scrobbling to ListenBrainz or Last.fm, a local-first one-way outbox, enabled and configured in Preferences → Sync. Full scrobbler behaviour: a now-playing indicator on the service, and the standard submission rule (a 30-second floor, then half the track or four minutes), stamped with the play's start time.
- **19** (in progress): the waveform seek bar (a loudness-envelope scrubber in the transport bar).

Not built yet: the rest of **Phase 19** (drag-and-drop import, a full-screen Now Playing surface) and the 1.0 endgame (real-library verification and Flatpak packaging). The roadmap has the full picture.

## Documentation

- [`spec.md`](spec.md): the design contract. Read it before changing semantics.
- [`roadmap.md`](roadmap.md): the phased plan, in independently shippable sub-phases.
- [`patchnotes.md`](patchnotes.md): release notes, newest at top.
- [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md): design lineage, dependency licenses, and the GPL-3 chain analysis.
- [`docs/`](docs/): design references for [schema](docs/schema.md), [import](docs/import.md), [path templates](docs/path-template.md), [genre normalization](docs/genre-normalization.md), [the file mover](docs/mover.md), [cover accent](docs/accent.md), [the search grammar](docs/search-grammar.md), [libmpv profiles](docs/libmpv-profiles.md), [the audiobook reader](docs/audiobook-reader.md), [the theme](docs/theme.md), [debugging](docs/debugging.md), and the [keymap](docs/keymap.md).

## Contributing

I'm a college student in my late thirties with no professional industry experience yet; Conservatory is one in a string of native Linux desktop apps I'm building to learn the craft and assemble a portfolio. I came from foobar2000 and Directory Opus, and I keep a large Calibre library. What Calibre does for my books, nothing did for my music, so I am building it.

I develop on Fedora 44 on a ThinkPad T14s AMD Gen 6, and that is the only environment I can honestly support. Contributions are welcome, but please understand that I can only test against my own setup.

When something misbehaves, run either binary with `--debug` for a verbose diagnostic stream (SQL with timings, file IO, network requests, and memory) on stderr. [`docs/debugging.md`](docs/debugging.md) covers the four channels and how to narrow them with `RUST_LOG`.

## License

GPL-3.0-or-later. The license is forced by the GPL libraries the player links, not by a call Conservatory makes: libmpv links a GPL ffmpeg build (the filters the audio chain rides), and librubberband (GPL-2-or-later) where the build carries it. See [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md) for the full chain.

## Support

If Conservatory's useful to you and you'd like to chip in:

```
bc1qkge6zr45tzqfwfmvma2ylumt6mg7wlwmhr05yv
```
