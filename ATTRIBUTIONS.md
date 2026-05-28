# Attributions

Design lineage, dependency licenses, and the GPL chain analysis for Conservatory. This file grows as dependencies are signed off and added (spec §11).

## Design lineage

Conservatory borrows patterns, not code, from several projects. Where it ports the *shape* of an idea, the implementation is written fresh so the projects evolve without coupling.

- **Calibre** (Kovid Goyal et al., GPL-3.0) — the library-as-database model, file ownership, and the save-to-disk path template that makes the on-disk tree a render of the database. Conservatory's path-template engine (spec §5.1) and shelf-genre field (the `author_sort` trick, spec §5.2) are modeled on Calibre's approach.
- **beets** (Adrian Sampson et al., MIT) — the `lastgenre` canonicalization model (curated whitelist plus tree) informs the shelf-genre normalization (spec §5.2).
- **deadbeef-cui** (Brandon LaRocque) — the faceted, multi-pane Columns UI browse layout, lifted from a DeaDBeeF plugin into a first-class window.
- **Belfry** (Brandon LaRocque) — absorbed wholesale: the podcast fetch pipeline, Smart Speed and Voice Boost engine, and the Inbox to Queue to Played triage model. `belfry-core`'s single-writer worker is ported, not rewritten.
- **Atrium** and **Viaduct** (Brandon LaRocque) — the single-writer SQLite worker pattern and the search-expression grammar shape. `conservatory-search` ports the *shape* of `atrium-search` but is an independent implementation (the Belfry precedent).
- **Hermitage** (Brandon LaRocque) — cover art as the visual unit, with a per-album accent extracted from cover hue via a median-cut quantizer.

## License: GPL-3.0-or-later

Conservatory is GPL-3.0-or-later. This is forced, not chosen: the absorbed Smart Speed chain (spec §6.3) uses librubberband for pitch-preserving time-stretch, and librubberband is GPL-2.0-or-later. The combined work is therefore GPL. This is the same constraint Belfry documents.

No license relaxation is possible without proposing a replacement for the rubberband dependency in the Smart Speed filter graph.

## Dependency licenses

Recorded here as dependencies are signed off (spec §11) and added to the workspace. The Phase 0 skeleton pulls no third-party crates yet.

| Crate / library | License | Used for | Phase |
|---|---|---|---|
| _(none yet)_ | | | |

System libraries planned: `libmpv` (with the ffmpeg filter library: `silenceremove`, `rubberband`, `acompressor`, `equalizer`, `loudnorm`), `gtk4`, `libadwaita`, `libsecret` (via `oo7`). Full per-dependency license notes will be filled in as each lands.
