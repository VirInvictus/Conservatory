# Conservatory Roadmap

Phasing is deliberate and hard (spec §17): each stage must be usable on its own, so attention can swing back to Atrium between phases without leaving Conservatory half-built. The manager half (Phases 1 to 3) must be usable before the player half is finished, the player must be usable before podcasts arrive, and audiobooks (Phase 7) come last because they lean on the podcast engine.

Each top-level phase is split into independently shippable sub-phases (the way the Atrium and Viaduct roadmaps actually grew). A sub-phase carries its own checklist, a `Tests:` line, and a *usable artifact* exit condition: the thing that must work before the sub-phase is called done. Provenance (what each piece is ported or modeled from) is noted inline rather than in a separate section.

## Continuous (every phase)

These run alongside every phase rather than belonging to one; called out here so they are not forgotten.

- [x] CI: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on Linux, green from Phase 1 on. (Workflow landed at 1a: `.github/workflows/ci.yml`.)
- [x] **Observability:** a tracing subscriber in both binaries (v0.0.38). The crates emit `tracing` events (worker, player engine, podcast fetch/refresh); without an installed subscriber they were silent no-ops, which is why the player ran with no diagnostics. The GUI defaults to `info` and takes a `--debug` flag (raises our crates to `debug`: the player load / advance / buffering transitions); the CLI honours `RUST_LOG`. The Atrium / Viaduct pattern. New `tracing` log lines land with the subsystems they cover.
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
- [x] Music profile (`player::profile`, pure + tested): gapless within an album (`gapless-audio`), ReplayGain via mpv's native `replaygain` property (mpv reads the file tags `lofty` stored), with the DB `replaygain_*` columns driving mode resolution (album→track→off downgrade by available tags). Crossfade is carried through (config field) but rendered at 4b with the queue (a between-tracks behaviour). *(Later dropped at Phase 5.5a: true crossfade is impossible in a single libmpv instance; the field is removed.)* **§16.7 deferred:** read-only ReplayGain, no in-app scan. **§16.6 deferred:** no EQ/DSP in 4a.
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

## Phase 5.5 — Audio engine (DSP chain, EQ, output quality)

The music daily-driver's "feel good" phase, and a foundational refactor. Today the engine sets three mpv properties (`gapless`, `replaygain`, `audio-device`) and builds no `af` filter chain at all (`conservatory-core/src/player/`). This phase turns the flat `MusicProfile` into a **labelled `af`-chain builder** (`@rg → @eq → @comp → @boost` stages, built once per item, parameters mutated only via `af-command` so changes never tear down the graph and click the audio). That chain engine is shared infrastructure: the Phase 6c spoken-word chain (Smart Speed / Voice Boost) and the Phase 7c audiobook chain become **presets on it**, not a parallel hardcoded path, which is why it lands before the **Phase 6c spoken-word chain** (spec §17), not before all of Phase 6: the podcast manager and triage (6a/6b) are independent of the audio engine and shipped first. No new Rust dependency: every filter rides the already-linked libmpv/ffmpeg (spec §11). Resolves spec §16.6. Settled scope (spec §6.2, §6.5): a real but bounded chain (EQ + compressor/limiter/leveler) and correct output, not a deadbeef-class everything; **crossfade is dropped** (impossible in a single libmpv instance, maintainer-rejected; the dead `crossfade_seconds` key is removed), and exclusive/bit-perfect output, LADSPA/raw-`af` hosting, and crossfeed are **deferred** (recorded, not built).

### Phase 5.5a — Chain foundation + correct ReplayGain staging (headless, core) ✅ (v0.0.39)

