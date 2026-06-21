# Attributions

Design lineage, dependency licenses, and the GPL chain analysis for Conservatory. This file grows as dependencies are signed off and added (spec §11).

## Design lineage

Conservatory borrows patterns, not code, from several projects. Where it ports the *shape* of an idea, the implementation is written fresh so the projects evolve without coupling.

- **Calibre** (Kovid Goyal et al., GPL-3.0) — the library-as-database model, file ownership, and the save-to-disk path template that makes the on-disk tree a render of the database. Conservatory's path-template engine (spec §5.1) and shelf-genre field (the `author_sort` trick, spec §5.2) are modeled on Calibre's approach.
- **beets** (Adrian Sampson et al., MIT) — the `lastgenre` canonicalization model (curated whitelist plus tree) informs the shelf-genre normalization (spec §5.2).
- **deadbeef-cui** (Brandon LaRocque) — the faceted, multi-pane Columns UI browse layout, lifted from a DeaDBeeF plugin into a first-class window.
- **Belfry** (Brandon LaRocque) — absorbed wholesale: the podcast fetch pipeline, Smart Speed and Voice Boost engine, and the Inbox to Queue to Played triage model. `belfry-core`'s single-writer worker is ported, not rewritten.
- **Viaduct** (Brandon LaRocque) — the single-writer SQLite worker plus read-only pool (the panic-restart receive loop, per-op `oneshot` replies), the main-thread coalescing-delta primitives (`CoalescingQueue` + `BatchUpdate`), the shared `reqwest` client baseline (used by the Phase 6 podcast fetcher), and the `mem_check` memory-checkpoint harness. Conservatory does **not** adopt Viaduct's `CREATE TABLE IF NOT EXISTS` schema setup; it uses numbered `user_version` migrations (the Atrium discipline) because the library is the user's irreplaceable data, not a regenerable cache. Viaduct's structured-condition Smart Feeds are *not* the Perspectives model (Conservatory's Perspectives are saved text, the Calibre / Atrium model).
- **Atrium** (Brandon LaRocque) — the single-writer worker pattern (shared with Viaduct) and the search-expression grammar shape. `conservatory-search` ports the *shape* of `atrium-search` (lex / parse / typed AST / eval + the all-or-nothing SQL-translate dual path, forgiving degrade-to-substring, sort-as-metadata, bm25 + recency ranking) but is an independent implementation (the Belfry precedent).
- **CalibreQuarry** (Brandon LaRocque) — the domain *semantics* of the search engine, ported in turn from Calibre: datatype-dispatched matching, multi-valued (`text_multi`) genre faceting, numeric/date relational operators and presence tests, and the composable saved-search (`vl:` virtual library) model with cycle detection that `conservatory-search` reuses for Perspectives.
- **Hermitage** (Brandon LaRocque) — cover art as the visual unit, with a per-album accent extracted from cover hue via a median-cut quantizer.
- **Cozy** (Julian Geywitz et al., GPL-3.0-or-later) — the audiobook half (spec §3.8, §4.5, §5.7). The closest architectural sibling: a GTK4 / libadwaita audiobook player for Linux, peewee over SQLite, MVVM (`model/` / `view_model/` / `ui/`). Conservatory models its audiobook data shape (Book → Chapter → file), import/scan, and the Audiobooks browse surface on Cozy. The one part that does not transfer is Cozy's GStreamer player layer; Conservatory plays audiobooks through the same libmpv engine as music and podcasts.
- **Audiobookshelf** (advplyr et al., GPL-3.0) — the audiobook *metadata model* and organization conventions (spec §4.5, §5.7, §7.5): authors vs narrators as distinct roles, series with a decimal sequence ("Book 1.5"), subtitle / publisher / ISBN / ASIN, the `Author/Series/Title (Year)/` folder layout, the `desc.txt` / `reader.txt` / `.opf` / `cover.jpg` sidecar conventions, and chapter extraction (embedded M4B markers vs multi-file). Its online metadata-provider abstraction (Audible / Audnexus / Google Books) is noted but deferred (spec §16.10).
- **m4b-tool** (Andreas Gohr / sandreas, MIT) — the *technique* reference for audiobook chapter handling: merging a multi-file book into a single chaptered M4B, splitting by chapter, and chapterizing via silence detection (a thin wrapper over ffmpeg / mp4v2). Not linked or ported; it informs the optional chapter-merge feature left open in spec §16.11.

