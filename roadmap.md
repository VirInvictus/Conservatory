# Conservatory Roadmap

Phasing is deliberate and hard (spec §17): each stage must be usable on its own, so attention can swing back to Atrium between phases without leaving Conservatory half-built. The manager half (Phases 1 to 3) must be usable before the player half is finished, the player must be usable before podcasts arrive, and audiobooks (Phase 7) come last because they lean on the podcast engine.

Each top-level phase is split into independently shippable sub-phases (the way the Atrium and Viaduct roadmaps actually grew). A sub-phase carries its own checklist, a `Tests:` line, and a *usable artifact* exit condition: the thing that must work before the sub-phase is called done. Provenance (what each piece is ported or modeled from) is noted inline rather than in a separate section.

## Continuous (every phase)

These run alongside every phase rather than belonging to one; called out here so they are not forgotten.

- [x] CI: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on Linux, green from Phase 1 on. (Workflow landed at 1a: `.github/workflows/ci.yml`.)
- [ ] Memory checkpoint per phase: a `heaptrack` / `massif` note against the spec §13 budgets; features that miss budget get gated. The harness is ported from Viaduct's `mem_check` binary (synthetic corpus, reads `VmHWM` from `/proc/self/status`, reports pass/fail against the ceiling) and grows new checkpoints as subsystems land.
- [ ] Docs kept in step (`docs/`): schema, keymap, path-template reference, genre-normalization notes, libmpv profile reference. Each lands with the sub-phase that creates its subject.
- [ ] `ATTRIBUTIONS.md` updated as each dependency is signed off and added (spec §11), and as each ported pattern lands.

## Phase 0 — Design and bootstrap ✅

- [x] Design contract (`spec.md`).
- [x] Workspace skeleton: four crates, portfolio docs, GPL-3 license, build files.
- [x] Build deferral lifted; build proceeds concurrently with Atrium (spec §16.1, §17).

## Phase 0.5 — Plugin restructure (v0.0.2) ✅

Music is the native program; podcasts and audiobooks become **compile-time plugins**: feature-gated workspace crates, on by default, internal-only API (spec §2.2, §16.13). Landed between Phases 1b and 1c, while zero podcast/audiobook code existed, so it was a workspace/docs change rather than a code migration.

- [x] Stub plugin crates `conservatory-podcasts` (filled at Phase 6) and `conservatory-audiobooks` (filled at Phase 7), so the feature wiring exists from day one.
- [x] `podcasts` / `audiobooks` features on both binaries (default on); bare invocation prints the enabled plugin set.
- [x] CI: a `music-only` job (`--no-default-features` clippy + test on both binaries) keeps the music-only build green forever.
- [x] The boundary rule recorded (spec §2.2, §4): plugins are code and dependencies, not the database. All schema stays in core's single append-only migration ledger; the unified queue stays a core commitment.

---

## Phase 1 — `conservatory-core` foundation

The headless engine's spine. Nothing user-facing ships here, but every later phase rests on it, and the CLI smoke tests make it exercisable from 1a on.

### Phase 1a — The writer ✅

- [x] Single-writer SQLite worker: a dedicated thread owns the one writable `rusqlite::Connection`; the rest of the engine holds a `Sender<Command>` and never touches the connection directly (spec §2.1). Port the structure from `belfry-core` / Viaduct's `database/worker.rs`: a panic-catch-and-restart loop around the receive loop, and a per-op `oneshot` reply so callers await their own result.
- [x] Read-only connection pool: a small fixed pool (Viaduct runs 3) of `SQLITE_OPEN_READ_ONLY` connections with a `busy_timeout`, so timeline reads / searches / counts never queue behind a long write. Spawned after the writer has initialized the file. (Ships read-only + `busy_timeout` behind the `ReadPool` abstraction; opens a fresh handle per call, with the persistent fixed-ring deferred as a post-profiling tuning per the Belfry precedent.)
- [x] PRAGMAs: WAL, `foreign_keys = ON`, `synchronous = NORMAL`, `journal_size_limit`, a bounded `mmap_size` (spec §4). (Also `temp_store = MEMORY`.)
- [x] Migrations versioned via `user_version`, append-only and backwards-compatible post-1.0. This is the **Atrium discipline, deliberately not Viaduct's**: Viaduct sets its schema up with `CREATE TABLE IF NOT EXISTS` plus ad-hoc `ALTER TABLE`, which Conservatory does not adopt; a numbered-migration ledger is the contract here because the library is the user's irreplaceable data, not a regenerable feed cache. (Runner machinery only; the registry is empty until 1b ships migration `0001`, so `user_version` is 0 at 1a.)
- [x] Fixtures: a synthetic library builder for tests (small, deterministic, no real audio needed for the DB layer). (Deferred to 1b and delivered there: `db::fixtures` with small/medium/large scales, surfaced as the `debug-fixture` CLI verb; 1a tests used a debug `_probe` table and synthetic inline migrations.)
- [x] Tests: first integration suite. Open, migrate, write-through-worker, read-from-pool round-trip; a migration-from-vN fixture; a writer-panic-restart test.

