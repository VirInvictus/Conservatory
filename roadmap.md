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

### Phase 3b — Columns UI faceted panes ✅

The first GTK4/libadwaita code (programmatic UI; facet logic in `conservatory-core`, the binary renders).

- [x] Hierarchical filter panes (default Genre → Album Artist → Album), the deadbeef-cui layout as a first-class window. (User-reconfigurable order / field expressions and their **persistence** deferred to the config layer, Phase 10; 3b ships the default hierarchy.)
- [x] Multi-value faceting: a track tagged `Electronic; Ambient` appears under both Genre rows (joins `track_genres`), while single-valued shelf genre drives the filesystem (the §5.2 decoupling). `core::db::facets`.
- [x] Memoized per-facet track counts; an `[All (N)]` synthetic row tops each pane (selecting it clears that pane's constraint).
- [x] Debounced selection-change before downstream recompute (the deadbeef invariant), via the ported coalescing queue; the cascade recomputes only panes downstream of the changed one + the leaf.
- [x] Coalescing-delta plumbing: ported Viaduct's `CoalescingQueue` (main-thread, interval + max-interval flush, dedup) and used it for the selection debounce. (`BatchUpdate` and live `LibraryChanges` delivery deferred until there is an in-GUI writer, Phase 5a.)
- [x] Tests: facet counts + cascade + multi-value genre against a fixture (`core/tests/facets.rs`, headless); coalescing burst → single flush (`ui::coalescing` test, headless glib). GTK widgets verified by build + manual launch.

*Usable artifact:* `conservatory <db>` launches the browse window; facet selection narrows downstream panes and the leaf list. (`conservatory-cli debug-facets <db>` exercises the same logic headless.)

### Phase 3c — Track list + Perspectives UI ✅

- [x] The leaf track list: sortable columns, multi-select (Ctrl/Shift), row affordances (rating stars, hover lift) shared with the future episode list. Sorting delegates to a pure `core::cmp_tracks`/`sort_tracks` (the GTK `CustomSorter` and the headless comparator share it); rating renders as accent-tinted symbolic stars; `TrackBrief` gained a name-ordered `genres` roll-up + `rating`. (The per-row playing/status glyph waits for playback state, Phase 4.)
- [x] Filter bar wired to `conservatory-search`; `Ctrl+F` focuses it; no separate search mode (spec §3.4). Always-on `SearchEntry`; the facet set and the grammar intersect on the leaf; malformed input degrades to substring + a yellow tint. The composition lives in a non-GTK `query.rs` (headless-tested), keeping core runtime-search-free (the `conservatory-search` dep stays consumer-side by design).
- [x] Perspectives surfaced in the UI: save, name, reload (re-parsed from text). Migration `0003` adds the core-owned `perspectives` table; the sidebar lists Default + saved searches; `vl:NAME` resolves from storage. Saves/deletes go through the **single-writer worker, now stood up in the GUI** on a tokio runtime (the in-GUI writer, pulled forward from Phase 5a to back persistence).
- [x] Tests: sort comparator (case-fold, stable tie-breaks, numeric keys); filter-bar facet∩grammar composition + `vl:` round-trip (binary, headless); Perspective CRUD + `vl:` resolution (core).

*Usable artifact:* **a working library browser.** Browse, filter, sort, and save Perspectives over the managed library.

---

## Phase 4 — Playback

A daily-driver music player. Profile switching at album/kind boundaries (spec §16.9) is the prototyping risk; tackle it in 4b where the unified queue makes it concrete.

### Phase 4a — libmpv host + music profile ✅

- [x] Dependency sign-off: `libmpv2` (spec §11; ATTRIBUTIONS.md) and the system `libmpv` (0.36+) requirement. `libmpv2 4.1` pulled into `conservatory-core` (the player lives in core, spec §16.13); `libmpv-dev` added to both CI jobs.
- [x] A single libmpv instance kept alive across items (`player::host::MpvHost`, property API + the input-command layer, spec §6). The threaded `Player` handle + command channel are deferred to 4b, where the GTK Now-bar is the second consumer; 4a drives the host directly from the CLI loop (no speculative plumbing).
- [x] Music profile (`player::profile`, pure + tested): gapless within an album (`gapless-audio`), ReplayGain via mpv's native `replaygain` property (mpv reads the file tags `lofty` stored), with the DB `replaygain_*` columns driving mode resolution (album→track→off downgrade by available tags). Crossfade is carried through (config field) but rendered at 4b with the queue (a between-tracks behaviour). **§16.7 deferred:** read-only ReplayGain, no in-app scan. **§16.6 deferred:** no EQ/DSP in 4a.
- [x] State persistence (`player::state`, pure + tested): position written on the insurance interval (30 s) and on the forced points (pause/seek/item-end/quit), through the single-writer worker into the new singleton `playback_state` table (migration `0004`); `play_count` + `last_played` bumped on a natural end-of-file only (`EndReason::Eof`).
- [x] Tests: profile resolution + ReplayGain downgrade (8 unit tests); state-write debounce + only-Eof-counts (4 unit tests); `playback_state` round-trip + play-count increment through the worker, and an `ao=null` libmpv smoke test that decodes a committed fixture to EOF (`tests/playback.rs`).

*Usable artifact:* `conservatory-cli play <db> [track_id]` plays a track from the managed library with gapless + ReplayGain through libmpv; the position is persisted on the insurance interval and `play <db>` (no id) resumes the saved cursor across a restart. The threaded engine, the unified queue, and the Now-bar land at 4b.

### Phase 4b — Unified queue + Now-bar

Split into two shippable sub-phases: **4b-i** lands the queue + the threaded engine headless (CLI-testable, the hard rule); **4b-ii** is the GTK Now-bar + queue view that consumes them.

#### Phase 4b-i — Unified queue + threaded Player (headless) ✅

- [x] `queue` table (migration `0005`, spec §4.3) → in-memory `Vec<PlayableItem>`; the full column set lands (the unified queue is a core commitment) but only `track_id` carries a foreign key — `foreign_keys = ON` refuses any DML on a child whose parent table is absent, so the `episode_id`/`book_id` FKs are added when `episodes`/`books` land (Phases 6/7) via a table rebuild. Positions stay contiguous `0..n-1`, renumbered transactionally on the single writer (`enqueue`/`remove`/`reorder`/`clear`/`replace`).
- [x] The threaded `Player`: a dedicated `std::thread` owns the `!Send` `MpvHost` (built there via a `make_host` factory), behind a `Send + Clone` `PlayerHandle` (command channel out, `Arc<Mutex<PlayerSnapshot>>` polled back). The 4a CLI pump-loop is lifted into `player::engine`. `PlayableItem { track_id, source, profile, album_id, kind }`.
- [x] On advance, apply the item's profile before playing (per-item `host.load`, the spec §16.9 boundary switch with the music profile). Advance only on natural `Eof`; an errored item skips; self-initiated `Stop`/`Redirect` (from our own load) do not advance. Persistence split (spec §6.4): debounced ticks fire-and-forget, terminal writes (pause/seek/stop/shutdown/final-EOF play-count) block on the worker so they land. *Audible within-album gaplessness (mpv playlist append) is deferred to 4b-ii, where it is verified by ear.*
- [x] `is:queued` wired up (was inert since 3a): `sql_translate` emits a `queue` subquery on the SQL fast path; the eval path reads `SearchRow.queued` (an `EXISTS` against the queue), populated in `search_rows`.
- [x] CLI: `queue add|list|remove|clear`; `play <db> <root> [track_id]` drives the engine through the queue (root resolves the relative `file_path`s), polling the snapshot until the queue ends.
- [x] Tests: queue position integrity (enqueue/remove/reorder stay contiguous); `is:queued` membership; the engine plays a queue of imported fixtures to its end through a null audio output, incrementing each play count and landing the cursor on the last item (`tests/queue.rs`).

*Usable artifact:* `conservatory-cli queue add` / `play <db> <root>` builds and plays a queue headlessly; the engine advances item to item applying each profile, persists position + play counts, and resumes from the saved cursor.

The GTK half is itself sizable, so it splits again: **4b-ii-a** makes the window play (engine + Now-bar + transport); **4b-ii-b** adds the visible queue panel and drag-and-drop reorder.

#### Phase 4b-ii-a — Player engine in the GUI + Now-bar transport ✅

- [x] The threaded `Player` stood up in the browse window (`player::spawn(worker.clone(), rt.handle())` on the existing in-GUI runtime); a libmpv init failure leaves it unset and the transport inert (browse still works). The window holds the `PlayerHandle`, the snapshot poll source, the playing queue's id→label map, and the library root.
- [x] Persistent bottom **Now-bar** (`now_bar.rs`, attached via `ToolbarView::add_bottom_bar`): title/artist, prev / play-pause / next (symbolic glyphs), position label + seek `Scale` (driven by `change-value`, so the refresh's programmatic `set_value` never loops), and a volume `ScaleButton`. Buttons are non-blocking `PlayerHandle` sends.
- [x] **Double-click / Enter plays the visible leaf list from that row** (the deadbeef idiom, spec §3.6): the selection model's display order is the queue, the activated index is the start. The id list + a `Track` batch-read (`get_tracks`) feed a pure `playqueue::build_play_queue` (order preserved, `source` = root-joined, profile resolved, start re-indexed past any vanished track).
- [x] A 250 ms `glib::timeout_add_local` polls the snapshot → `refresh_now_bar` (position/seek/icon every tick; title/artist only on track change). Clean teardown on `close-request`: remove the timer, then `player.shutdown()` (joins the engine thread; its terminal flush still has the worker), then worker/runtime drop.
- [x] The library root arrives as an optional second CLI arg (`conservatory <db> [root]`); Phase 10 config replaces it.
- [x] Tests: `build_play_queue` (order, root-join, start re-index, missing tracks) + time formatting unit tests; `get_tracks` cross-chunk worker test. The widgets are verified by build + manual launch (the 3b/3c precedent).

*Usable artifact:* `conservatory <db> <root>` — double-click a track to play the visible list from there, with a working Now-bar transport (play/pause, prev/next, seek, volume) that reflects state.

#### Phase 4b-ii-b — Queue view + drag-and-drop reorder (GTK) ✅

- [x] A right-side slide-in queue drawer (`queue_panel.rs`, a `gtk::Revealer`, header toggle + `Ctrl+U`): a `ListView` of `QueueRow` (kind icon + title/artist), the playing row accent-highlighted, **drag-and-drop reorderable** (the Atrium idiom: the row carries its position, the `DropTarget` computes Above/Below from cursor Y, controllers torn down in `unbind`), plus keyboard `Alt+↑/↓` reorder, `Delete`, `Ctrl+Shift+C`.
- [x] The engine gained **in-place mutation** (`MoveItem`/`RemoveItem`/`ClearQueue`) so editing the live queue never restarts the current track; the `current_index` math is pure + unit-tested (`move_current_index`/`remove_current_index`). The GUI applies the identical `(from, to)` to `worker.reorder_queue` and `player.move_item`, so DB position == engine index stays invariant; double-click now **writes the DB queue through** (`replace_queue_with_tracks`) so the drawer reflects the spec §4.3 source of truth.
- [x] Core read `load_queue_display` (queue ⋈ tracks ⋈ artists) backs the drawer; the highlight follows playback via the 250 ms snapshot poll.
- [x] Tests: the index helpers (8), `drop_target_position` (Above/Below, up/down, clamp), an engine null-host integration test (move/remove track `current_index` without restarting), and a `load_queue_display` worker test. Widgets verified by build + manual launch.

*Usable artifact:* build and play a queue in the GUI; open the drawer and reorder it by drag (or keyboard); the playing row is highlighted and the Now-bar reflects state.

#### Phase 4b-ii-c — Queue polish (GTK) ✅

- [x] Launch-resume: on GUI startup `resume_saved_queue` loads the saved DB queue into the engine **paused at the cursor** (a new `paused` flag on the engine's `SetQueue`, exposed as `PlayerHandle::resume`), so reopening the app is silent until play.
- [x] `Ctrl+Enter` appends the browse selection to the queue (DB tail via `enqueue_tracks` + live engine tail via the new `AppendItems` command, which starts playing if the queue was idle); plain Enter / double-click still replaces.
- [x] Tests: engine null-host integration — append-to-idle starts playing, a second append extends the tail, and a fresh engine resumes the whole queue paused at the cursor.
- [ ] **Deferred:** a cover thumbnail in the Now-bar (blocked: `albums.cover_path` is unpopulated until cover-to-disk lands, spec §7.4); the audible within-album gapless prototype (mpv internal playlist append, spec §16.9); the `playback_state` explicit queue-entry reference; the library root sourced from config (Phase 10) rather than a CLI arg.

*Usable artifact:* reopen the app and pick up where you left off (paused at the cursor); `Ctrl+Enter` appends the selection to a playing queue.

### Phase 4c — System integration

Split: **4c-i** is the D-Bus half (MPRIS2 + the suspend inhibitor, on `zbus`); **4c-ii** is the audio-output-device picker (an mpv-property + GUI-menu mechanism, no D-Bus).

#### Phase 4c-i — MPRIS2 + suspend inhibitor ✅

- [x] `conservatory-core/src/mpris.rs` serves `org.mpris.MediaPlayer2` + `…Player` on the session bus via `zbus 5` (signed off, ATTRIBUTIONS.md): metadata, `PlaybackStatus`, `Position`, `Volume`, `CanGoNext/Previous`, and `Play`/`Pause`/`PlayPause`/`Next`/`Previous`/`Stop`/`Seek`/`SetPosition` driving the `PlayerHandle`. `run(player, pool)` polls the snapshot (~300 ms), emits `PropertiesChanged` on change, and resolves the current track's metadata via a new `track_metadata` read. The GUI spawns it on its runtime; **media keys, the GNOME overlay, and the lock screen come for free.**
- [x] Suspend inhibitor: a logind `org.freedesktop.login1.Manager.Inhibit("sleep", …, "block")` proxy on the system bus, the FD held while playing and dropped on pause/stop (best-effort: a missing system bus doesn't break MPRIS).
- [x] Tests: pure mapping helpers (`playback_status`, `can_go_next/previous`, `wants_inhibit`, volume/position conversions, `metadata_fields`) + a `track_metadata` worker test. Live D-Bus serving + the logind inhibit are verified manually (`playerctl`, `systemd-inhibit --list`), the build-plus-manual precedent.

*Usable artifact:* `playerctl` and the keyboard media keys drive playback; the GNOME media overlay/lock screen show the track; the machine won't suspend mid-track.

#### Phase 4c-ii — Output-sink picker (GTK) ✅

- [x] `MpvHost::audio_devices()` parses mpv's `audio-device-list` (node → `AudioDevice { name, description }`) and `set_audio_device()` sets the `audio-device` property; the engine queries the list once at init and exposes it (plus the current selection) on the snapshot; a `SetAudioDevice` command applies a switch.
- [x] A header `MenuButton` (`set_create_popup_func`, built fresh on open from the snapshot) lists the sinks (the current one checked) and switches output on click (spec §6.5).
- [x] Tests: a host integration test (`audio_devices()` includes `auto`; `set_audio_device("auto")` succeeds); the menu is verified by build + manual launch.

*Usable artifact:* **a daily-driver music player.** It replaces deadbeef for the managed library, with full system media integration and output-device selection.

---

## Phase 5 — Bulk editing + embedded write-back

### Phase 5a — Bulk metadata editing

Split headless-first (the CLI-testable rule): **5a-i** is the editing logic + worker commands + `tag` CLI verb; **5a-ii** is the GTK bulk-edit dialog. Cover editing is Phase 5d (`cover_path` is unpopulated until then); embedded write-back is Phase 5b.

#### Phase 5a-i — Field editing + path-affecting move (headless, CLI) ✅

- [x] Core write commands (`db::writes` + worker): `update_track` (title / rating / track artist, get-or-create by derived sort name), `update_album` (title / year / shelf genre / album artist), `set_track_genres` (clear + re-link the raw §5.2 multi-value side). `COALESCE`-guarded so only the changed fields move; the FTS triggers re-sync on every UPDATE (verified by test).
- [x] Pure resolver (`conservatory-core/src/edit.rs`, unit-tested): parse `field=value`, classify track-level vs album-level and **path-affecting** (album / albumartist / year / shelfgenre, the default-template fields), build the typed `TrackEdit`/`AlbumEdit`, split raw genres, and literal search-and-replace. Shared by the CLI and (5a-ii) the GTK dialog.
- [x] Path-affecting edits reuse the Phase 2c mover (dry-run preview + undo), re-rendering only the touched albums (the generalized `shelf-genre-set` → `organize` flow).
- [x] CLI: `tag set <db> <selector> <field=value>... [--root] [--apply]` and `tag replace <db> <selector> <field> <find> <replace> [--root] [--apply]`, selector via `conservatory-search`.
- [x] Tests (`tests/edit.rs`, committed + synthetic fixtures): field updates re-read; FTS follows a title/artist/album/albumartist rename; `set_track_genres` replaces (not appends); a year edit re-renders, moves, and `undo` reverts DB + tree. Hand-verified against the `testdata/` albums.

*Usable artifact:* `conservatory-cli tag set <db> '<expr>' year=1992 --root <root> --apply` edits the matched library and re-shelves any moved files, fully headless and undoable.

#### Phase 5a-ii — GTK bulk-edit dialog ✅

- [x] A bulk-edit dialog (`adw::AlertDialog` with a labelled-entry grid, the Perspective-save precedent) over the leaf multi-selection, opened by a header pencil button or `Ctrl+E`; one entry per field (album artist, album, year, shelf genre, track artist, title, raw genres, rating), blank means unchanged. Reads the selection with the existing `is_selected` + `downcast::<TrackRow>` loop, parses each filled field through `core::edit::parse_assignment` (rejecting the whole set if a value is invalid), and applies via the new worker commands.
- [x] Path-affecting edits show a move **preview-and-confirm** (`mover::plan` → a "Move N files?" `AlertDialog` → `mover::apply`, `MoveKind::Organize`, scoped to the touched albums); the browse refreshes via `populate_initial` after the edit.
- [x] Search-and-replace is available headless (`tag replace`, 5a-i); the in-dialog replace mode is deferred (the per-field set covers the common case). Live incremental `LibraryChanges`/`BatchUpdate` delta delivery stays deferred (a full reload is used).
- [x] The dialog is verified by build + manual launch (the 3b/3c precedent); the underlying edit/move logic is covered by the 5a-i tests.

*Usable artifact:* select tracks in the browser, bulk-edit their fields (`Ctrl+E`), and have path-affecting edits move files safely behind a preview.

### Phase 5b — Embedded-tag write-back (§5.5)

Headless-first: **5b-i** is the core write + `embed-tags` CLI + tests; **5b-ii** is the GTK action. Only the rebuildable descriptive layer is written; the curated layer (rating, shelf genre, play counts, starred) stays DB-only (§5.6).

#### Phase 5b-i — Core write-back + `embed-tags` ✅

- [x] `tags::write_track_tags(path, &TagWrite)` (lofty write, signed off at 1c): write the format's canonical primary tag authoritatively (title, track artist + sort, album, album artist + sort, year, track/disc, raw multi-value genres), creating it if absent, dropping the legacy ID3v1. `db::writeback_rows` is the one join that supplies all of it (display + sort names + group-concat genres).
- [x] CLI `embed-tags <db> <selector> --root <root> [--apply]`: dry-run shows the per-file field diffs (current tags vs DB); `--apply` writes. Re-derivable from the DB (the source of truth), so dry-run is the safety and there is no undo journal.
- [x] Tests (`tests/writeback.rs`): per-format round-trip (edit DB → embed → re-read the file), and the **§5.6 re-import contract** (embed → fresh DB → re-import → the edited descriptive field survives). Hand-verified against the `testdata/` albums.

*Usable artifact:* `conservatory-cli embed-tags <db> '<expr>' --root <root> --apply` writes the curated metadata into the files; a wipe-and-reimport reconstructs the descriptive layer (§5.6 holds).

#### Phase 5b-ii — GUI action ✅

- [x] An "Embed metadata into files" header action (the save icon) over the leaf selection (explicit, not auto-on-edit, the Calibre model), behind a "Write tags to N file(s)?" confirm and a result dialog; writes through `write_track_tags`. Verified by build + manual launch.

> **APE-strip deferred.** The Lattice `apestrip` hygiene (strip a stray APEv2 that shadows ID3 on MP3, with optional APE→ID3 migration) is **not** in 5b: lofty reads APE on MPEG but neither writes nor removes it, so a reliable strip needs byte-level surgery (exactly why `apestrip.py` is hand-rolled). Deferred to a byte-level pass, paired with the Phase 8c "detect stray APE" audit. (`embed-tags` writes the canonical ID3v2 correctly; it just cannot remove a pre-existing APE shadow on MPEG.)

### Phase 5c — ReplayGain scan (resolves spec §16.7) ✅

Phase 4a reads ReplayGain but never scans (the §16.7 open decision); this settles it on the "scan in-app" side so the player can normalize untagged albums.

- [x] **Mechanism (settled): `rsgain`** (installed, v3.6), not the `ebur128` crate. The crate measures only decoded PCM and the pure-Rust decoder (symphonia) can't decode Opus (half the library); rsgain decodes every format itself and writes correct RG2.0 tags (incl. Opus R128 + album gain). External tool (ATTRIBUTIONS.md; bundle for Flatpak later). `conservatory-core/src/replaygain.rs`: `scan_album_files` shells `rsgain custom -a -s i -c p -l <lufs>` (the Lattice invocation); `replaygain_from_file` re-reads the written gains.
- [x] DB refresh: a `set_track_replaygain` worker command updates `tracks.replaygain_*` from the scan, so the Phase 4a profile resolution (album → track → off) sees the values unchanged.
- [x] CLI: `replaygain scan <db> <selector> --root <root> [--apply] [--target-lufs N]` (per-album grouping; dry-run lists the albums, `--apply` scans + syncs).
- [x] Tests (`tests/replaygain.rs`): the DB-sync half is hermetic (write a known tag → read → feed the profile); the rsgain scan is a skip-if-absent integration test (covers FLAC + Opus).

*Usable artifact:* `conservatory-cli replaygain scan <db> '<expr>' --root <root> --apply` gives untagged albums correct ReplayGain in-app.

### Phase 5d — Cover art to disk + cover management (spec §7.4) ✅

Implements the §7.4 covers-on-disk story that the Now-bar thumbnail and MPRIS art were deferred behind (Phases 4b-ii-c, 4c-i). The **trust-critical mover is left untouched**: covers are derived (re-extractable from embedded art), so they are synced idempotently after a move, not journaled.

- [x] `conservatory-core/src/covers.rs`: `write_cover` (sniff PNG vs JPEG), `sync_album_cover` (write into the album folder, drop a stale cover at the old location), `resync_album_covers` (ensure every album's cover is in its current folder + `cover_path` matches, after a move). Import writes each album's `cover.jpg`/`.png` and records `cover_path`; `organize` and path-affecting edits resync.
- [x] Set / replace a cover from a file: CLI `set-cover <db> <album_id> <image> --root` (writes the file, updates `cover_path`, refreshes the accent). (The in-dialog GUI cover field is deferred; the CLI verb covers it.)
- [x] Wired the unblocked consumers: the Now-bar cover thumbnail (a `gtk::Image`, loaded on track change) and MPRIS `mpris:artUrl` (`file://<root>/<cover_path>`, root passed into `mpris::run`).
- [x] Tests (`tests/covers.rs`): import writes the cover + populates `cover_path` + accent; a year edit + organize moves the cover and updates `cover_path` (old removed). The artUrl mapping is a `mpris::build_metadata` unit test; the Now-bar image is build + manual.

*Usable artifact:* albums show their cover on disk, in the Now-bar, and in the GNOME media controls.

---

## Phase 6 — Podcasts (absorb Belfry)

Podcast parity. Belfry retires only when 6c lands (spec §16.8, CLAUDE.md). The fetch/parse port is `belfry-core`'s; Viaduct contributes the HTTP-client baseline. The subsystem lands as the **`conservatory-podcasts` plugin crate** (spec §2.2), which is where the heavy dependencies (`reqwest`, `feed-rs`, `ammonia`, `id3`, `oo7`) get pulled; its schema still lands in core's migration ledger (the boundary rule).

### Phase 6a — Fetch/parse port (headless)

The headless absorption, split so each piece leaves a usable artifact: **6a-i** is the core DB foundation (schema + worker, no network); **6a-ii** is the Viaduct-style fetcher + `feed-rs`/namespace parse + the refresh pipeline; **6a-iii** is OPML round-trip, credentials, and episode download.

#### Phase 6a-i — Podcast schema + core worker + models (no network) ✅

- [x] Schema: ported Belfry's eight podcast tables (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`) as migration `0006`, with one change (spec §4.2): triage Queue state lives in the unified `queue` table, so `playback` drops Belfry's `in_queue` / `queue_position` (Inbox / Queue / Played derives from `playback.played` plus `queue` membership). `episode_fts` / `show_fts` added to the FTS set (ordinary tables + triggers, matching the music FTS style). The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [x] **Queue `episode_id` foreign key**: migration `0006` rebuilds the `queue` table to add the FK deferred at `0005` (the 4b-i note: `foreign_keys = ON` refused it until `episodes` existed); `book_id` stays plain until Phase 7. `docs/schema.md` updated.
- [x] Core domain + DB plumbing: `Show` / `Episode` / `Playback`+`PlayedState` / `ShowSettings`+`InboxPolicy` / `ListeningSession` / `Chapter` / `Tag` models (`db/models.rs`); reads (`get_show`/`list_shows`, `get_episode_by_guid`/`list_episodes_for_show`, `get_playback`, `get_show_settings`, `list_chapters`, `list_tags_for_show`); worker commands + `WorkerHandle` methods (`get_or_create_show`, `update_show` carrying the conditional-GET state, `delete_show`, `upsert_episode` by `(show_id, guid)`, `upsert_playback`, `upsert_show_settings`, `replace_chapters`, `get_or_create_tag`, `set_show_tags`). The schema is core-owned (spec §2.2); the plugin (6a-ii+) consumes these typed methods.
- [x] Tests (`tests/podcasts.rs`): show get-or-create idempotency; episode upsert dedups by guid + a re-fetch never erases a downloaded path; FTS sync across edit/delete; playback + show-settings round-trip; chapters replace; tags round-trip; the queue `episode_id` FK verified via `PRAGMA foreign_key_list`. Migration table-exists test extended. Music-only build (`--no-default-features`) stays green (core is feature-free; the tables apply in every build).

*Usable artifact:* a show, its episodes, and its triage/playback/settings/chapters/tags round-trip through the single-writer worker, headless; the unified queue can now reference episodes.

#### Phase 6a-ii — Fetch + parse + refresh pipeline (headless)

Split again: **6a-ii-a** is the RSS-catching layer (HTTP client + conditional-GET fetcher, no parse); **6a-ii-b** is feed-rs/namespace parse + the refresh orchestration + CLI.

##### Phase 6a-ii-a — HTTP client + conditional-GET fetcher ✅

- [x] `conservatory-podcasts/src/http.rs`: the `reqwest` client baseline ported from **Viaduct's `network/http.rs`** (rustls, gzip/brotli, `POOL_MAX_IDLE_PER_HOST=4`, 30 s idle/request + 10 s connect timeouts, a descriptive `Conservatory/<ver>` User-Agent, the `ACCEPT_FEED` header). `build_client()`.
- [x] `conservatory-podcasts/src/fetcher.rs`: the conditional-GET `Fetcher` ported from the network slice of **Viaduct's `network/fetcher.rs`** (the mature path; Belfry's loop was never implemented). `fetch(url, etag, last_modified)` sends `If-None-Match` / `If-Modified-Since`, short-circuits 304 (empty body), extracts `ETag` / `Last-Modified` / `Cache-Control: max-age`, and keeps a per-host 429 cooldown honouring `Retry-After`. The broadcast request-coalescing and the content-hash re-parse skip are deliberately dropped/deferred (each show has a distinct feed URL; the content hash lives with the refresh state at ii-b). `FetchError` (`error.rs`).
- [x] Deps activated in `conservatory-podcasts`: `reqwest` (rustls/gzip/brotli), `tokio`, `chrono`, `thiserror`, `tracing` (+ `wiremock` dev). `bytes` deferred (body is `Vec<u8>`); ATTRIBUTIONS.md updated with the Viaduct/NNW provenance.
- [x] Tests (`tests/fetcher.rs`, `wiremock`, hermetic): 200 body + `ETag`/`Last-Modified`/`Cache-Control` extraction; conditional GET sends `If-None-Match` and handles 304; 429 + `Retry-After` returns `RateLimited` and the cooldown short-circuits the next fetch (verified by an `expect(1)` mock); invalid-URL path; `max-age` parse + UA/client smoke.

*Usable artifact:* a `Fetcher` that does conditional GET against a feed URL, honouring 304 and 429, headless and wiremock-tested.

##### Phase 6a-ii-b — feed-rs/namespace parse + refresh + CLI

- [ ] Parse: `feed-rs` for the RSS/Atom core plus Belfry's hand-rolled `podcast:` namespace handler (`namespace.rs`, ported, merged to feed-rs entries by position with a guid cross-check); a fresh `Entry → Episode` mapping (enclosure from `entry.media`/links, guid = item `podcast:guid` else feed-rs id, pub_date/season/episode/type); episode identity by `(show_id, guid)`. Show-note sanitize (`ammonia`) and the three-source chapter precedence may slip to 6a-iii / 6b. Dependency sign-off for `feed-rs` / `quick-xml`.
- [ ] A `slugify` for the managed `Podcasts/<slug>` folder (spec §5.3). The refresh orchestration fetches concurrently under a `Semaphore(REFRESH_PARALLELISM)` (`tokio::task::JoinSet`), parses, upserts episodes through the 6a-i worker methods, and stamps `etag` / `last_modified` / `last_fetched` on the show. CLI `podcast add|remove|refresh` behind `#[cfg(feature = "podcasts")]`.
- [ ] Tests: `(show_id, guid)` dedup through a refresh, `podcast:` namespace parse, conditional-GET round-trip (etag stored then replayed), against `wiremock` feed fixtures.

*Usable artifact:* `podcast add <url>` then `podcast refresh` subscribes to and pulls a show's episodes entirely headless.

#### Phase 6a-iii — OPML + credentials + download (headless)

- [ ] OPML import/export round-trip, preserving tags and `applePodcastsID` (spec §8). CLI: `import-opml` / `export-opml`.
- [ ] HTTP Basic auth credentials in libsecret via `oo7` (the `auth_user` / `auth_pass_ref` hooks from 6a-i); episode `download` into the managed `Podcasts/<slug>/...` tree (spec §5.3). CLI `podcast download`.
- [ ] Tests: OPML round-trip; credential store (in-memory backend); download writes the file and sets `audio_path`.

*Usable artifact:* OPML in/out round-trips; an episode downloads into the managed tree.

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

---

## Phase 8 — Library maintenance and audits

A read-only health-and-hygiene suite modeled on **Lattice** (Brandon's CLI/TUI music auditor; ATTRIBUTIONS.md). Lattice scans the filesystem and reports, never mutating; Conservatory already owns the database, so these audits run against the DB plus the managed files. Each surfaces as a CLI verb first (the every-surface-CLI-testable rule), with GUI reports layered on later. The phase is **media-type-agnostic and depends only on Phases 1 to 3**, so it can be pulled forward of Phases 6/7 if a library-integrity need arises; it is placed here so it can cover all three media types at once. Integrity and decode checks shell out to `flac` / `ffmpeg` (external-tool sign-off, spec §11).

Deliberately **not** adopted from Lattice: the AI-readable library exports (`--ai-library` / `--ai-wings`, an LLM-prompt text dump) and the per-genre "wings" text trees, both superseded by Conservatory's live faceted browse; and Lattice's path-pattern tag fallback (Conservatory's tags come from the database, not the path).

### Phase 8a — Integrity verification

Modeled on Lattice's `--testFLAC` / `--testMP3` / `--testOpus` / `--testWAV` / `--testWMA` and its four-tier classification.

- [ ] Decode-verify every file (or a selection) with parallel workers. Tooling (**settled**): `flac -t` for FLAC (authoritative, MD5-verifies the decoded audio, which catches bit-rot a plain decode misses) and the `ffmpeg` CLI for the rest (strict decode with forced demuxers). Both shell out (external-tool sign-off when this lands); the libmpv-reuse alternative was rejected because a player decoder is lenient by design and would weaken the verdicts. Classify each as CORRUPT (tool error, or a FLAC that decodes fewer samples than declared, i.e. truncation), SUSPECT (decoded to the end but the tool complained, or trailing data), METADATA (only a container/tag warning, audio intact), or OK, the Lattice tiers.
- [ ] Persist results (a last-checked timestamp + verdict, keyed by path + size/mtime) so a re-verify skips unchanged files; surface CORRUPT / SUSPECT in a report and, later, a GUI list.
- [ ] CLI: `verify <selector> [--verbose]`, with a non-zero exit only when CORRUPT files exist (the Lattice contract), so it is scriptable in a cron/backup hook.
- [ ] Tests: a deliberately truncated/corrupt fixture classifies CORRUPT, a clean fixture OK; the skip-unchanged cache works.

*Usable artifact:* `conservatory-cli verify <db>` reports library corruption with the same conservative tiers as Lattice.

### Phase 8b — Duplicate detection

Modeled on Lattice's `--duplicates` (four-section report).

- [ ] A four-tier dupe report: exact albums (normalized artist + album matched across directories), within-album multi-format (same track number/title in several formats), fuzzy similar-name candidates (a SequenceMatcher-style ratio over normalized names, threshold ~0.85), and track-level cross-library (by size/identity). Normalization mirrors Lattice: NFKC, quote/dash folding, whitespace collapse, lowercase, with a loose key that strips parentheticals and "feat." clauses.
- [ ] CLI: `duplicates <db> [--tier ...]`. Report only: no deletion, any cleanup goes through the Phase 2c mover (dry-run + undo).
- [ ] Tests: each tier against a fixture with planted duplicates; normalization equivalence; multi-format grouping.

*Usable artifact:* find duplicate albums and tracks across the managed library.

### Phase 8c — Library health audits + statistics

Modeled on Lattice's `--auditTags` / `--auditBitrate` / `--auditReplayGain` / `--missingArt` / `--auditArtQuality` / `--stats`.

- [ ] Audits: missing critical tags (title / artist / track number / genre), bitrate below a floor (default 192 kbps), ReplayGain coverage per album (missing / partial / album-missing / ok, recognizing the Opus `R128_*` convention), missing cover art, and low-resolution cover art (a pixel floor, default 500x500, measured from the cover file or embedded art). Most are expressible over the existing DB and `conservatory-search`, but the cover-resolution and ReplayGain-coverage checks need this dedicated pass.
- [ ] Library statistics: per-format counts with average bitrate, rating distribution, genre / artist / album / track totals, and total size + duration.
- [ ] Detect MP3s carrying stray APEv2 tags (they shadow ID3 in foobar2000 / DeaDBeeF and silently defeat tag edits); report-only. The **fix** (a byte-level APE strip, the `apestrip.py` technique, with optional APE→ID3 migration) lands here too, since lofty cannot strip APE on MPEG (the Phase 5b deferral); it is a small byte-surgery module, not a lofty call. The detect-and-fix split mirrors duplicates (8b) reporting then the Phase 2c mover doing the cleanup.
- [ ] (Minor) Rating normalization across player conventions on read (POPM scale differences between WMP, foobar2000, and DeaDBeeF), the Lattice `tags.py` / `rerate.py` lesson, so imported ratings land consistently on the 0 to 5 scale.
- [ ] CLI: `audit <db> [tags|bitrate|replaygain|art|artres|ape|all]`; `stats <db>`.
- [ ] Tests: each audit flags its planted-deficiency fixture and passes a clean one; stats totals match a known fixture.

*Usable artifact:* a one-command health report for the library, plus a statistics summary.

### Phase 8d — Playlist export / import (.m3u)

Modeled on Lattice's `--playlist` (rule-based smart `.m3u`), bridged to Conservatory's Perspectives (saved searches).

- [ ] Export a Perspective or an ad-hoc search expression to a `.m3u` / `.m3u8` (relative or absolute paths, configurable), and import an existing `.m3u` into a Perspective or straight into the queue, resolving paths back to managed tracks where possible.
- [ ] CLI: `playlist export <db> '<expr|vl:NAME>' <out.m3u>`; `playlist import <db> <in.m3u>`.
- [ ] Tests: exporting a search to m3u then re-importing round-trips to the same track set; missing-path entries are reported, not fatal.

*Usable artifact:* move playlists in and out of Conservatory as portable `.m3u` files.