> **Reference checkouts.** Cozy (`geigi/cozy`), Audiobookshelf (`advplyr/audiobookshelf`), and m4b-tool (`sandreas/m4b-tool`) are cloned locally under `~/.gitrepos/{cozy,audiobookshelf,m4b-tool}/` as read-only reference (the same status as `calibre/`), alongside the prior-art checkouts the earlier projects already lean on. They are for reading, not editing. All three licenses are compatible with Conservatory's GPL-3.0-or-later.

## License: GPL-3.0-or-later

Conservatory is GPL-3.0-or-later. This is forced, not chosen: the absorbed Smart Speed chain (spec §6.3) uses librubberband for pitch-preserving time-stretch, and librubberband is GPL-2.0-or-later. The combined work is therefore GPL. This is the same constraint Belfry documents.

No license relaxation is possible without proposing a replacement for the rubberband dependency in the Smart Speed filter graph.

## Dependency licenses

Recorded here as dependencies are signed off (spec §11) and added to the workspace. All were already part of the agreed workspace dependency catalog; Phase 1a is where the first of them are actually pulled in by `conservatory-core` and `conservatory-cli`.

| Crate / library | License | Used for | Phase |
|---|---|---|---|
| `tokio` | MIT | Async runtime: the single-writer worker's `spawn_blocking` task, `mpsc` command channel, and per-op `oneshot` replies | 1a |
| `rusqlite` | MIT | SQLite bindings (the `bundled` SQLite library is public domain) for the writer connection, read-only pool, and migration runner | 1a |
| `thiserror` | MIT OR Apache-2.0 | The `conservatory-core` error enum and its channel-failure conversions | 1a |
| `tracing` | MIT | Worker-loop instrumentation (per-command timing, panic-restart logging) | 1a |
| `clap` | MIT OR Apache-2.0 | `conservatory-cli` argument parsing (the `debug-roundtrip` verb) | 1a |
| `anyhow` | MIT OR Apache-2.0 | Error context in the CLI binary | 1a |
| `tempfile` | MIT OR Apache-2.0 | Test-only: temporary databases in the integration and unit suites | 1a (dev) |
| `lofty` | MIT OR Apache-2.0 | Embedded-tag read (`read_track` into the import draft); the write side feeds 5b write-back | 1c |
| `image` | MIT OR Apache-2.0 | Cover-art decode (jpeg/png) for the median-cut accent (`compute_accent`) | 1c |
| `regex` | MIT OR Apache-2.0 | `~regex` match in `conservatory-search`'s in-memory evaluator | 3a |
| `chrono` | MIT OR Apache-2.0 | Date keywords/precision in `conservatory-search` (also used by core for timestamps) | 1a / 3a |
| `gtk4` | MIT (bindings); GTK4 lib LGPL-2.1-or-later | The GUI toolkit for the browse window and all later GTK surfaces | 3b |
| `libadwaita` | MIT (bindings); libadwaita lib LGPL-2.1-or-later | GNOME/Adwaita application window, header bar, and widgets | 3b |
| `libmpv2` | MIT (bindings); libmpv lib GPL-2.0-or-later / LGPL-2.1-or-later | The playback host (`player::host::MpvHost`): one libmpv instance, property API + input commands. Signed off over the unmaintained `libmpv-rs`; `symphonia` was not a candidate (decode-only, no filter graph for the Phase 6c spoken-word chain) | 4a |

System libraries: `libmpv` (Phase 4a; built with the ffmpeg filter library `silenceremove`, `rubberband`, `acompressor`, `equalizer`, `loudnorm` for the Phase 6c spoken-word profile; on Fedora that means RPM Fusion's `ffmpeg-libs`, not `ffmpeg-free-libs`), `gtk4`, `libadwaita`, and `libsecret` (via `oo7`, Phase 6). libmpv links ffmpeg/librubberband, the GPL-forcing chain documented above; the `libmpv2` Rust bindings are MIT but the linked library carries the GPL obligation. Per-dependency notes fill in as each lands.