*Usable artifact:* `conservatory-cli` (a debug verb is fine) opens the DB, applies migrations, and round-trips a row through the worker and read pool.

### Phase 1b — Music data model + FTS5 scaffolding ✅

- [x] Schema: `artists`, `albums`, `tracks`, `genres`, `track_genres`, `genre_aliases`, `genre_priority` (spec §4.1), as the first numbered migration set. (Migration `0001_initial.sql`; the runner owns `user_version`, so the file is pure DDL.)
- [x] FTS5: `track_fts` (title, artist, album) and `album_fts` (title, album artist), kept in sync by triggers (spec §4.4). Pattern follows Viaduct's trigger-synced virtual table. (Ordinary, not external-content, tables: the denormalized artist/album columns are looked up by the triggers, including on artist/album rename.)
- [x] Read helpers and the `Track` / `Album` / `Artist` models the CLI and later the GTK side consume. (`db::models` + `db::reads`: counts, lookups, `list_albums`; inserts go through the worker, `db::writes`.)
- [x] Tests: schema migration test; FTS trigger sync (insert/update/delete keeps the index correct); read-helper counts against a fixture.

*Usable artifact:* a fixture library loads into the schema; counts and basic lookups verify through the read pool. (Shipped as `conservatory-cli debug-fixture <db> --scale small|medium|large`.)

### Phase 1c — Tag read + cover/accent ✅

- [x] Dependency sign-off: `lofty` over `symphonia` for tag read (spec §7.1, §11; ATTRIBUTIONS.md). `lofty` (read + write, broad coverage) serves both the 1c read and the 5b write-back; `symphonia` is decode-only and would need a second crate for writes.
- [x] Tag reader: read embedded tags into the import draft (title, artists, album, track/disc no + totals, year, raw multi-value genres, ReplayGain, format/bitrate/sample-rate, duration). `src/tags.rs`: `read_track` → `TrackDraft`. Raw genres kept verbatim (the §5.2 decoupling); splitting is Phase 2b's job.
- [x] Cover art + accent: extract embedded cover or locate a sibling cover file, and compute the accent via a median-cut quantizer (`albums.accent_rgb` is populated at Phase 2 import; 1c computes the value headless). `src/accent.rs`, ported from Hermitage (docs/accent.md, spec §7.4). `image` signed off with jpeg + png features (webp deferred).
- [x] Tests: read committed per-format fixtures (flac/mp3/opus/m4a) into a draft (`tests/tags.rs`); accent extraction is deterministic against synthesized covers (`src/accent.rs` unit tests). Fixtures regenerable via `examples/gen_audio_fixtures.rs` (ffmpeg + lofty), CI stays hermetic.

*Usable artifact:* `conservatory-cli debug-tags <file>` reads a real audio file into a populated draft with an accent colour, headless.

---

## Phase 2 — Import and organize

The manager becomes usable headless here. This is the phase that earns the trust commitment in spec §5, so the file mover (2c) is the load-bearing sub-phase and gets the heaviest test suite.

### Phase 2a — Path-template engine ✅