- [x] Evolve `player/profile.rs` + `player/host.rs` from flat-field application to a **labelled `af`-chain builder** (`player/chain.rs`, `build_af_chain`): one chain per item, each stage a labelled `lavfi` filter (`@rg` now; `@eq` / `@comp` / `@boost` slots documented for 5.5b/c), set on mpv's `af` at load. (Runtime `af-command` parameter mutation lands with the first mutable stage, the EQ at 5.5b; 5.5a builds the chain fresh per item, which is gap-free across the per-item loadfile.)
- [x] **Own the ReplayGain gain stage** (fixes mpv bug #8267): `@rg:lavfi=[volume=<dB>]` at the chain *head* from the 5c-scanned `tracks.replaygain_track/_album`, recomputed per track (the host rebuilds the chain from each item's profile on load). mpv's built-in `--replaygain` is dropped (applied *after* the `af` chain; not re-applied per track across a gapless boundary). `replaygain_preamp` added; **clip-prevention is the no-peak-data gain clamp** (attenuate-only when `replaygain_clip`, the safe default) — the brick-wall limiter + peak-aware attenuation are 5.5c (the schema has no peak columns).
- [x] `--gapless-audio=weak` when gapless (preserve the source rate on a mixed-rate library), `no` for single items; `audio-samplerate`/`audio-format` left unset.
- [x] **Drop crossfade**: removed the unused `crossfade_seconds` field (`profile.rs` + `PlaybackConfig`) and its test; ship gapless-only.
- [x] CLI: `debug-dsp <db> [track_id]` prints the resolved `af` chain + the RG breakdown (mode / raw gains / preamp / clip / net dB) + gapless / speed.
- [x] Tests: `chain.rs` builder (the `@rg` string, empty when off, **different gains → different chains** = the #8267 guard); `profile.rs` resolution (mode downgrade / preamp / clip clamp / off→None / episode→None); the libmpv `playback.rs` EOF test now sets a real `@rg` chain (proves the mpv `af` syntax). Music-only build green.

*Usable artifact:* correct, per-track ReplayGain on a labelled `af` chain, headless. `debug-dsp` shows it; a track plays with the head `volume` applied (verified against the real `testdata/` albums).

### Phase 5.5b — Equalizer (core + GTK)

Split headless-first: **5.5b-i** lands the graphic EQ in the chain + persistence + the CLI (applied at load); **5.5b-ii** adds live per-band `af-command` mutation and the first GTK "Sound" preferences dialog. Settled scope: a **10-band ISO octave** graphic EQ (31 / 62 / 125 / 250 / 500 / 1k / 2k / 4k / 8k / 16k Hz); the **parametric `anequalizer`** is a later follow-up, not 5.5b.

#### Phase 5.5b-i — Graphic EQ core + persistence + CLI (headless) ✅ (v0.0.40)

- [x] Graphic EQ in the chain (`player/chain.rs`): `build_af_chain(profile, eq)` appends `@eq:lavfi=[equalizer@b0=f=31:t=o:w=1:g=…, … equalizer@b9=…]`, a stack of named `equalizer` peaking bands at the ISO centres, **only when the EQ is non-flat** (flat ⇒ no `@eq`, the no-op chain). Each band is named `equalizer@b<n>` so 5.5b-ii can target it live with `af-command`. Avoids `superequalizer`/`firequalizer` (no runtime command). `eq_stage` is pure + unit-tested.
- [x] Named EQ presets + the active state **persisted** in the DB (migration `0008`, the `perspectives` precedent): `eq_presets(name, bands CSV)` seeded `Flat`, and the singleton `eq_state` (live bands + selected preset). `EqState` model (`bands: [f64; 10]`, `preset`), reads (`get_eq_state` / `list_eq_presets` / `get_eq_preset`), worker writes (`set_eq_state` / `save_eq_preset` / `delete_eq_preset`). The engine gained `PlayerCommand::SetEq` (host holds the EQ, applied on the next load); CLI `play` sends the persisted EQ.
- [x] CLI: `eq show` (bands + preset + the resolved `@eq` chain), `eq set <band> <gain>` (±24 dB clamp; marks custom), `eq preset list|save|load|delete` (`Flat` is undeletable).
- [x] Tests: the `@eq` builder (flat → no stage; non-flat → named bands at the ISO centres; `@rg` precedes `@eq`); EqState/preset round-trips + forgiving CSV parse (`tests/eq.rs`); the migration table-exists; the libmpv EOF test now sets a non-flat `@eq` chain (proves the `equalizer@b0=f=31:t=o:w=1:g=…` mpv syntax). Music-only build green.

#### Phase 5.5b-ii — Live mutation + the GTK "Sound" dialog ✅ (v0.0.41)

- [x] Live per-band gain via `af-command` (`host.af_command` → `equalizer@b<n>`'s `gain` command, gap-free; ffmpeg's `equalizer` supports it). `host.set_eq_band` does the live path when the `@eq` stage is present, and a **structural rebuild** only at the flat↔non-flat boundary (the stage appears / disappears) or on a preset switch (`set_eq` applies-when-playing). The host now keeps the `current_profile` so it can rebuild mid-playback. Engine `SetEqBand` + `PlayerHandle::set_eq_band`; a pure `eq_band_command` (unit-tested: a band change maps to `af-command @eq gain <dB> b<n>`).
- [x] First GTK preferences surface: a "Sound" `adw::PreferencesPage` in an `adw::PreferencesDialog` (the app's first; Phase 10 builds on it), opened by a header button or `Ctrl+,`. An Equalizer group of 10 vertical sliders (−12..+12 dB, 0-detent) under their ISO centre labels + a preset `ComboRow` (the saved presets + "Custom") + Save as… / Delete / Reset. Sliders drive the engine live and select "Custom"; preset/reset push the whole state; persistence is on close (slider edits) and immediate (explicit actions). The persisted EQ is also pushed to the engine at startup (`apply_persisted_eq`), which the GUI never did before.
- [ ] (Later) the parametric option via `anequalizer` (per-band freq/Q/gain, live `change`).
- [x] Tests: the `eq_band_command` mapping (no chain rebuild); an engine null-host integration that mutates bands live mid-playback and still reaches EOF (the real mpv `af-command` path); the `match_preset` projection; build + manual for the dialog. Music-only build green.

*Usable artifact:* (5.5b-i) a graphic equalizer with persisted presets via the CLI, applied to playback; **(5.5b-ii) the same with live sliders in the Sound preferences dialog — drag a band and hear it move.** **Phase 5.5b is complete.**

### Phase 5.5c — DSP modules + output quality (core + GTK)

Split headless-first, the 5.5b rhythm: **5.5c-i** lands the DSP modules in the chain + persistence + CLI; **5.5c-ii** adds output backend / resampler control and consolidates the GTK "Sound" page. Settled scope (spec §6.2, §16.6): a bounded, useful chain (compressor + brick-wall limiter + `dynaudnorm` leveler), not a deadbeef-class everything.

#### Phase 5.5c-i — DSP modules core + persistence + CLI (headless) ✅ (v0.0.42)

- [x] DSP modules as toggleable chain stages (`player/dsp.rs`, pure): compressor (`acompressor`), a brick-wall limiter (`alimiter`, `level=disabled` so it is a transparent peak catcher and the ReplayGain clip safety net), and a volume leveler (`dynaudnorm`, single-pass/live; NOT `loudnorm`, whose accurate mode is two-pass/offline-only). `build_af_chain(profile, eq, dsp)` appends `@comp` / `@limit` / `@boost` after `@eq` in signal-flow order, each present only when its module is enabled. User-facing dB knobs (compressor threshold, limiter ceiling) are converted to the filters' linear forms in the one stage builder.
- [x] Each module is `{ enabled, settings }` (`ModuleState<T>`): the settings persist while the module is off, so toggling a tuned module back on restores its parameters. The host holds the active `DspState` and applies it on each load; a settings change does a structural `af` rebuild (an explicit settings change, gap-acceptable; DSP has no per-slider live path like the EQ). Engine `SetDsp` + `PlayerHandle::set_dsp`.
- [x] Persisted in the singleton `audio_state` table (migration `0009`, the `eq_state` precedent): the playback defaults (ReplayGain mode / preamp / clip, gapless), the DSP modules, and the output backend / resampler — one row holding the whole audio config, so 5.5c-ii needs no second migration. `AudioState` model + `ResamplerQuality` enum, forgiving read (`get_audio_state`), worker write (`set_audio_state`). (The playback + output halves are consumed at 5.5c-ii; 5.5c-i seeds and stores them.)
- [x] CLI: `dsp show` (modules + the resolved `@comp` / `@limit` / `@boost` chain), `dsp comp on|off [--threshold --ratio --attack --release]`, `dsp limiter on|off [--ceiling]`, `dsp leveler on|off [--target --gausssize]`; `debug-dsp` prints the DSP breakdown; `play` applies the persisted DSP.
- [x] Tests: `dsp.rs` builders (off → no stage; on → expected lavfi string + correct dB→linear conversion); `chain.rs` full-chain order (`@rg < @eq < @comp < @limit < @boost`); `AudioState` round-trip + params-survive-an-off-toggle through the worker (`tests/audio_state.rs`); the migration table-exists; the libmpv EOF test now sets a real `@comp`/`@limit`/`@boost` chain (proves the `acompressor`/`alimiter`/`dynaudnorm` mpv syntax). Music-only build green. No new dependency.

Split headless-first, the 5.5b/5.5c rhythm: **5.5c-ii-a** lands the output backend / resampler apply in the player host + the engine commands + the CLI `output` verb group (CLI-testable); **5.5c-ii-b** consumes the persisted config in the GUI and consolidates the "Sound" page.

##### Phase 5.5c-ii-a — Output backend + resampler apply + CLI (headless) ✅ (v0.0.43)

- [x] Output quality applied in the player host: the **backend** (mpv `ao`, switched live via `ao-reload`, gap-acceptable) and the high-quality **resampler** knobs (`audio-resample-filter-size` / `-cutoff`, re-asserted per load; avoid-resample stays the default, `audio-samplerate` / `audio-format` left unset). `MpvHost` holds both; the seeded-but-unused `audio_state.output_backend` / `resampler_quality` (migration `0009`) are finally consumed.
- [x] Engine plumbing: `PlayerCommand::SetOutputBackend` / `SetResamplerQuality` + `PlayerHandle::set_output_backend` / `set_resampler_quality` (the `SetDsp` precedent). `PlaybackConfig::from_audio_state` maps the persisted playback defaults (RG mode / preamp / clip, gapless) into the resolver, kept in `player/profile.rs` so the db layer stays free of the `ReplayGain` enum.
- [x] CLI: `output show` / `output backend <auto|pipewire|pulse|alsa|jack>` / `output resampler <default|high>` (read `get_audio_state`, write `worker.set_audio_state`, the `dsp` verb precedent); `debug-dsp` gains the backend + resampler line.
- [x] Tests: host null-AO integration (`set_output_backend("null")` exercises the `ao` + `ao-reload` path hermetically; `set_resampler` High/Default; the EOF smoke now re-asserts a High resampler through `load`); `PlaybackConfig::from_audio_state` mapping (each mode + forgiving fallback); the CLI verbs verified end-to-end against a fixture DB. Music-only build green. No new dependency, no new migration.

##### Phase 5.5c-ii-b — The consolidated GTK Sound page ✅ (v0.0.44)

- [x] The "Sound" page (from 5.5b) consolidates the full chain: ReplayGain (mode / preamp / clip), EQ, DSP modules (each an `adw::ExpanderRow` with an enable switch + tuning rows, the app's first `ExpanderRow`), output backend / device / resampler, gapless. Defaults flow into `build_play_queue` / `build_mixed_queue` instead of `PlaybackConfig::default()` (a `playback_config()` read of `audio_state` via `PlaybackConfig::from_audio_state`), and a startup `apply_persisted_audio` hook pushes the DSP / backend / resampler into the engine (the `apply_persisted_eq` precedent; also fixing that 5.5c-i's DSP was stored but never GUI-applied). The device picker stays in the header too (a second write-through `ComboRow` in the page). DSP / output drive the engine live; ReplayGain / gapless resolve per-queue. The whole `audio_state` persists on dialog close.
- [x] **Deferred and recorded (not built):** exclusive/bit-perfect output (ALSA `hw:` + `--audio-exclusive`, bare-install-only, fights the Flatpak sandbox); LADSPA / raw-`af` escape hatch (needs the `org.freedesktop.LinuxAudio.Plugins` extension + ffmpeg `--enable-ladspa`); native `crossfeed` headphone module (cheap, a natural future stage); the parametric `anequalizer`; peak-aware ReplayGain attenuation (no peak columns; the limiter is the safety net).
- [x] Tests: the pure picker-mapping helpers in `ui/sound.rs` (`option_index` / `option_value` / `option_labels`, forgiving fallback + round-trip); the dialog widgets build + manual launch. Music-only build green. No new dependency, no new migration.

*Usable artifact:* (5.5c-ii-a) the output backend + resampler are driven from the CLI and persist (`output backend … pipewire` / `output resampler … high`), with the playback defaults flowing into the queue builders; **(5.5c-ii-b)** a foobar2000-style Sound/DSP preferences surface over the music engine. **Phase 5.5c-ii and Phase 5.5 are complete; the music daily-driver's audio engine is done.**

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

##### Phase 6a-ii-b — feed-rs/namespace parse + refresh + CLI ✅

- [x] Parse (`parse.rs`): `feed-rs` for the RSS/Atom/JSON core plus Belfry's hand-rolled `podcast:` namespace handler (`namespace.rs`, ported, merged to feed-rs entries by position with a guid cross-check); a fresh `Entry → ParsedEpisode` mapping (enclosure from `entry.media`, guid = item `podcast:guid` else feed-rs id, pub_date/duration/season/episode/type); episode identity by `(show_id, guid)`. The namespace handler was **extended** to also read `itunes:season`/`itunes:episode`/`itunes:episodeType` (feed-rs surfaces none, and real feeds carry them there; `podcast:` wins). Show-note sanitize (`ammonia`) and chapter storage stay deferred to 6a-iii / 6b (the chapters URL is captured, not persisted). `feed-rs` / `quick-xml` signed off (ATTRIBUTIONS.md).
- [x] A `slugify` + `episode_dir` for the managed `Podcasts/<slug>/<date>--<ep-slug>` layout (spec §5.3, `slug.rs`). The refresh orchestration (`refresh.rs`) fetches concurrently under a `Semaphore(REFRESH_PARALLELISM)` (`tokio::task::JoinSet`), parses, upserts episodes through the 6a-i worker methods, and stamps `etag` / `last_modified` / `last_fetched` on the show (preserving the user-configured fields; triage is 6b). CLI `podcast add|remove|refresh` behind `#[cfg(feature = "podcasts")]`; the plugin gained a `conservatory-core` dep to drive the typed worker methods (no cycle; the §2.2 boundary is code, not schema).
- [x] Tests: `(show_id, guid)` dedup through a refresh, the podcast/itunes namespace parse + precedence, conditional-GET round-trip (etag stored on `add` then replayed for a 304), against `wiremock` feed fixtures + a real core worker on a temp DB.

*Usable artifact:* `podcast add <url>` then `podcast refresh` subscribes to and pulls a show's episodes entirely headless.

#### Phase 6a-iii — OPML + credentials + download (headless)

Split: **6a-iii-a** is OPML round-trip (network-free, no new deps); **6a-iii-b** is the credential store (`oo7`) + HTTP Basic auth wiring + episode download.

##### Phase 6a-iii-a — OPML round-trip ✅

- [x] OPML import/export round-trip, preserving tags and `applePodcastsID` (spec §8). `conservatory-podcasts/src/opml.rs`: `parse_opml` (forgiving, flattens folder hierarchy, every `<outline>` with an `xmlUrl` is a subscription; title = `title` else `text` else URL; tags from the Pocket Casts `category="a,b"`) and `write_opml` (OPML 2.0, XML-escaped). Import is **network-free**: `import_opml` creates the shows + tags through the worker (`get_or_create_show` / `get_or_create_tag` / `set_show_tags`), `applePodcastsID` into `shows.apple_podcasts_id`; episodes arrive on the next `refresh`. `export_opml` reads shows + tags back out.
- [x] CLI: `import-opml <db> <file>` / `export-opml <db> [--out <file>]` behind `#[cfg(feature = "podcasts")]`. No new deps (`quick-xml` from 6a-ii-b).
- [x] Tests: round-trip with escaping + the forgiving/nested/foreign parse + the title fallback (unit); import-through-a-worker creates shows + tag links + the Apple id, a re-import is idempotent, and export-then-reparse round-trips (`tests/opml.rs`).

*Usable artifact:* `export-opml` backs up your subscriptions; `import-opml` brings a list in (then `podcast refresh` pulls episodes).

##### Phase 6a-iii-b — Credentials (oo7) + episode download ✅

- [x] A `CredentialStore` (an enum, not a `dyn` trait, so the async methods stay simple) with an `oo7`/libsecret backend and an in-memory test backend; `shows.auth_pass_ref` stores the lookup key, never the password (spec §8). HTTP Basic auth wired into the fetch path (`fetch_authed`) from `(auth_user, password)`; `refresh_show` / `refresh_all` resolve a show's credential and attach it. `oo7` activated (`default-features = false` + `tokio` + `native_crypto`, since the default async-std runtime clashes with the workspace's tokio).
- [x] Episode `download` into the managed `Podcasts/<slug>/<date>--<ep-slug>/` tree (spec §5.3): stream to a sibling `.part` file, fsync, rename (the `mover::fsops` crash-safe shape); a new core `set_episode_audio_path` worker command + `get_episode` read record/resolve the row (`upsert_episode` preserves `audio_path`, so download sets it explicitly). CLI `podcast download <db> <episode_id> --root <root>`.
- [x] Tests: credential store round-trip + resolve rules (in-memory backend); a Basic-auth-gated download (401 without creds, 200 with, the password flowing through the store); download writes the file and sets `audio_path`; filename derivation. Hermetic (`wiremock` + a temp-DB worker).

*Usable artifact:* **Phase 6a (the headless podcast subsystem) is complete.** A private (Basic-auth) feed refreshes, and an episode downloads into the managed tree. What remains for podcasts is the GUI (Phase 6b).

### Phase 6b — Podcasts tab + triage

Split so the window-root restructure is isolated from the podcast feature work: **6b-i** turns the single-view music window into the multi-view shell of spec §2.3 (the adaptive `AdwViewSwitcher` over an `AdwViewStack`, Music as the first page, an empty Podcasts page); **6b-ii** fills that page with Belfry's triage. The shell is where the second tab first exists, so it lands here rather than implicitly inside the triage work.

#### Phase 6b-i — Window shell (AdwViewStack + adaptive view switcher) ✅

- [x] Restructure the window root (spec §2.3): the music browse moves out of `AdwToolbarView`'s content into an `AdwViewStack` page ("Music"); the header gains an `AdwViewSwitcher` (`policy = wide`) bound to the stack. `AdwViewSwitcherTitle` is deprecated since libadwaita 1.4 and is not used. (The always-on filter bar moved from a global top bar into the Music page so it does not show over the Podcasts tab; its behaviour is unchanged.)
- [x] Adaptive collapse: an `AdwBreakpoint` (max-width 550sp) hides the header switcher and reveals a bottom `AdwViewSwitcherBar` on narrow widths.
- [x] Now-bar / switcher-bar stacking (spec §2.3): the persistent Now-bar stays the stable innermost bottom bar; the `AdwViewSwitcherBar` is added after it (reveals *beneath* it) only at the narrow breakpoint.
- [x] Feature-gated: the switcher, bottom bar, breakpoint, and Podcasts page exist only behind `#[cfg(feature = "podcasts")]` (the binary's first feature gates); `--no-default-features` (music-only) keeps a single-page stack with no switcher chrome, visually unchanged (spec §2.2, §2.3). (7b generalises the gate to include `audiobooks`.)
- [x] Lazy page construction: the Podcasts page builds its child on the child's first `::map` (an empty placeholder until 6b-ii); `AdwViewStack` retains each page's widget state once built.
- [x] Keyboard: `Alt+1` / `Alt+2` / `Alt+3` switch top-level views (a global `ShortcutController`, the AdwTabView convention; `Ctrl+1/2/3` left free for the podcast triage lists). `Alt+3` is inert until the Audiobooks tab (7b). `docs/keymap.md` updated.
- [x] Tests: the `Alt+N` → page-name mapping is a pure unit test; a launch smoke confirms the new tree constructs and runs cleanly; the music-only build compiles + runs with no switcher widgets present.

*Usable artifact:* the GUI is a multi-view window. Switch between Music and an empty Podcasts tab, the switcher collapsing to a bottom bar on narrow widths, the Now-bar persistent across both. The music-only build is visually unchanged (no switcher).

#### Phase 6b-ii — Triage panes

Split: **6b-ii-a** is the read-only three-pane browse (sidebar + episode list + detail); **6b-ii-b** is the triage actions (mark played/archived, star) + the Tags sidebar (DB + GUI, no engine); **6b-ii-c** is episode playback + the unified queue + per-show overrides (the engine-touching half).

##### Phase 6b-ii-a — Triage browse (read-only) ✅

- [x] The Podcasts view (spec §3.7): a sidebar of triage buckets (Inbox / Queue / Played) and subscribed shows, an episode list (`ColumnView`) with a played-state glyph + title/date/length, and a detail pane with show notes, filling the 6b-i page (built lazily on `::map`). `conservatory/src/ui/podcasts.rs` (nested `gtk::Paned`; AdwNavigationSplitView is a later refinement). Tags sidebar section deferred to 6b-ii-b (needs a tag-filtered read).
- [x] Core triage reads: `EpisodeListRow` + `episodes_for_show` + `episodes_in_bucket` (the §4.2 derivation: Queue = unified-queue membership, Played = `played >= PlayedFully`, Inbox = the rest). CLI `podcast episodes <db> [--show <id> | --bucket inbox|queue|played]` (the headless surface).
- [x] Tests: the bucket derivation (core integration test); `EpisodeRow` formatting (unit); the GTK view's construction (display-guarded build test). Music-only build stays green.

*Usable artifact:* open the Podcasts tab and browse your subscriptions: pick Inbox/Queue/Played or a show, see its episodes with state, read the show notes.

##### Phase 6b-ii-b — Triage actions + Tags ✅

- [x] Triage transitions (mark played / unplayed / archived, star) via **partial** playback upserts (`set_episode_played` / `set_episode_starred`, so an action never clobbers a sibling field; marking unplayed rewinds the position). A detail-pane action bar in the GUI; the list glyph + bucket counts refresh after each action. CLI `podcast mark` / `podcast star` (the headless surface).
- [x] A Tags sidebar section: `list_all_tags` + a tag-filtered `episodes_for_tag` read; `Source::Tag` in the view.
- [x] Tests: the partial writes (mark-played keeps starred and vice-versa; mark-unplayed rewinds; archived → ArchivedUnlistened) + the tag-filter read + bucket reflection, a core integration test.

*Usable artifact:* the Podcasts inbox is actionable: mark episodes played / archived, star them, filter by tag, all reflected live.

Episode playback splits again, because the per-kind persistence + resume is the engine-risky part: **c-1** plays episodes forward (persistence guarded to track-only); **c-2** adds episode resume + per-kind persistence; **c-3** adds per-show overrides.

###### Phase 6b-ii-c-1 — Episode playback (forward) ✅

- [x] The structural change from Belfry: **Queue is the shared unified queue**, so an episode and a track sit next to each other. `enqueue_episodes` / `replace_queue_with_episodes`; the `load_queue_display` episode join (else queued episodes render blank); `build_episode_queue` (downloaded file `root`+`audio_path`, else the enclosure URL; libmpv `loadfile` streams a URL as-is) + a basic `resolve_episode_profile`; `EpisodeListRow`/`EpisodeRow` carry `audio_path`/`audio_url`.
- [x] **Per-kind persistence guard:** the engine persists position + play counts only for `MediaKind::Track`, so an episode plays to EOF but never writes an episode id into the music `playback_state` / `tracks.play_count`. (Episode resume + per-kind persistence is c-2.)
- [x] GUI: the Podcasts episode list plays on double-click / Enter (the visible list from that row) and appends on `Ctrl+Enter`, the music leaf idiom.
- [x] Tests: episode queue write + display join (core); `build_episode_queue` local/stream/skip (unit); an engine null-sink test playing an episode to EOF that asserts the guard held; the existing track-playback test still passes (music-regression check).

*Usable artifact:* double-click an episode in the Podcasts list and it plays (downloaded or streamed) in the unified queue, with the Now-bar + queue drawer.

###### Phase 6b-ii-c-2 — Episode resume + per-kind persistence ✅

- [x] The engine writes the podcast `playback` table on episode tick/EOF (position + `played`/`play_count`), not the music `playback_state` singleton; the resume cursor learns the current item's kind so a restart resumes an episode to the second. Migration `0007` adds `playback_state.kind` + `episode_id` (the cursor); new partial-upsert worker writes `set_episode_position` (InProgress, preserving starred/play_count) and `complete_episode` (PlayedFully + play_count bump) carry the per-episode state. The engine's three persistence sites dispatch by `MediaKind` (the episode position write is synchronous + guarded on `!ended` so a terminal flush cannot clobber the completion). The CLI `play` and GUI `resume_saved_queue` rebuild a mixed track+episode queue (`build_mixed_queue`) and resume at the cursor's `(kind, id)`.
- [x] Tests: episode persistence + cursor round-trip through the worker (`podcasts.rs`); an episode plays to EOF writing the podcast `playback` row (PlayedFully + play_count) but never the music cursor, with a colliding track untouched (`queue.rs`); `build_mixed_queue` interleave / skip / cursor re-index (`playqueue.rs`); the music-only build stays green.

###### Phase 6b-ii-c-3 — Per-show overrides

Split, because the six settings span playback (resolved into the profile) and management (refresh-time routing + retention), and a GUI editor: **c-3-a** is per-show playback speed, headless; **c-3-b** is inbox-policy routing + retention; **c-3-c** is the GUI per-show settings panel. Smart Speed / Voice Boost are flags carried for Phase 6c (the `af`-chain), not built here.

###### Phase 6b-ii-c-3-a — Per-show playback speed (headless) ✅

- [x] `MusicProfile` (the de-facto single per-item profile, spec §6.1) gains `speed` + `pitch_correction`; `resolve_episode_profile(Option<&ShowSettings>)` resolves the speed from the show's `playback_speed` (clamped to `[0.25, 4.0]`, pitch correction on), music stays at 1.0. `MpvHost::load` applies mpv `speed` + `audio-pitch-correction` (scaletempo2) before `loadfile` (1.0/off is a no-op for the track path).
- [x] The episode-queue builders (`build_episode_queue`, `build_mixed_queue`, the CLI `resolve_queue_items`) thread each episode's show settings in: `EpisodeSource`/`MixedQueueRow`/`QueueDisplayRow` carry `show_id`, a new core `show_settings_map` batch-reads them, and the builders resolve speed per show. `EpisodeRow` exposes `show_id`.
- [x] CLI `podcast settings <db> <show_id> [--speed N]` views or sets a show's overrides (preserving the other fields), the headless surface.
- [x] Tests: profile speed resolution + clamp; `build_episode_queue` / `build_mixed_queue` apply per-show speed; a host integration test asserts `load` sets mpv's `speed`; music-only build green.

*Usable artifact:* set a show's speed (`podcast settings ... --speed 1.5`) and its episodes play at that rate with corrected pitch, in the unified queue.

###### Phase 6b-ii-c-3-b — Inbox-policy routing + retention ✅

- [x] Apply a show's `inbox_policy` to new episodes on refresh (Inbox / AlwaysQueue / AlwaysArchive); prune downloaded episodes beyond `keep_count` (retention). These are management settings (`conservatory-podcasts`), not playback. Routing rides `refresh::apply_feed`: the show's settings are read once (default Inbox when absent), and **only genuinely-new episodes route** (a re-refresh never re-queues one the user removed); `AlwaysQueue` enqueues to the unified queue, `AlwaysArchive` marks `ArchivedUnlistened`, `Inbox` is a no-op (the §4.2 derivation). Retention is a separate **root-aware** pass (`retention.rs`: `plan` → `apply`, the mover's dry-run-then-apply shape): downloaded episodes beyond `keep_count` (0 = keep all) lose their file + `audio_path` (revert to stream-only); a new `clear_episode_audio_path` worker command + the `podcast prune <db> [show_id] --root [--apply]` verb (dry-run default).
- [x] Tests: a new episode routes per policy and an already-seen one does not re-route (`refresh.rs`); retention prunes the oldest downloads, keeps the newest, ignores `keep_count = 0` and stream-only episodes (`retention.rs`); music-only build green.

###### Phase 6b-ii-c-3-c — GUI per-show settings panel ✅

- [x] A per-show settings surface in the Podcasts detail pane (spec §3.7): speed, Smart Speed / Voice Boost toggles (the flags 6c consumes), skip intro/outro, inbox policy. Writes through `upsert_show_settings`. A gear button appears in the detail pane when a **show** is the selected sidebar source, opening an `adw::AlertDialog` whose `extra_child` is an `adw::PreferencesGroup` (`SpinRow` speed/skip, `SwitchRow` Smart Speed/Voice Boost, `ComboRow` inbox policy), pre-populated from `get_show_settings` (or the schema-default skeleton). Reuses the bulk-edit dialog idiom + the `rt.block_on(worker.*)` write idiom; the working episode-detail flow is untouched. First use of the libadwaita preference-row widgets (no new dep; libadwaita 0.7.2). The panel preserves the global-inherit `skip_forward`/`skip_back` fields it does not expose.
- [x] Tests: the pure form mapping (`inbox_policy_*` index round-trip, out-of-range degrade; `settings_from_form` applies edits + preserves the skip fields; `default_settings`) unit-tested headless in `conservatory/src/ui/podcasts.rs` (constructs no widgets); the dialog itself is build + manual (the 3b/3c GUI precedent). Music-only build green.

*Usable artifact:* podcasts play in the one queue (downloaded or streamed), resuming where you left off, with per-show speed/boost settings editable in the GUI. (Smart Speed / Voice Boost filters are 6c.) **Phase 6b-ii-c (episode playback + per-show overrides) and the c-3 split (a speed / b routing+retention / c GUI) are complete; 6b-ii is done.**

### Phase 6c — Podcast playback profile + parity

Split into the audio engine (6c-i / 6c-ii, the parity-critical half, done first) and a follow-on for the surfacing work (6c-iii+). Belfry retires only when the whole of 6c lands (spec §16.8), so the follow-on is part of parity, not optional.

Two filter choices were settled against the 5.5 findings (`docs/libmpv-profiles.md`): variable speed via mpv `--speed` + `audio-pitch-correction` (scaletempo2) rather than a chained `rubberband` at every speed, and live single-pass `dynaudnorm` rather than two-pass/offline `loudnorm`. Smart Speed is the `silenceremove` af-filter (spec contract). **A consequence: `rubberband` is no longer actually used**, so at 6c-i the spec §15 / ATTRIBUTIONS "GPL-3 forced by librubberband" rationale was re-confirmed and reworded: GPL-3 is forced by the **linked GPL stack** (the GPL ffmpeg build the chain filters ride, and librubberband where the mpv build carries it), not by a `rubberband` call we make. GPL-3 stands.

#### Phase 6c-i — Smart Speed + Voice Boost af stages (headless core + CLI) ✅ (v0.0.45)

- [x] Podcast profile ported from Belfry §5: Smart Speed (silence-skip via `silenceremove`) and Voice Boost (compression + voice EQ + live `dynaudnorm`), **built as presets on the Phase 5.5 `af`-chain engine, not a parallel hardcoded path** (`player/spoken.rs`: `smart_speed_stage` `@ss`, `voice_boost_stages` `@vbcomp`/`@vbeq`/`@vbnorm`). `MusicProfile` carries `smart_speed` / `voice_boost`; `resolve_episode_profile` sets them from the show settings (false for music, so the music chain is unchanged). `build_af_chain` appends them after the music stages, Smart Speed first.
- [x] CLI `podcast debug-chain <ep>` dumps an episode's resolved chain; the GUI per-show dialog's "audio processing arrives later" caption is gone.
- [x] Tests: the `spoken.rs` builders; `build_af_chain` episode-vs-music; the libmpv `ao=null` EOF run sets both flags (proves the `silenceremove` + Voice Boost mpv syntax decodes). Music-only build green. No new dependency, no new migration.

#### Phase 6c-ii — Time-saved accounting (headless core + CLI) ✅ (v0.0.46)

- [x] Episodes share the unified queue and the per-item profile switch prototyped in 4b; **append-only `listening_sessions` discipline**. The engine writes one session row per episode boundary (`player/session.rs`'s pure `SessionAccumulator` + the engine's start-on-load / close-on-boundary wiring); `smart_speed_saved = max(0, audio_seconds/speed − real_seconds)` (the non-linear-timeline math `silenceremove` requires, with its forward playhead jumps counted as covered audio), with user-seek ticks excluded and pauses resynced rather than accrued. `insert_listening_session` (write) + `listening_totals` (aggregate read); CLI `podcast stats`. No new migration, no new dependency.
- [x] Tests: the accounting math (seek-excluded ticks, Smart-Speed-off ≈ 0, variable-speed nets zero, pause accrues nothing, divide-by-zero guard); a `listening_sessions` append round-trip; an engine null-host run lands exactly one session row with sane totals.

#### Phase 6c-iii+ — Chapters + Now Playing additions (the follow-on)

Split into the chapters core (persistence then navigation) and the surfacing work:
**a** = chapter persistence, **b** = chapter navigation (the shared engine
mechanism 7c reuses), **c** = the Now Playing episode surface, **d** = sleep timer.

##### Phase 6c-iii-a — Chapter persistence (headless + CLI) ✅ (v0.0.47)

- [x] Persist the parsed chapter set (the 6a-ii note: the `podcast:chapters` URL was captured but not stored). `conservatory-podcasts/src/chapters.rs` fetches the URL and parses the Podcast Index JSON; `refresh::apply_feed` stores it for each genuinely-new episode through the existing `replace_chapters` worker command (best-effort: a fetch/parse failure is logged, never fatal). CLI `podcast chapters <ep>`. `serde`/`serde_json` activated in the podcasts crate. ID3-CHAP fallback (from a downloaded file) stays deferred to the -c fold-in.
- [x] Tests: the JSON parser (full / empty / malformed / blank strings); a wiremock refresh that serves a feed + its chapters JSON and asserts the set lands.

##### Phase 6c-iii-b — Chapter navigation (the shared engine mechanism) ✅ (v0.0.48)

- [x] A **skip-to-next / skip-to-previous-chapter** transport action (an absolute `seek` to the neighbouring `chapters.start_time`) wired to buttons in the Now-bar and a keybinding (`Ctrl+Shift+←/→`). Built generic in the core player (`player/chapters.rs`: `ChapterMark` on `PlayableItem`, pure `current_chapter_at` / `neighbour_chapter` helpers, a `SkipChapter` command + snapshot `current_chapter`/`chapter_count`) so the audiobook engine reuses it at 7c with `book_chapters` (not a podcast-only path). The GUI attaches marks after a queue build (`attach_episode_chapters`); the Now-bar chapter buttons appear only for a chaptered item.
- [x] Tests: the `neighbour_chapter` helpers (forward / back / clamped at the ends); an engine skip forward-to-boundary / back-to-start (paused); the filter-graph swap between a track and an episode mid-queue (both play to completion, proving the §16.9 profile switch). No new dependency, no new migration.

##### Phase 6c-iii-c — Now Playing episode surface ✅ (v0.0.49)

- [x] Now Playing additions for episodes: a clickable chapter list in the bottom drawer (jump-to via `seek`, current-chapter highlight that follows the playhead), `ammonia`-sanitized show notes, and a Smart Speed indicator (live saved time). Show notes are cleaned **at ingest** (`conservatory-podcasts/src/notes.rs`, HTML → plain text), so every reader benefits and the DB column is clean. The snapshot gained `smart_speed_active` / `smart_speed_saved`; the chapter highlight + Smart Speed line tick from the existing 250 ms poll (a class toggle, not a rebuild). `ammonia` activated in the podcasts crate.
- [x] Tests: the `sanitize_notes` cases (tags / entities / paragraph breaks / dropped `<script>` / malformed / blank-line collapse); a snapshot assertion that an episode with Smart Speed on reports it active. No new migration.
- ID3-CHAP embedded-chapter fallback (from a downloaded file, `id3`) stays deferred (a small later fold-in).

##### Phase 6c-iii-d — Sleep timer ✅ (v0.0.52)

- [x] Sleep timer (15 / 30 / 45 / 60 min, end of episode, end of queue, tap-to-extend; Belfry §3.6), the `S` keybinding. Built engine-side as a pure, unit-tested clock (`player/sleep.rs`: `SleepMode` `After`/`EndOfItem`/`EndOfQueue` + `SleepClock`) the engine ticks each loop turn: a duration timer counts down only while playing and pauses on elapse (the session-accumulator idle precedent), opening a 30 s tap-to-extend window that the next `Play` re-arms; "end of item" pauses cued on the next item at the EOF boundary; "end of queue" disarms when the queue ends. Transient per-session state, so no DB persistence and no migration. **Media-agnostic** (the user's scope decision: an album track gets a sleep timer too, broadening spec §3.6's episode-only wording; the menu's boundary row reads "End of track" / "End of episode" / "End of book" by kind). Surfaced as a Now-bar moon `MenuButton` (the `build_output_menu_button` popover idiom; shows the `M:SS` remaining for a duration timer) plus a "Sleep · …" line in the Now Playing drawer; `S` pops the menu (a window-local controller so the bare letter does not fire in the filter entry). CLI `play --sleep <15|30|45|60|episode|queue>` arms it headless.
- [x] Tests: the `sleep.rs` clock (count-down-only-while-playing, fire-at-zero, tap-to-extend inside/outside the 30 s window, boundary modes have no countdown); an engine null-host run where a duration timer fires + pauses mid-queue then tap-extends, and an "end of item" timer pauses at the first item's boundary without playing the second (`tests/sleep.rs`); the GUI label helpers (`fmt_sleep_remaining` rounds up, `sleep_boundary_label` by kind, `sleep_drawer_text`). No new dependency, no new migration. Music-only build green.

*Usable artifact:* **podcast parity reached.** One queue, one engine, both media types, full Smart Speed / Voice Boost, with the sleep timer the last piece. **Belfry has now retired** (v0.0.52, spec §16.8): the GitHub repo is archived and the `~/.gitrepos` project map updated; the local clone is kept frozen as reference.

---

## Phase 7 — Audiobooks (the third tab)

Audiobooks are the third media type (spec §3.8), landing as the **`conservatory-audiobooks` plugin crate** (spec §2.2). They are long-form speech, so they reuse the absorbed spoken-word engine (Smart Speed, Voice Boost, variable speed, sleep timer) from Phase 6c and the unified queue; that is why this phase lands after podcast parity. The data model, import, and browse surface are modeled on **Cozy** (the GTK4/libadwaita audiobook player); the metadata model and folder conventions on **Audiobookshelf**; chapter handling technique on **m4b-tool** (all three cloned under `~/.gitrepos/` as read-only reference, ATTRIBUTIONS.md). Belfry's retirement at 6c is unaffected. Metadata is local-source-only in v1 (online providers deferred, spec §16.10).

### Phase 7a — Audiobook model + import (headless)

Split i/ii/iii like Phase 6a: **7a-i** the schema foundation (migration + models + worker CRUD), **7a-ii** the tag/sidecar reader + chapter resolver (carries the embedded-M4B-chapter-reading dependency decision), **7a-iii** the path template + import pipeline + CLI.

- [x] **Schema (7a-i, v0.0.53):** migration `0011` per spec §4.5: `book_people` (authors + narrators, role-tagged), `series`, `books`, `book_authors`, `book_narrators`, `book_chapters`, `book_playback`. `book_fts` (title, author, narrator, series) trigger-synced (spec §4.4), with author/narrator/series denormalized from the link tables. The unified `queue` gains the `book_id` foreign key (the deferred 0006 rebuild). Core models + read helpers + single-writer CRUD, with a worker round-trip + FTS + queue-FK test suite. The migration lands in `conservatory-core`'s ledger, not the plugin crate (spec §2.2).
- [x] **Tag + sidecar reader (7a-ii, v0.0.54):** embedded M4B/ID3 tags (lofty), then the Audiobookshelf sidecar conventions (`.opf` via `quick-xml`, `desc.txt`, `reader.txt`, `cover.jpg`), then folder structure, into a `BookDraft` (author, narrator, series + decimal sequence, year, publisher, ISBN/ASIN, description, language). Author and narrator are distinct roles, merged by precedence sidecar > tags > folder. Custom `NARRATOR`/`SERIES`/`SERIES-PART` frames are read across both ID3v2 `TXXX` and MP4 freeform spellings; people sort last-name-first (`person_sort_name`, not the music article-move). `conservatory-audiobooks` filled; `audiobook debug-read` CLI artifact.
- [x] **Chapter resolver (7a-ii):** embedded M4B markers (via an `ffprobe -show_chapters` shell-out, no Rust MP4 dep, the rsgain precedent) → else one-file-per-chapter folder (ordered by the part tag) → else a whole-file single chapter. Each chapter is a `(file_path, file_offset, duration)` draft addressing either a standalone file or an M4B span. Opt-in silence detection stays deferred (spec §16.11).
- [x] **Audiobook path template (7a-iii, v0.0.55):** `Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})`, the series components collapsing for standalone books (a standalone renders under the literal `Standalone`, so every author folder is two levels deep, spec §5.7). The new tokens (`{author}`, `{narrator}`, `{series}`, `{series_index}`) extend the Phase 2a engine through a small `TemplateFields` trait shared with the music `TrackFields`, so music rendering is byte-for-byte unchanged (the existing 17 path tests guard it). The series index is decimal-aware: an integral `1.0` zero-pads to `01`, a fractional `1.5` renders unpadded.
- [x] **Import pipeline + the mover learns about books (7a-iii):** the plugin's `import_book` resolves a `BookDraft` into rows and moves the book's files into the managed tree via the Phase 2c mover (dry-run + undo, books owned like music, not ephemeral like podcasts). The journaled mover gains a `book_id` column (migration `0012`): a book op rewrites every `book_chapters` row of the book whose path matches the moved file, so a single M4B that backs many chapters follows the one moved file. Move ops are built per unique physical file, not per chapter. A conflict refuses the whole import with no rows written; one book per call.
- [x] **Cover + accent (7a-iii):** the Hermitage median-cut accent into `books.accent_rgb` at insert, and the cover written into the moved book folder via the existing folder-based cover sync (`set_book_cover_path`), the music-import shape (spec §7.4).
- [x] **CLI (7a-iii):** `audiobook import` (copy default, `--move`, a conflict refuses with a nonzero exit) and `audiobook set` (rating / starred / shelf genre; path-affecting edits deferred to 7b). Undo is the existing media-agnostic `organize --undo <job>` (spec §9).
- [x] **Tests (7a-iii):** the book path render (default template, standalone, integer + decimal index, collapsing groups, sanitization, and the music regression set); the mover book round-trip (multi-file + the single-M4B many-chapters case, undo, crash replay); and the end-to-end import of the multi-file fixture (rows, moved files, conflict refusal). Full workspace suite + clippy `-D warnings` + fmt + the `--no-default-features` music-only build green.

*Usable artifact:* point the CLI at a folder or an `.m4b` and get an organized, database-owned audiobook with ordered chapters, headless. **Phase 7a complete.**

### Phase 7b — Audiobooks tab (browse)

Split like Phase 6b: **7b-i** the read-only browse surface (the cover shelf + detail pane + state derivation), **7b-ii** the filter bar + bucket sidebar, **7b-iii** bulk edit.

- [x] **The Audiobooks view (7b-i, v0.0.56):** the third `AdwViewStack` page added to the Phase 6b-i shell (the switcher now offers all three, Alt+3), built lazily on `::map`. A cover-grid **shelf** (`gtk::GridView`, the app's first; accent-tinted tiles, the Hermitage unit, the first `accent_rgb` use in the GUI) beside a book **detail pane** (cover, title, author/narrator · series/sequence · year, a progress bar + state, and the chapter list), a side-by-side `gtk::Paned`. The multi-view chrome split out of `attach_podcasts_view` into a shared `install_view_chrome` so podcasts and audiobooks are independent (either alone still gets the switcher). Per-book speed / sleep-timer controls are playback, so they land at 7c. Cozy's layout over Conservatory's database. New core read `list_book_rows` (denormalized author/narrator/series + progress, the `EpisodeListRow` precedent) + the `audiobook list` CLI artifact.
- [x] **State derivation (7b-i):** New / In progress / Finished from `book_playback` (`BookState::derive`), pure and tested; `sort_shelf` surfaces in-progress books first (most recently played first).
- [x] **Filter bar (7b-ii, v0.0.57):** the Audiobooks shelf gains an always-on `gtk::SearchEntry` wired to `conservatory-search`; same grammar, no separate search mode, `Ctrl+F` focuses it (Managed scope, so it does not collide with the window's global music `Ctrl+F`). The audiobook fields joined the **shared** `Field`/`State`/`SearchItem` (`author:`/`narrator:`/`series:`/`is:finished`), so they are known on every surface (a book field in the music bar matches nothing); they are **eval-only** (`sql_translate` returns `None` for them, forcing the in-memory path). `is:finished` follows the `is:played`/`is:starred` shape; negate with `NOT is:finished` (the spec's `is:finished false` example predates the `is:` mechanics). The shelf is loaded whole and re-filtered in memory per keystroke (no debounce; tens of rows). New GUI `book_query::filter_books` (headless, unit-tested) + the `audiobook list <db> [expr]` CLI filter (the headless artifact). A degraded expression tints the bar (`filter-warn`).
- [x] **Bulk edit, headless half (7b-iii-a, v0.0.58):** the typed `BookEdit` resolver (`conservatory-audiobooks/edit.rs`, pure: path-affecting = author/series/series_index/title/year; narrator/shelf_genre/rating/starred are not), the core worker writes (broadened `update_book`, new `set_book_series` (clears to standalone), `set_book_authors`/`set_book_narrators` clear+relink), and the **book reorganize** mover path (`reorg.rs`: `apply_book_edit` writes metadata, `plan_book_reorg`/`apply_book_reorg` re-render the folder and move the files via the Phase 2c journaled mover under `MoveKind::Organize` — no mover change needed, a `book_id` op rewrites chapters + folder the same as import). CLI `audiobook set` gained `--title/--year/--author/--narrator/--series/--series-index` + `--root`/`--apply` (dry-run previews the move; `--series ""` clears to standalone). Cover follows the move (best-effort). Tests: resolver units, the reorg round-trip (multi-file + single-M4B + undo + unchanged no-op + conflict refusal). Verified end-to-end against the import fixture.
- [x] **Bulk edit, GTK half (7b-iii-b, v0.0.59):** the shelf became `MultiSelection` (a plain click still selects one, so the detail browse is unchanged; the detail pane follows the first selected book). A pencil button on the filter bar + `Ctrl+E` (Managed scope, no collision with the window's global music shortcuts) open an `adw::AlertDialog` bulk-edit grid (Author(s)/Narrator(s) `;`-separated, Series, Series index, Title, Year, Shelf genre, Rating, plus a **Standalone** checkbox = the explicit series clear). Blank = unchanged; a bad value rejects the whole set. Apply writes metadata via `apply_book_edit`, then a path-affecting edit aggregates `plan_book_reorg` across the books into a single **"Move N files?"** confirm → `apply_book_reorg` per book → shelf reload. The pure edit/move logic is the 7b-iii-a tests; the dialog is build + manual. **Completes Phase 7b.**
- [ ] Tests: shelf/filter model logic; book-state derivation; Perspective save/reload over books.

*Usable artifact (7b-i):* browse the audiobook library as a cover shelf, in-progress first, and open a book for its metadata, progress, and chapters; filter / sort / bulk-edit follow in 7b-ii/iii.

### Phase 7c — Audiobook playback (chapters + first-class resume)

Split into three commits, mirroring the 6b-ii-c episode rollout: the headless segment engine, then resume + per-book profile + chapter navigation, then the GTK / MPRIS surface.

- [x] **Book plays as one item, the segment engine (7c-i, v0.0.60):** a book is one `PlayableItem` (kind `Audiobook`). The new pure `player/book.rs` (`plan_book` / `BookSegment` / `BookPlan` / `locate` / `build_book_item`) maps a book's chapters into per-file **segments** with cumulative book-absolute starts and lifts the chapter marks to absolute book time. The engine advances file to file *internally* on each file's EOF (no queue advance, the session stays open) and completes the book (`book_playback.finished`) only at the last file's EOF (spec §6.1); the snapshot reports book-absolute position / duration. CLI `audiobook play <db> <id> --root`. Decision: engine-driven file advance (an M4B is gapless and chapters are internal seeks; a multi-file book gaps briefly at file boundaries, true cross-file gapless a later refinement). Tests: multi-file advance through every file, single-M4B completion, the segment math units.
- [ ] **Resume + per-book profile + chapter navigation (7c-ii):** migration 0013 (`playback_state.book_id` cursor; `listening_sessions.book_id` so book Smart-Speed time-saved feeds `stats`). `resolve_book_profile` reuses the spoken-word `af`-chain (variable speed, Smart Speed, Voice Boost) with per-book overrides from `book_playback` (spec §6.3); no new filter graph. First-class resume: absolute `book_playback.position`, written on the insurance interval / pause / seek (spec §6.4). Chapter navigation reuses the 6c skip mechanism, mapping the absolute seek to the right segment + in-file offset across the file/M4B-span boundary. Tests: resume-to-the-second across a restart, chapter-skip across a file boundary, per-book override resolution, a book listening session.
- [ ] **GTK, MPRIS, per-book settings (7c-iii):** play a book from the shelf (double-click / Ctrl+Enter append) into the unified queue; the Now-bar + Now Playing book surface (metadata, chapter list/jump, sleep timer, speed); MPRIS metadata for the current book/chapter (spec §6.5); a per-book playback-settings gear (speed / Smart Speed / Voice Boost → `book_playback`).

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

---

## Phase 9 — Listening history sync (optional, off by default)

A peripheral "feel good" addition for the music-and-podcast lifer: scrobble completed plays to an external listening-history service. **Optional and off by default**, it sits late and self-contained so it never blocks the engine work, and it is the deliberate, scoped reversal of the spec §14 no-social/no-cloud stance (recorded there). Local-first is preserved: with sync off the app is unchanged, and **ListenBrainz leads** (open, self-hostable, fits the offline-first rule) with Last.fm as a secondary optional target. Reuses `reqwest` (already in the workspace via `conservatory-podcasts`); no new Rust dependency. Hooks the existing play-completion path (spec §6.4) that already updates play counts.

- [ ] A `scrobble` module (in core, behind config): on track/episode completion, queue a "listen" submission; a small on-disk outbox survives offline and retries (local-first: a play is recorded locally first, synced when the network returns, never lost if the service is down).
- [ ] ListenBrainz client (user token in libsecret via the existing `oo7`); "now playing" update on start + "listen" submission on completion. Last.fm client as an optional second target (session auth).
- [ ] Config `[scrobble]`: `enabled = false`, `service = "listenbrainz"`, plus the token reference; a Preferences "Sync" group to enable it and paste a token.
- [ ] Honours scope: music tracks and podcast episodes only by default; audiobooks excluded unless explicitly opted in (a 14-hour book is not a "listen").
- [ ] Tests: outbox persists and retries across a simulated offline window; completion hook enqueues exactly once; disabled is a true no-op; credential store round-trip (in-memory backend).

*Usable artifact:* completed plays scrobble to ListenBrainz (or Last.fm), surviving offline, with the feature entirely inert when disabled.

---

## Phase 10 — Configuration & preferences

> **Stub.** This phase is referenced throughout the earlier phases (the library root sourced from config rather than a CLI arg; user-reconfigurable + persisted facet-pane order; the consolidated "Sound" page the EQ/DSP work at 5.5b/c builds toward) but is not yet broken into sub-phases. It is recorded here so those forward references resolve; the detailed checklist lands when the phase is scoped. Known contents so far:

- [ ] A persisted config (spec §10): the library root, the `[playback]` defaults that today flow from `PlaybackConfig::default()`, and the per-pane field expressions + order (the §3.2 "panes are configurable 1 to 5" promise, deferred from 3b/3c).
- [ ] An `AdwPreferencesDialog` consolidating the Phase 5.5 "Sound" page (ReplayGain / EQ / DSP / output) with a General page (library root, import defaults) and a Library page (pane configuration).
- [ ] The library root stops being a CLI arg (the carry-forward note from 4b-ii-a / 4b-ii-c).

*Usable artifact:* (to be detailed) the app is configured from a Preferences dialog, not CLI args, and remembers the user's browse layout.

---

## Phase 11 — Browse & player polish (Columns UI parity)

The finishing pass that brings the music surface up to the deadbeef / foobar2000 Columns UI the browse is modeled on (spec §3.2, §3.3): the side panels, the chrome, and the player conveniences a daily driver is expected to have. Each piece is small and self-contained, GTK-side over logic that already exists in core, so they ship independently and none blocks the others. Modeled on the reference deadbeef layout (the cover-art + properties + status-bar furniture around the central facet/track view).

### Phase 11a — Track properties inspector + cover-art panel

- [ ] A **properties / metadata inspector** for the selected track (the deadbeef `selproperties` widget): location, codec / format, sample rate, channels, bitrate, file size, duration, ReplayGain values, embedded-vs-sidecar cover, MusicBrainz id. Read-only; all of it is already in the DB (`tracks`) or cheap to stat. A collapsible side panel, not a modal.
- [ ] A **cover-art panel** in the browse window (the deadbeef `coverart` widget, "playing or selected" mode): the album art at a readable size, accent-tinted (the Hermitage unit), distinct from the small Now-bar thumbnail. Reuses `albums.cover_path` (Phase 5d) and the accent.
- [ ] Tests: the inspector field projection (a pure map from a `Track` + album row to the displayed fields); the panel is build + manual (the 3b/3c precedent).

*Usable artifact:* select a track and see its full technical metadata and a large cover, as in the deadbeef layout.

### Phase 11b — Status bar + play-status glyph column

- [ ] A **status bar** (spec §3.2 footer): the current track's format / sample-rate / channels, plus the active view's track count and total playtime (the deadbeef "N tracks, D total playtime" line). The aggregate is a cheap core read over the current facet/filter selection.
- [ ] The **play-status glyph column** (the leftmost ♫ in the deadbeef track list): a per-row playing / paused indicator. This is the item **explicitly owed from Phase 3c** ("the per-row playing/status glyph waits for playback state, Phase 4"); Phase 4 shipped the playback state, so it is now unblocked. Driven by the engine snapshot's current item (a symbolic icon, no font assumption).
- [ ] Tests: the aggregate-count / total-playtime read against a fixture; the glyph follows the snapshot's current index (headless logic); widgets build + manual.
- [x] **Pulled forward at v0.0.38 (playback feedback):** the snapshot gained `kind` / `streaming` / `buffering`, so the Now-bar shows a **buffering spinner** (mpv `core-idle`) and a **streaming glyph** for an undownloaded episode, and the Podcasts episode list gained a **downloaded vs stream-only** glyph column. (The full status-bar line and the in-list play-status glyph above are still to do.)

*Usable artifact:* the browse window shows the playing row at a glance and a foobar-style status line.

### Phase 11c — Now Playing surface (bottom drawer)

- [x] **A bottom Now Playing drawer landed at v0.0.38** (the lighter realization of the spec §3.6 surface, chosen over a full-bleed takeover): a slide-up `gtk::Revealer` above the Now-bar, the horizontal twin of the right-docked queue drawer, opened by clicking the Now-bar cover/title or `Ctrl+I`. It shows the current item's full metadata (track: format / bitrate / sample rate / ReplayGain / path / rating / plays / album / year; episode: show / date / runtime / size / source stream-or-local / notes), refreshed as the queue advances. The pure field projection is unit-tested. **The drawer's content area is the intended home for the spectrum visualizer** (the deferred item below).
- [ ] Still to do (the richer surface): a full-bleed cover, an accent-tinted scrubber, and a queue-tail peek; track ReplayGain/EQ/DSP/gapless state and episode chapters / Smart Speed indicator / sleep timer (the episode additions overlap Phase 6c, so this consumes what 6c builds).
- [ ] Tests: the surface state projection from the snapshot + the current item's metadata (headless); widget build + manual.

*Usable artifact:* clicking the Now-bar (or `Ctrl+I`) slides up a Now Playing drawer with the current item's metadata, across both media types.

### Phase 11d — Transport conveniences

- [ ] **Stop-after-current** (the deadbeef `toggle_stop_after_current`, the user's `Ctrl+M`): the engine finishes the current item, then stops instead of advancing. A small flag on the engine consulted in `advance_after_end`.
- [ ] **Jump-to-current-track** (the user's `Ctrl+J`): scroll the browse / queue to the playing row and select it.
- [ ] Both as menu actions and keybindings (`docs/keymap.md`), the spec §3.1 "every action visible and keyboard-accessible" rule.
- [ ] Tests: the stop-after-current flag gates the advance (engine null-host integration); the jump resolves the current row index (headless).

*Usable artifact:* **Columns UI parity.** The music surface matches the deadbeef daily-driver layout: side panels, status bar, an expanded Now Playing view, and the expected transport conveniences.

### Deferred (recorded, not built)

- [ ] **Spectrum visualizer** (the deadbeef `spectrum` widget): a real-time frequency-bar analyzer. Captured here at the user's request, but **post-1.0 and optional**: it needs an audio-tap off the libmpv output (an `af` data sink or a visualizer hook) and is player-toy territory rather than core to "Calibre for audio". Built only after the parity furniture above, if at all. **Its home is the v0.0.38 Now Playing drawer (11c)** (the user's intent), so it slots in there beside the metadata when built.
