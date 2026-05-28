# Conservatory Roadmap

Phasing is deliberate and hard (spec §17): each stage must be usable on its own, so attention can swing back to Atrium between phases without leaving Conservatory half-built. The manager half (Phases 1 to 3) must be usable before the player half is finished, and the player must be usable before podcasts arrive.

## Phase 0 — Design and bootstrap ✅

- [x] Design contract (`spec.md`).
- [x] Workspace skeleton: four crates, portfolio docs, GPL-3 license, build files.
- [x] Build deferral lifted; build proceeds concurrently with Atrium (spec §16.1, §17).

## Phase 1 — `conservatory-core` foundation

- [ ] Single-writer SQLite worker + read-only pool (port the pattern from `belfry-core`).
- [ ] WAL, foreign keys, `synchronous=NORMAL`; migrations versioned via `user_version`.
- [ ] Music data model (artists, albums, tracks, genres) and FTS5 scaffolding.
- [ ] Tag read (lofty or symphonia, decided here per spec §7.1).
- [ ] Fixtures + the first integration suite.

## Phase 2 — Import and organize

- [ ] Path-template engine (spec §5.1).
- [ ] Shelf-genre resolver with normalization + priority chain (spec §5.2).
- [ ] File mover with dry-run preview, undo journal, crash-safe replay (spec §5.4).
- [ ] Embedded-tag write-back (spec §5.5).
- [ ] CLI: `import`, `organize`, `tag`, `shelf-genre`. The manager is usable headless here.

## Phase 3 — GTK browse

- [ ] Columns UI faceted view over the database (the deadbeef-cui layout, first-class).
- [ ] `conservatory-search` grammar: lex / parse / AST / evaluator / SQL translator.
- [ ] Sortable track list, multi-select, saved Perspectives. A working library browser.

## Phase 4 — Playback

- [ ] libmpv engine (`libmpv2`), music profile (gapless, ReplayGain, crossfade).
- [ ] Unified queue + `PlayableItem`, Now-bar, queue view.
- [ ] MPRIS2, media keys, PipeWire sink picker, suspend inhibitor. A daily-driver music player.

## Phase 5 — Bulk editing

- [ ] Multi-select bulk metadata editing, search-and-replace across fields.
- [ ] Edits that change shelf genre or path trigger a move job (dry-run + undo).

## Phase 6 — Podcasts (absorb Belfry)

- [ ] Port `belfry-core` podcast fetch/parse (conditional GET, `podcast:` namespace, OPML).
- [ ] Podcasts tab: Inbox to Queue to Played triage, per-show overrides.
- [ ] Smart Speed + Voice Boost playback profile (the librubberband chain).
- [ ] Episodes share the unified queue with tracks.
- [ ] **Podcast parity reached. Belfry can retire** (spec §16.8). Update the `~/.gitrepos` project map and archive/retire the Belfry repo.