- [x] Template tokenizer/renderer for the path-template string (spec §5.1); the default `{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. `src/path_template.rs`: `PathTemplate::parse` (validates braces/tokens/specs) + infallible `render(&TrackFields)`.
- [x] An album resolves to exactly one path (one shelf genre, one album artist) even when track-level tags disagree; compilations resolve the album-artist component to a **Various Artists** bucket. Per-field fallbacks keep structural folders non-empty (`Unknown` / `Various Artists` / `Unknown Album` / `Untitled`); optional pieces (year, track, disc) collapse with their literals.
- [x] Filesystem-safe rendering: per-component sanitization (path separators → `_`, reserved device names, trailing dot/space, byte cap, whitespace collapse) without leaking raw tags onto disk (docs/path-template.md).
- [x] Tests: 17 unit tests (rendering across field combinations, sanitization edge cases, Various Artists bucketing, missing-field fallbacks, parse errors, collisions) + a `track_render_rows` integration test (`tests/paths.rs`).

*Usable artifact:* `conservatory-cli debug-paths <db>` renders correct target paths for a fixture library and reports collisions. The engine is pure (`find_collisions` exposes batch collision detection for the Phase 2c mover).

### Phase 2b — Shelf-genre resolver ✅

- [x] Normalization layer: split on `;` `/` `,`, case-fold for matching, map through `genre_aliases` (spec §5.2). §16.4 settled: **empty and user-built** (no default vocabulary; the schema can seed one later without a migration). `src/shelf_genre.rs`: `normalize` keeps canonical/original casing in the output.
- [x] Priority chain: manual override → single album-level tag → most-common normalized track genre (ties broken by `genre_priority` rank, then first-seen) → `Unknown` bucket. `resolve_shelf_genre` (pure) + `resolve_album` (DB-driven).
- [x] `shelf_genre` is the only input to the genre folder level; raw `track_genres` are read but never touched (the decoupling in spec §5.2 and the CLAUDE.md hard rule).
- [x] Tests: 10 unit tests (agreeing/disagreeing/absent genres, alias-map application, priority + first-seen tie-breaks, multi-value-in-one-tag split) + a fixture-backed integration test (`tests/shelf_genre.rs`).

*Usable artifact:* `conservatory-cli debug-shelf-genre <db>` derives a stable `shelf_genre` for every album in a fixture library (matches the stored value).

### Phase 2c — File mover (dry-run + undo journal + crash-safe replay) ✅

The headline risk (spec §5.4, CLAUDE.md hard rule). Release-blocking, not nice-to-have.

- [x] Dry-run preview: `mover::plan` is pure (reads the filesystem, changes nothing); reports the operations that would run, conflicts, and in-place skips.
- [x] Undo journal written **before** the move (SQLite migration `0002`, durable via WAL) and replayed on restart (`mover::recover`, roll-forward, idempotent). Every relocation is a reversible job; `mover::undo` reverts a completed job. The crash-safety ordering is documented in docs/mover.md.
- [x] Conflict handling: duplicate target paths, missing sources, existing destinations all refuse the job (no overwrite). Cross-filesystem moves use copy → fsync → verify → delete (`fsops::relocate`). Partial-batch failure is covered by the journal + roll-forward replay.
- [x] Copy vs move is a per-job choice (`MoveMode`; copy leaves originals, move consumes them; undo deletes the copy vs moves the file back).
- [x] Tests: `tests/mover.rs` (move/undo round-trip leaves tree + DB consistent; simulated mid-move crash rolls forward cleanly; conflict refusal; copy mode; tree↔DB consistency as a §5.6 spot-check) + `fsops` unit tests (rename fast path, idempotent replay, cross-fs copy path). Full §5.6 re-import coverage lands with Phase 2d/5b.

*Usable artifact:* `conservatory-cli debug-organize <db> <root> [--apply] [--undo <id>]` plans, applies, and fully undoes a real move job; a crash between any two steps leaves a recoverable state (roll-forward on next run).

### Phase 2d — Import pipeline + CLI verbs ✅

- [x] Wire the pipeline: scan/drop → read tags (1c) → resolve into DB → derive `shelf_genre` (2b) → render target (2a) → move/copy (2c). `src/import/` (scan + resolve + two-pass pipeline; resolve in memory then a conflict pre-check before any DB write, so a refused import changes nothing). Accent (1c) computed + stored per album.
- [x] CLI: `import` (copy default, `--move`), `organize` (re-render from DB; dry-run/`--apply`/`--undo`), `shelf-genre-set` (spec §9). Output `--tsv` (default) / `--json` / `--human`. (`tag set` deferred to the Phase 5a editor; `--json` is a compact numeric summary until serde is signed off.)
- [x] Tests: `tests/import.rs` (end-to-end import of the committed per-format fixtures into a managed tree; copy keeps sources / move consumes them; re-import refused and DB unchanged; `shelf-genre-set` then `organize` moves the album) + the resolver/scan unit tests. Verified by hand against two real albums (mp3 + opus) from the library.

*Usable artifact:* **the manager is usable headless.** `conservatory-cli import <db> <folder> <root>` gives an organized, database-owned library; `organize`/`shelf-genre-set` re-shelve it.

---

## Phase 3 — GTK browse

A working library browser. The search crate (3a) is headless and could in principle ship before the GUI; it sits here because the browse surface is its first real consumer.

### Phase 3a — `conservatory-search` crate ✅

- [x] Grammar pipeline ported in *shape* from `atrium-search`, implemented independently (spec §3.4, ATTRIBUTIONS.md): `lex` → `parse` (typed AST + extracted `sort:` specs) → `ast` (round-trippable `Expr`) → `eval` (in-memory fallback) and `sql_translate` (all-or-nothing dual path: emit SQL only if every node maps, else fall back). Storage-agnostic (`SqlValue`, no rusqlite); deps `regex` + `chrono` only.
- [x] Domain semantics modeled on CalibreQuarry: datatype-dispatched matching, multi-valued `genre:` faceting, numeric relops (`rating:>=4`, `bitrate:`, `duration:`), date keywords + precision + ranges (`added:thisweek`, `year:1998..2004`), `true`/`false` presence. Typed against the Track domain (album/artist exposed through it; podcast/audiobook at 6/7).
- [x] Fields per spec §3.4: `artist:`/`albumartist:`/`album:`/`title:`, `genre:` vs `shelfgenre:`, `year:`/`added:`, `rating:`/`bitrate:`/`duration:`/`format:`, `is:played`/`is:starred`/`is:queued` (`is:queued` matches nothing until the queue table lands at 4b). Podcast/audiobook fields degrade to substring until 6/7.
- [x] Match modifiers (substring / `=`exact / `~`regex / `?`fuzzy Damerau-Levenshtein), boolean `AND`/`OR`/`NOT` + implicit AND + `!`, comparison/range, `sort:`/`sort:-` lifted to result metadata.
- [x] **Forgiving parser:** never errors; unknown fields/states/sorts and structural failures (unbalanced parens) degrade to substring + a warning.
- [x] Ranking: bare-text hits ordered by FTS5 `bm25` blended with recency (`rank::blend_relevance`) over the 1b FTS tables (`fts_rank` read).
- [x] **Perspectives:** `vl:NAME` expanded at parse time via a `PerspectiveResolver` with cycle detection (forgiving: a cycle degrades to empty + warning). Persistent storage + the save/load UI land at 3c.
- [x] CLI: `search <db> '<expression>'` (`--tsv`/`--json`/`--human`). Crate is GUI/storage-agnostic and fuzzable.
- [x] Tests: parse round-trip (parse → Display → re-parse stable); **SQL-vs-eval parity** over a 2k-track fixture (`tests/search_parity.rs`); degrade-to-substring; `vl:` cycle guard; per-field eval + per-node SQL.

*Usable artifact:* `conservatory-cli search '<expr>'` filters the library with the full grammar (verified against the real imported `testdata/` albums via both the SQL and eval paths).

### Phase 3b — Columns UI faceted panes

- [ ] N configurable hierarchical filter panes (default Genre → Album Artist → Album), each driven by a field expression, persisted (spec §3.3). The deadbeef-cui layout as a first-class window.
- [ ] Multi-value faceting: a track tagged `Electronic; Ambient` appears under both, while the single-valued shelf genre still drives the filesystem (the decoupling, spec §3.3).
- [ ] Memoized per-facet track counts; an `[All (N)]` synthetic row tops each pane.
- [ ] Debounced selection-change before downstream recompute (the deadbeef-cui invariant that keeps multi-select drags cheap on large libraries).
- [ ] Coalescing-delta plumbing: port Viaduct's `CoalescingQueue<T>` + `BatchUpdate` (main-thread, interval + max-interval flush) to deliver `LibraryChanges` as coalesced deltas, never full reloads (spec §2.1). UI updates apply as deltas; facets never scroll back to top on an unrelated event.
- [ ] Tests: facet counts against a fixture; selection narrows downstream panes correctly; delta coalescing under a burst of changes.

*Usable artifact:* the faceted panes filter a real library; selection is debounced and repaint hits the spec §13 budget on the fixture's scale.

### Phase 3c — Track list + Perspectives UI

- [ ] The leaf track list: sortable columns, multi-select (Ctrl/Shift), row affordances (status glyph, rating, hover lift) shared with the future episode list.
- [ ] Filter bar wired to `conservatory-search`; `Ctrl+F` focuses it; no separate search mode (spec §3.4).
- [ ] Perspectives surfaced in the UI: save, name, reload (re-parsed from text).
- [ ] Tests: sort/multi-select model logic; Perspective save/reload round-trip.

*Usable artifact:* **a working library browser.** Browse, filter, sort, and save Perspectives over the managed library.

---

## Phase 4 — Playback

A daily-driver music player. Profile switching at album/kind boundaries (spec §16.9) is the prototyping risk; tackle it in 4b where the unified queue makes it concrete.

### Phase 4a — libmpv host + music profile

- [ ] Dependency sign-off: `libmpv2` (spec §11) and the system `libmpv` (0.36+) requirement.
- [ ] A single libmpv instance kept alive across items (property API + filter graph, spec §6).
- [ ] Music profile: gapless within an album (`--gapless-audio`), ReplayGain track/album modes read from `tracks.replaygain_*`, crossfade (off by default). Decide §16.7 (scan vs read-only ReplayGain) and §16.6 (EQ/DSP depth) or defer explicitly.
- [ ] State persistence: position on pause/seek (debounced)/item-end/quit and every 30 s (the Belfry insurance interval); play counts and `last_played` on completion (spec §6.4).
- [ ] Tests (headless where possible): profile resolution; state-write debounce; play-count increment on completion.

*Usable artifact:* play a track from the library with gapless + ReplayGain; resume position survives a restart.

### Phase 4b — Unified queue + Now-bar

- [ ] `queue` table → in-memory `Vec<PlayableItem>`; position writes debounced (spec §4.3, §6.1). `PlayableItem { source, kind, profile }`.
- [ ] On advance, apply the item's profile (filter chain, ReplayGain mode, gapless/crossfade) before playing. Prototype the music-track ↔ (future) episode profile swap mid-queue here (spec §16.9), even though episodes arrive at Phase 6.
- [ ] Persistent Now-bar transport across views; queue view as a single drag-reorderable list, each row badged with its kind.
- [ ] Tests: queue model (add/remove/reorder, position integrity); PlayableItem profile resolution.

*Usable artifact:* build and play a queue; reorder it; the Now-bar reflects state.

### Phase 4c — System integration

- [ ] MPRIS2 (`org.mpris.MediaPlayer2`) via `zbus`: full metadata for the current item, play/pause/next/previous/seek, exposed to GNOME's media overlay and lock screen.
- [ ] Media keys / headset buttons; PipeWire output-sink picker; suspend inhibitor during active playback (spec §6.5).
- [ ] Tests: MPRIS metadata mapping; inhibitor lifecycle (acquire on play, release on stop).

*Usable artifact:* **a daily-driver music player.** It replaces deadbeef for the managed library, with system media integration.

---

## Phase 5 — Bulk editing + embedded write-back

### Phase 5a — Bulk metadata editing

- [ ] Multi-select in any list, edit fields across the selection: artist, album artist, album, year, genre (raw tags and shelf genre), rating, cover (spec §3.5).
- [ ] Search-and-replace across a field.
- [ ] An edit that alters shelf genre or the album/artist path triggers a file-move job, reusing the Phase 2c mover (dry-run preview + undo).
- [ ] Tests: bulk edit applies across a selection; a shelf-genre edit enqueues a move; undo reverts both DB and tree.

*Usable artifact:* bulk-edit a selection and have path-affecting edits move files safely.

### Phase 5b — Embedded-tag write-back (§5.5)

- [ ] Write curated DB metadata back into the files' embedded tags, batched as a job, respecting format capabilities (Vorbis comments, ID3, MP4 atoms). Requires the write side of the 1c tag library.
- [ ] CLI: `embed-tags <selector> [--dry-run]`.
- [ ] Tests: write-back round-trips through a re-read for each format; the spec §5.6 re-import contract holds (rebuildable subset reconstructs after a wipe-and-reimport).

*Usable artifact:* the library is never a roach motel: you can walk away with self-describing, portable files.

---

## Phase 6 — Podcasts (absorb Belfry)

Podcast parity. Belfry retires only when 6c lands (spec §16.8, CLAUDE.md). The fetch/parse port is `belfry-core`'s; Viaduct contributes the HTTP-client baseline. The subsystem lands as the **`conservatory-podcasts` plugin crate** (spec §2.2), which is where the heavy dependencies (`reqwest`, `feed-rs`, `ammonia`, `id3`, `oo7`) get pulled; its schema still lands in core's migration ledger (the boundary rule).

### Phase 6a — Fetch/parse port (headless)

- [ ] Schema: port Belfry's podcast tables (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`), with triage Queue state expressed through the unified `queue` table rather than a per-episode flag (spec §4.2). Episode `episode_fts` / `show_fts` added to the FTS set. The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [ ] Fetch loop ported from `belfry-core`: per-show polling with conditional GET (ETag / Last-Modified) and jittered intervals. The shared `reqwest` client baseline is ported from Viaduct's `network/http.rs` (gzip/brotli, rustls, pool caps, connect + request timeouts, descriptive User-Agent).
- [ ] Parse: `feed-rs` plus a hand-rolled `podcast:` namespace handler; episode identity by `(show_id, guid)`; three-source chapter precedence; show-note sanitize (`ammonia`). Dependency sign-off for `feed-rs`/`quick-xml`/`ammonia`/`id3`/`reqwest`/`oo7` (spec §11).
- [ ] HTTP Basic auth credentials in libsecret via `oo7`.
- [ ] OPML import/export round-trip, preserving tags and `applePodcastsID` (spec §8). CLI: `podcast add|remove|refresh|download`, `import-opml`/`export-opml`.
- [ ] Tests: conditional-GET state machine (304 path), `(show_id, guid)` dedup, OPML round-trip, `podcast:` namespace parse, against `wiremock` fixtures.

*Usable artifact:* subscribe to and refresh a show entirely headless via the CLI.

### Phase 6b — Podcasts tab + triage

- [ ] The Podcasts view: Belfry's three-pane Inbox → Queue → Played triage (sidebar of triage lists / shows / tags; episode list; detail pane), intact (spec §3.7).
- [ ] Per-show overrides: speed, Smart Speed, Voice Boost, skip, retention, inbox policy.
- [ ] The structural change from Belfry: **Queue is the shared unified queue**, so an episode and a track can sit next to each other.
- [ ] Streaming before/without download: if the local file is absent and a URL is present, libmpv streams with range requests (spec §5.3).
- [ ] Tests: triage transitions; per-show override resolution; episode-into-unified-queue insertion.

*Usable artifact:* podcasts are browsable and triageable in the GUI, with episodes flowing into the one queue.

### Phase 6c — Podcast playback profile + parity

- [ ] Podcast profile ported verbatim from Belfry §5: Smart Speed (silence-skip via `silenceremove` + pitch-preserving `rubberband`) and Voice Boost (compression + EQ + loudness normalization), including time-saved session accounting. This is the librubberband chain that forces GPL-3-or-later (spec §15).
- [ ] Episodes share the unified queue and the per-item profile switch prototyped in 4b; append-only `listening_sessions` discipline.
- [ ] Now Playing additions for episodes: chapters, show notes, Smart Speed indicator, sleep timer.
- [ ] Tests: filter-graph swap between a track and an episode mid-queue; time-saved accounting; resume offset on long items.

*Usable artifact:* **podcast parity reached.** One queue, one engine, both media types, full Smart Speed / Voice Boost. **Belfry can retire**: update the `~/.gitrepos` project map and archive the Belfry repo (spec §16.8).

---

## Phase 7 — Audiobooks (the third tab)

Audiobooks are the third media type (spec §3.8), landing as the **`conservatory-audiobooks` plugin crate** (spec §2.2). They are long-form speech, so they reuse the absorbed spoken-word engine (Smart Speed, Voice Boost, variable speed, sleep timer) from Phase 6c and the unified queue; that is why this phase lands after podcast parity. The data model, import, and browse surface are modeled on **Cozy** (the GTK4/libadwaita audiobook player); the metadata model and folder conventions on **Audiobookshelf**; chapter handling technique on **m4b-tool** (all three cloned under `~/.gitrepos/` as read-only reference, ATTRIBUTIONS.md). Belfry's retirement at 6c is unaffected. Metadata is local-source-only in v1 (online providers deferred, spec §16.10).

### Phase 7a — Audiobook model + import (headless)

- [ ] Schema (numbered migration, spec §4.5): `book_people` (authors + narrators, role-tagged), `series`, `books`, `book_authors`, `book_narrators`, `book_chapters`, `book_playback`. `book_fts` (title, author, narrator, series) trigger-synced (spec §4.4). The unified `queue` gains the `audiobook` kind + `book_id` (spec §4.3). The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [ ] Tag + sidecar reader: embedded M4B/ID3 tags, then the Audiobookshelf sidecar conventions (`.opf` via the existing `quick-xml`, `desc.txt`, `reader.txt`, `cover.jpg`), then folder structure, into a book draft (author, narrator, series + decimal sequence, year, publisher, ISBN/ASIN, description, language). Author and narrator are distinct roles.
- [ ] Chapter resolver: embedded M4B markers → else one-file-per-chapter folder → (opt-in, deferred per spec §16.11) silence detection. Each chapter is a `(file_path, file_offset, duration)` row addressing either a standalone file or an M4B span.
- [ ] Audiobook path template: `Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})`, series components collapsing for standalone books (spec §5.7). New tokens (`{author}`, `{narrator}`, `{series}`, `{series_index}`) extend the Phase 2a engine. Import moves/copies into the managed tree via the Phase 2c mover (dry-run + undo): books are owned like music, not ephemeral like podcasts.
- [ ] Cover + accent via the Hermitage path into `books.accent_rgb` (spec §7.4).
- [ ] CLI: `audiobook import`, `audiobook set` (spec §9).
- [ ] Tests: M4B-with-embedded-chapters import; multi-file book import; `.opf`/`reader.txt`/`desc.txt` sidecar parse; decimal series-sequence parse; a book renders to the correct path and the mover round-trips it; `book_fts` sync.

*Usable artifact:* point the CLI at a folder or an `.m4b` and get an organized, database-owned audiobook with ordered chapters, headless.

### Phase 7b — Audiobooks tab (browse)

- [ ] The Audiobooks view (spec §3.8): a cover-grid shelf (accent-tinted, the Hermitage unit) plus a book detail pane (chapter list, progress, author/narrator, series/sequence, per-book speed + sleep-timer controls). Cozy's layout, rebuilt over Conservatory's database.
- [ ] State derivation: New / In progress / Finished from `book_playback`; in-progress books surface first.
- [ ] Filter bar wired to `conservatory-search` with the audiobook fields (`author:`, `narrator:`, `series:`, `is:finished`); same grammar, no separate search mode.
- [ ] Bulk edit (spec §3.5) across selected books; a path-affecting edit (author/series/title/year) enqueues a move via the Phase 2c mover.
- [ ] Tests: shelf/filter model logic; book-state derivation; Perspective save/reload over books.

*Usable artifact:* browse, filter, sort, and bulk-edit the audiobook library in the GUI.

### Phase 7c — Audiobook playback (chapters + first-class resume)

- [ ] A book is one `PlayableItem` (kind `Audiobook`); the engine plays its ordered chapters with internal, gapless chapter advance (across files or within an M4B) and advances the queue only when the book finishes (spec §6.1).
- [ ] Reuse the spoken-word profile (variable speed, Smart Speed, Voice Boost) with per-book overrides resolved from `book_playback` (spec §6.3). No new filter graph.
- [ ] First-class resume: absolute `book_playback.position`, `finished` on completion, written on the insurance interval (spec §6.4). Now Playing additions for books: chapter list/jump, sleep timer, speed control.
- [ ] MPRIS metadata for the current book/chapter (spec §6.5).
- [ ] Tests: chapter advance across a multi-file book and within an M4B (no gap, correct offsets); resume-to-the-second across a restart; per-book override resolution; finished-state transition.

*Usable artifact:* **audiobook parity.** Play a book from the shelf with chapters, variable speed, sleep timer, and exact resume, in the one unified queue alongside music and podcasts.
