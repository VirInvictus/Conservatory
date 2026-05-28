# Conservatory — Application Specification

**Version:** 0.0.1 (in development; Phase 0 design is complete and the Phase 1 bootstrap has begun. The build is no longer deferred, see §17)
**Target:** GNOME 50+, GTK4 ≥ 4.16, libadwaita ≥ 1.7
**Language:** Rust (2024 Edition)
**Build System:** Cargo workspace (`conservatory-core` + `conservatory-search` + `conservatory-cli` + `conservatory`) / Meson wrapper for Flatpak packaging
**License:** GNU GPL v3.0 or later (forced by librubberband in the absorbed Smart Speed chain, the same license chain as Belfry; see §15)

> **Status note.** This is the design contract. The decisions below are settled enough to build against. As of v0.0.1 the workspace skeleton exists and Phase 1 (§17) is underway; the build is no longer deferred (the original deferral and its rationale are preserved in §16.1 and §17 for the record, since they are the thing to re-read if the concurrency with Atrium proves a mistake). Provisional detail (exact schema columns, CLI verbs, config keys) follows the established portfolio patterns from Atrium and Belfry and will firm up at implementation time. Genuinely open decisions are collected in §16, not scattered as silent guesses.

---

## 1. Mission Statement

Conservatory is **Calibre for audio**: a native GNOME library manager that *owns and organizes* your music and podcasts on disk, presented through a foobar2000 Columns UI browse surface, played through a libmpv daily-driver engine that runs both media types from a single queue.

Four commitments, in priority order:

1. **The database owns the library.** The SQLite database is the source of truth for organization and curated metadata; the application owns the on-disk layout and moves files to match it. This deliberately *inverts* the filesystem-canonical stance of Lattice and Belfry. There, the filesystem is the contract and the database is a regenerable index. Here, you hand Conservatory your files and it shelves them (`Genre / Album Artist / Album /` by default, §5.1), the way Calibre takes a book and files it under its author tree. That is a trust commitment, and §5.4 spends it carefully (dry-run, undo, embedded-tag write-back so files stay portable).

2. **Calibre-shaped, Columns UI browse.** Every list view is a queryable database. The default music surface is a faceted, hierarchical Columns UI browser (the design proven in `deadbeef-cui`, freed from being a player plugin), backed by the full Calibre-style search expression grammar (§3.4). Sortable columns, multi-select bulk actions, saved Perspectives. The browse panes filter; the grammar searches; they are the same surface.

3. **One engine, one queue, two media types.** Music tracks and podcast episodes share a single libmpv engine and a single play queue. Each queued item carries its own playback profile: gapless and ReplayGain for an album track, Smart Speed and Voice Boost for an episode. A first-class, mixed music-and-podcast listening queue is the standout feature and the reason Belfry is absorbed rather than kept separate (§6.1).

4. **A daily-driver player, not a previewer.** For libraries Conservatory manages, it is the place you listen, replacing deadbeef. Gapless, ReplayGain, crossfade, output-device selection, MPRIS, media keys (§6). This is required, not optional: because Conservatory moves files, any external player's in-place references go stale the moment a library is re-shelved.

Reference apps:

- **Calibre** (Kovid Goyal et al.) — the library-as-database model, file ownership, and the "save to disk" path template that makes the on-disk tree a render of the database rather than a lock-in.
- **foobar2000 / Columns UI / Facets**, via **`deadbeef-cui`** (Brandon's own) — the faceted, multi-pane, metadata-first browse layout for large collections.
- **beets** (Adrian Sampson et al.) — the `lastgenre` canonicalization model (curated genre whitelist plus tree) that informs the shelf-genre normalization in §5.2.
- **Overcast** and **Castro**, via **Belfry** (Brandon's own) — the absorbed podcast engine (Smart Speed, Voice Boost) and the Inbox → Queue → Played triage model.
- **Atrium** and **Viaduct** (Brandon's own) — the single-writer SQLite worker pattern and the search-expression grammar shape.
- **Hermitage** (Brandon's own) — cover art as the visual unit; per-album accent extracted from cover hue (median-cut quantizer).

**Absorbs Belfry.** Conservatory is the convergence of Brandon's music tooling (`Lattice`, `deadbeef-cui`) and his podcast project (`Belfry`) into one media app. Belfry's Phase 1 work is not discarded: `belfry-core`'s single-writer worker is the exact pattern this app needs and migrates here, and Belfry's audio engine and triage model become the Podcasts side. The one casualty is Belfry's filesystem-canonical design; in Conservatory, podcasts become app-managed downloads (§5.3), which is acceptable for ephemeral episodes in a way it would not be for a curated music collection. Belfry is **not retired until Conservatory reaches podcast parity** (§17).

Non-goals are enumerated in §14.

---

## 2. Architecture

### 2.1 Single-Writer SQLite Worker

A dedicated tokio task owns the single writable `rusqlite::Connection`. The GTK thread holds an `mpsc::Sender<Command>` and never touches the writable connection directly. Reads use a separate read-only connection pool the worker does not own. WAL mode is mandatory. This is the pattern shipped in Viaduct, Atrium, and Belfry Phase 1.

It matters acutely here because Conservatory has several independent producers writing concurrently:

- the **import / organizer** (new tracks, file moves, embedded-tag write-back),
- the **playback loop** (position persistence, play counts, listening sessions),
- the **podcast fetch loop** (new episodes, conditional-GET state, downloads),
- the **user** (metadata edits, shelf-genre changes, queue edits, triage).

```text
┌──────────────────────────────────────────────┐
│            Conservatory Engine (Rust)         │
│           (tokio multi-thread runtime)        │
├──────────────────────────────────────────────┤
│  [Import / Organizer]                         │
│   ├─ Tag reader/writer (lofty / symphonia)    │
│   ├─ Path-template engine (§5.1)              │
│   ├─ Shelf-genre resolver (§5.2)              │
│   └─ File mover (dry-run + undo journal)      │
├──────────────────────────────────────────────┤
│  [Playback Engine]                            │
│   ├─ libmpv host (libmpv2 crate)              │
│   ├─ Unified queue (PlayableItem, §6.1)       │
│   ├─ Music profile: gapless / ReplayGain      │
│   └─ Podcast profile: Smart Speed/Voice Boost │
├──────────────────────────────────────────────┤
│  [Podcast Fetch] (ported from belfry-core)    │
│   ├─ Per-show scheduler, conditional GET      │
│   └─ feed-rs + podcast: namespace handler     │
├──────────────────────────────────────────────┤
│  [Data Layer]                                 │
│   ├─ Writer task (rusqlite, WAL)              │
│   ├─ Read-only pool                           │
│   └─ FTS5 (tracks, albums, shows, episodes)   │
└──────────┬───────────────────────────────────┘
           │ (tokio mpsc + glib channel)
    ┌──────┴────────────────────┐
    │  GTK4 Main UI Thread      │
    └───────────────────────────┘
```

`LibraryChanges`, `PlaybackChanges`, and `JobProgress` are coalescing batch types delivered through a `glib::MainContext` channel. UI updates apply as deltas, never full reloads (the deadbeef-cui rebuild discipline: never scroll the facets back to the top on an unrelated event).

### 2.2 Crate Layout

Four crates, matching the Belfry / Atrium discipline that every non-GUI surface stays CLI-testable:

- **`conservatory-core`** — headless data layer. SQLite worker plus read pool; tag read/write; the import pipeline, path-template engine, shelf-genre resolver, and file mover; the libmpv host plus playback profiles; the unified queue model; the podcast fetch/parse pipeline (ported from `belfry-core`); OPML import/export; cover-art decode plus accent extraction. GUI-free.
- **`conservatory-search`** — the Calibre-shaped search expression language (lex / parse / AST / evaluator / SQL translator), typed against Conservatory's domain (Track / Album / Artist / Show / Episode). The grammar *shape* is ported from `atrium-search`; the implementation is independent so the projects evolve without coupling (the Belfry precedent; record it in `ATTRIBUTIONS.md`).
- **`conservatory-cli`** — headless binary: import, organize, search, tag, queue, podcast ops, stats. See §9.
- **`conservatory`** — the GTK4 binary. Depends on the three above.

> **Library-graduation watch.** The path-template engine plus file mover (§5) is domain-light and could justify a standalone crate later, and the playback engine is now eyed by two efforts (this and Belfry's lineage). Neither is at graduation point yet; flag again if either grows a stable, reusable public surface.

### 2.3 Widget Tree

A top-level view switcher selects between **Music** and **Podcasts**, with a persistent Now-bar across both.

```text
AdwApplicationWindow
├── AdwBreakpoint (narrow → split views collapse)
└── AdwToastOverlay
    └── AdwToolbarView
        ├── AdwHeaderBar (AdwViewSwitcher: Music | Podcasts; search; jobs; menu)
        └── AdwViewStack
            ├── "Music"
            │   └── faceted Columns UI panes (1–5, §3.3) over a track list
            └── "Podcasts"
                └── AdwNavigationSplitView (sidebar triage + episode list + detail)
        └── AdwBin (now_bar — persistent transport, the unified queue's head)
```

The Music view is the deadbeef-cui layout as a first-class window: N configurable hierarchical filter panes (default Genre → Album Artist → Album) feeding a sortable track list. The Podcasts view is Belfry's three-pane triage layout.

---

## 3. User Interface

### 3.1 Design Principles

- **Whitespace and colour with discipline.** libadwaita's restrained accent system; per-album accent extracted from cover art (the Hermitage median-cut pattern) used as an accent, not a brand colour. Desktop reading distances; AdwClamp-bounded list widths on ultrawide displays.
- **Every list is a queryable database.** The Calibre gift: filter bar with the full grammar (§3.4), sortable columns, multi-select bulk actions, saved Perspectives, on both the music and podcast surfaces.
- **Metadata-first browse for large collections.** The Columns UI facets (§3.3) are the primary music navigation, tuned for 50k-plus-track libraries the way deadbeef-cui is.
- **Every action visible and keyboard-accessible.** Framework's discipline: no hidden gestures; every swipe has a menu equivalent; keyboard-first works.

### 3.2 Layout (Music)

```text
┌─────────────────────────────────────────────────────────────┐
│ [≡]  Conservatory      ( Music | Podcasts )      [search][⚙] │
├───────────────┬───────────────┬───────────────┬─────────────┤
│ Genre         │ Album Artist  │ Album         │ Tracks      │
│ [All (842)]   │ [All (1203)]  │ [All (60)]    │ # Title  ★  │
│ Electronic    │ Boards of Can.│ Geogaddi      │ 1 ...    ●  │
│ Ambient       │ Brian Eno     │ Music Has ... │ 2 ...       │
│ Jazz          │ Aphex Twin    │ ...           │ ...         │
│ ...           │ ...           │               │             │
├───────────────┴───────────────┴───────────────┴─────────────┤
│ [▶] cover  Track — Artist     ◀◀  ▶  ▶▶   ReplayGain  3:41   │
└─────────────────────────────────────────────────────────────┘
```

Panes are configurable (1–5), each driven by a title-formatting-style field expression, exactly as deadbeef-cui configures its columns. Selecting in a pane filters the panes to its right and the track list. Multi-select aggregates (Ctrl/Shift-click). An `[All (N …)]` synthetic row tops each pane. Selection-change is debounced before downstream recompute (the deadbeef-cui invariant that keeps multi-select drags cheap on large libraries).

### 3.3 Faceted Browse (Columns UI)

The music browse model, lifted from `deadbeef-cui` and rebuilt over Conservatory's database instead of DeaDBeeF's medialib:

- Hierarchical panes; default Genre → Album Artist → Album → Tracks; user-reconfigurable order and field expressions, persisted.
- Multi-value tags split for faceting (a track tagged `Electronic; Ambient` appears under both Electronic and Ambient in the Genre pane) while the *shelf* genre that determines its file location stays single-valued (§5.2). Facets read raw tags; the filesystem reads the shelf genre. These are deliberately decoupled.
- Track counts per facet row, memoized.
- The track list is the leaf: sortable columns, multi-select, the same row affordances as Belfry's episode list (status glyph, rating, hover lift).

### 3.4 Filtering, Search, and Perspectives

One grammar, both surfaces, the Atrium/Belfry shape. The filter bar above any list accepts the full expression language; `Ctrl+F` focuses it; there is no separate search mode.

| Field | Example |
|---|---|
| `artist:`, `albumartist:`, `album:`, `title:` | `albumartist:"Boards of Canada"` |
| `genre:`, `shelfgenre:` | `genre:ambient` (raw tag) vs `shelfgenre:Electronic` (filed-under) |
| `year:`, `added:` | `year:1998..2004`, `added:thisweek` |
| `rating:`, `bitrate:`, `duration:`, `format:` | `rating:>=4 AND format:flac` |
| `is:played`, `is:starred`, `is:queued` | `is:starred AND genre:jazz` |
| podcast fields (`show:`, `is:in_inbox`, `pub:` …) | as Belfry §3.7 |

Boolean (`AND`/`OR`/`NOT`), match modifiers (substring / `=`exact / `~`regex / `?`fuzzy), comparison and range, date keywords, sort modifiers (`sort:KEY`, `sort:-KEY`). Forgiving parser: malformed input degrades to substring match with a yellow filter-bar tint, never an error. **Perspectives** are named saved expressions (Calibre saved searches, Atrium's term), stored as text and re-parsed on load so they inherit later grammar additions. A Perspective can target tracks, albums, or episodes and can be a queue source (§6.1).

### 3.5 Bulk Metadata Editing

Calibre's editor surface, music-shaped. Multi-select in any list, then edit fields across the selection: artist, album artist, album, year, genre (raw tags and shelf genre), rating, cover. Search-and-replace across a field. A change that alters the shelf genre or the album/artist path triggers a file move (§5.4), surfaced as a job with a dry-run preview and undo. Embedded-tag write-back (§5.5) is part of the same job.

### 3.6 Now Playing and the Queue

The unified queue (§6.1) is the spine. The Now-bar persists across Music and Podcasts; tapping it expands to a Now Playing surface (the Hermitage Codex moment: full-bleed cover, accent-tinted scrubber, queue tail peek). For episodes the surface adds chapters, show notes, Smart Speed indicator, and sleep timer (Belfry §3.6). For tracks it shows album context, ReplayGain state, and gapless/crossfade status. The queue view itself is a single list that interleaves tracks and episodes, drag-reorderable, each row badged with its kind.

### 3.7 Podcasts Tab

Belfry's Inbox → Queue → Played triage, intact (Belfry §3.3–3.4): a sidebar of triage lists, shows, and tags; an episode list; a detail/now-playing pane. Per-show overrides for speed, Smart Speed, Voice Boost, skip, retention, and inbox policy. The only structural change from Belfry is that the **Queue is the shared unified queue**, so an episode and an album track can sit next to each other in it.

---

## 4. Data Model

WAL mode, foreign keys on, `synchronous=NORMAL`. Single writer; read commands open read-only at the process level. FTS5 on titles. Migrations versioned via `user_version`, append-only and backwards-compatible post-1.0 (the Atrium discipline).

### 4.1 Music (draft schema)

```sql
CREATE TABLE artists (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    sort_name       TEXT NOT NULL,      -- "Beatles, The"; drives path + sort (Calibre author_sort)
    musicbrainz_id  TEXT,
    UNIQUE (sort_name)
);

CREATE TABLE albums (
    id                  INTEGER PRIMARY KEY,
    title               TEXT NOT NULL,
    album_artist_id     INTEGER REFERENCES artists(id),  -- NULL => Various Artists bucket
    shelf_genre         TEXT,           -- THE ONLY input to the genre folder level; editable (§5.2)
    year                INTEGER,
    release_date        TEXT,
    musicbrainz_release_id TEXT,
    cover_path          TEXT,
    accent_rgb          INTEGER,        -- packed RGB, median-cut from cover
    folder_path         TEXT NOT NULL,  -- managed; rendered from the template (§5.1)
    added_at            INTEGER
);

CREATE TABLE tracks (
    id              INTEGER PRIMARY KEY,
    album_id        INTEGER REFERENCES albums(id) ON DELETE CASCADE,
    artist_id       INTEGER REFERENCES artists(id),  -- track artist (may differ from album artist)
    title           TEXT NOT NULL,
    track_no        INTEGER,
    disc_no         INTEGER,
    duration        REAL,               -- seconds
    file_path       TEXT NOT NULL,      -- managed; under the album folder
    format          TEXT,               -- flac/mp3/opus/aac/...
    bitrate         INTEGER,
    sample_rate     INTEGER,
    replaygain_track REAL,
    replaygain_album REAL,
    rating          INTEGER DEFAULT 0,  -- 0–5; foobar/Lattice loanword
    play_count      INTEGER DEFAULT 0,
    last_played     INTEGER,
    starred         INTEGER DEFAULT 0,
    musicbrainz_recording_id TEXT,
    added_at        INTEGER
);

-- Raw multi-value genres, preserved untouched for facets + search. NOT the shelving input.
CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL);
CREATE TABLE track_genres (
    track_id INTEGER REFERENCES tracks(id) ON DELETE CASCADE,
    genre_id INTEGER REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (track_id, genre_id)
);

-- Genre normalization (§5.2). Source of the seed map is OPEN (§16).
CREATE TABLE genre_aliases   (raw TEXT PRIMARY KEY, canonical TEXT NOT NULL);
-- User priority list, tie-breaks shelf-genre derivation when track genres disagree.
CREATE TABLE genre_priority  (genre TEXT PRIMARY KEY, rank INTEGER NOT NULL);
```

### 4.2 Podcasts

Ported from Belfry §4.1 (`shows`, `episodes`, `playback`, `show_settings`, `listening_sessions`, `chapters`, `tags`, `show_tags`), with one change: triage Queue state is represented through the unified `queue` table (§4.3) rather than a per-episode `in_queue` flag. The rest is unchanged, including the append-only `listening_sessions` discipline.

### 4.3 Unified Queue and Playback Bridge

```sql
-- One ordered queue across both media types. The bridge that makes the unified queue real.
CREATE TABLE queue (
    id          INTEGER PRIMARY KEY,
    position    INTEGER NOT NULL,       -- explicit, drag-reorderable
    kind        TEXT NOT NULL CHECK (kind IN ('track','episode')),
    track_id    INTEGER REFERENCES tracks(id)   ON DELETE CASCADE,
    episode_id  INTEGER REFERENCES episodes(id) ON DELETE CASCADE,
    CHECK ( (kind='track'   AND track_id   IS NOT NULL AND episode_id IS NULL)
         OR (kind='episode' AND episode_id IS NOT NULL AND track_id   IS NULL) )
);
CREATE INDEX idx_queue_position ON queue(position);
```

The engine reads `queue` into an in-memory `Vec<PlayableItem>` (§6.1); position writes are debounced. Resume position for long items (mixes, episodes) lives in the per-kind state tables (`tracks.last_played` / Belfry's `playback`).

### 4.4 FTS5

`track_fts` (title, artist, album), `album_fts` (title, album artist), plus Belfry's `episode_fts` and `show_fts`. Triggers keep them in sync. Not transcripts (§14).

---

## 5. Library and Filesystem Layout (the file-ownership model)

This is the section that distinguishes Conservatory from everything else Brandon has built. The application owns the on-disk layout and moves files to match the database.

### 5.1 Music On-Disk: a rendered template

The database is truth; the on-disk tree is a *render* of a configurable path template, exactly as Calibre's "save to disk" template and beets' `paths:` config work. The default:

```text
<library root>/
└── <Shelf Genre>/
    └── <Album Artist sort_name>/
        └── <Album> (<Year>)/
            ├── NN - <Title>.<ext>
            └── cover.jpg
```

Default template string: `{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}`. Because the layout is a render of the database, re-shelving an album is a *template-or-field change*, not a lock-in. The template is user-editable; `{shelf_genre}` is the only piece that depends on the genre decision in §5.2.

An **album is the unit that moves.** A single album resolves to exactly one path: one shelf genre and one album artist drive the directory, even when track-level genres or artists disagree. Compilations resolve their album-artist component to a **Various Artists** bucket.

### 5.2 Shelf-Genre Resolution

The riskiest design call, because genre is the least canonical, most multi-valued tag. The resolution keeps the raw tags off the filesystem entirely.

- Each album has an editable **`shelf_genre`** field (the Calibre `author_sort` trick: the shelving key is a separate field, not the raw tag). It is the only input to the genre folder level. The decision is **flat**: the normalized `shelf_genre` value is used verbatim as the top folder. (A genre *tree* with rollup, where leaf tags like Synthwave collapse to a coarse parent like Electronic, was considered and deferred to a possible v2, §16.)
- `shelf_genre` is auto-filled on import by a priority chain:
  1. a manual override, if the user has set one;
  2. else a single album-level genre tag, if present;
  3. else the most common normalized genre across the album's tracks, ties broken by the user's `genre_priority` list, then first;
  4. else an `Unknown` bucket.
- A **normalization layer** runs before the chain: split on `;` `/` `,`, case-fold, then map through `genre_aliases` (for example `IDM` → `Electronic`, `Hip Hop`/`Rap` → one canonical). The seed source for that alias map is open (§16).
- `shelf_genre` is always overridable per album and bulk-editable. Editing it moves the album's directory (§5.4). Raw `track_genres` are never touched by any of this; they exist for facets and search.

### 5.3 Podcasts On-Disk: managed downloads

Podcasts adopt the managed model: `<library root>/Podcasts/<show-slug>/<YYYY-MM-DD>--<episode-slug>/`. Belfry's filesystem-canonical guarantee ("the library survives the database") is **dropped for podcasts**, accepted because downloaded episodes are ephemeral, not curated. Streaming still works before/without download (Belfry §6.2): if the local file is absent and a URL is present, libmpv streams with range requests.

### 5.4 Import, Organize, and Move Pipeline

1. **Scan or drop** files or folders.
2. **Read tags** (lofty / symphonia, §7.1).
3. **Resolve** artists/albums/genres into the database; derive `shelf_genre` (§5.2).
4. **Render** the target path from the template (§5.1).
5. **Move or copy** into the managed tree. In-place vs copy-on-import is a per-import user choice (copy leaves the originals untouched; move consumes them).

Every operation that relocates files is a **job** with: a **dry-run preview** of exactly which files move where, an **undo journal** so the move is reversible, conflict handling (duplicate target paths, read-only sources, cross-filesystem moves), and crash safety (the journal is written before the move, replayed on restart). Moving a user's real files is a trust commitment; the dry-run and undo are not optional.

### 5.5 Embedded-Tag Write-Back and Portability

Although the database owns organization, Conservatory **writes curated metadata back into the files' embedded tags** (a Calibre "embed metadata" analogue), so the files remain portable and self-describing outside the app. Write-back is a job, batched, and respects format capabilities (Vorbis comments, ID3, MP4 atoms). This is what keeps the file-ownership model from being a roach motel: you can always walk away with tagged files.

### 5.6 Re-Import Contract

Conservatory is not filesystem-canonical, so the Belfry rescan contract does not apply unchanged. The contract here is weaker and explicit: **the managed tree plus embedded tags can rebuild a library's tracks, albums, and artists**; the database-exclusive data that a re-import cannot recover is the *curated* layer (shelf-genre overrides, ratings, play counts, starred, Perspectives, queue, podcast triage/listening state). That curated layer is what the nightly DB backup and the JSON export protect. The integration suite verifies the rebuildable subset against a fixture library.

---

## 6. Playback Engine

A single libmpv instance kept alive across items (the `libmpv2` binding, property API plus filter graph).

### 6.1 PlayableItem and the Unified Queue

The engine plays a queue of:

```rust
struct PlayableItem {
    path_or_url: Source,     // local managed file, or stream URL for an undownloaded episode
    kind: MediaKind,         // Track | Episode
    profile: PlaybackProfile // resolved per-kind + per-show/per-album overrides
}
```

The queue (§4.3) interleaves both kinds freely. On advance, the engine applies the item's profile (the right `af` filter chain, ReplayGain mode, gapless/crossfade behaviour) before playing. This single abstraction is what lets one queue, one Now-bar, one MPRIS surface, and one set of media keys serve both music and podcasts.

### 6.2 Music Profile

- **Gapless** playback within an album (libmpv `--gapless-audio`).
- **ReplayGain** track and album modes, read from `tracks.replaygain_*`. Whether Conservatory also *scans* ReplayGain values for untagged files, or only reads existing ones, is open (§16).
- **Crossfade** between non-gapless tracks (user-configurable duration; off by default).
- Optional **EQ / DSP**: depth is open (§16); deadbeef ships a full DSP chain, and matching it is its own project.

### 6.3 Podcast Profile

Smart Speed (silence-skip via `silenceremove` + pitch-preserving `rubberband`) and Voice Boost (compression + EQ + loudness normalization), ported verbatim from Belfry §5.1–5.3, including the time-saved session accounting. Per-show overrides as in Belfry.

### 6.4 State Persistence

Position written on pause, seek (debounced), item end, app quit, and every 30 s during playback (the Belfry insurance interval). Resume offset of a few seconds for context on long items. Music play counts and `last_played` update on completion; episode listening sessions are append-only (Belfry §5.4).

### 6.5 System Integration

MPRIS2 (`org.mpris.MediaPlayer2`) with full metadata for the current item regardless of kind; play/pause/next/previous/seek; exposure to GNOME's media overlay, lock screen, and headset buttons. PipeWire output-sink picker. Suspend inhibitor during active playback.

---

## 7. Tagging and Metadata

### 7.1 Tag Read/Write

`lofty` (broad format coverage, read + write) is the leading candidate; `symphonia` is the fallback/decoder reference. Subject to the dependency sign-off rule (§11). Reads feed import; writes feed embedded-tag write-back (§5.5).

### 7.2 Genre Normalization

The alias map and priority list of §5.2. **Open:** whether Conservatory ships a default vocabulary (beets' `lastgenre` whitelist, or the MusicBrainz genre list) or starts empty and is user-built (§16).

### 7.3 MusicBrainz

Tagging from a canonical source (matching tracks to MusicBrainz, fetching authoritative metadata and cover art) is **out of the core scope by default** and tracked as an open decision (§16). It is exactly Picard's domain and a deep matching problem; Conservatory assumes files arrive reasonably tagged unless and until this is taken on.

### 7.4 Cover Art and Accent

Cover art is the visual unit (Hermitage). On import, extract or locate cover art, store at `cover.jpg` in the album folder, and compute a dominant-hue accent (median-cut quantizer) into `albums.accent_rgb` for the browse and Now Playing surfaces.

---

## 8. Podcasts (absorbed from Belfry)

The podcast subsystem is Belfry, ported: per-show polling with conditional GET and jittered intervals; HTTP Basic auth with credentials in libsecret (`oo7`); `feed-rs` plus a hand-rolled `podcast:` namespace handler; episode identity by `(show_id, guid)`; three-source chapter precedence; OPML round-trip preserving tags and `applePodcastsID`. See Belfry `spec.md` §7 for the exhaustive contract; that detail migrates into this document as the absorption is implemented, at which point Belfry's spec is superseded.

---

## 9. CLI

`conservatory-cli` ships alongside the GUI (the Hermitage / CalibreQuarry / Belfry pattern: GUI to browse, CLI to batch).

```text
conservatory-cli import <path> [--copy|--move] [--dry-run]
conservatory-cli organize [--dry-run]          # re-render the tree from the DB
conservatory-cli search '<expression>'         # the §3.4 grammar
conservatory-cli tag set <selector> field=value...
conservatory-cli shelf-genre set <album-selector> <genre>
conservatory-cli queue add|remove|reorder|list <selector>
conservatory-cli play <selector>               # hand off to standalone mpv
conservatory-cli stats                         # library + listening stats
conservatory-cli podcast add|remove|refresh|download <spec>   # Belfry verbs
conservatory-cli import-opml|export-opml
conservatory-cli embed-tags <selector> [--dry-run]   # write DB metadata into files
conservatory-cli backup|restore                # DB snapshot
```

Read commands open the DB read-only at the process level. Write commands spin up the worker on a current-thread runtime and shut down cleanly (the Atrium/Belfry pattern). Output: `--tsv` (default), `--json`, `--human`.

---

## 10. Configuration

`~/.config/conservatory/config.toml`, optional, sane defaults.

```toml
[library]
root = "~/Music"
path_template = "{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}"
import_mode = "copy"          # "copy" | "move"
embed_tags_on_edit = true

[genre]
default_unknown = "Unknown"
# seed vocabulary source is OPEN (§16); empty until decided

[playback]
gapless = true
crossfade_seconds = 0
replaygain = "album"          # "off" | "track" | "album"
# podcast Smart Speed / Voice Boost defaults inherited from Belfry config shape

[podcasts]
library_subdir = "Podcasts"
max_concurrent_downloads = 3
```

---

## 11. Dependencies

Backend (Rust): `tokio`, `rusqlite` (bundled, FTS5), `libmpv2`, `lofty` (and/or `symphonia`), `reqwest` (conditional GET, Basic auth), `oo7` (libsecret), `feed-rs` + `quick-xml` (podcasts), `ammonia` (show-note sanitize), `id3` (chapter fallback), `image` (cover decode/accent), `serde`/`serde_json`/`toml`, `regex`, `tracing`, `zbus` (MPRIS + inhibitor). A MusicBrainz client crate only if §7.3 is taken on.

Frontend: `gtk4` (≥ 4.16), `libadwaita` (≥ 1.7), system `libmpv` (0.36+) with the ffmpeg filter library (`silenceremove`, `rubberband`, `acompressor`, `equalizer`, `loudnorm`), `libsecret` (via `oo7`).

**No third-party crate or system library lands without prior sign-off.** Brandon's standing rule; doubly noted here because this list is longer than usual and still partly provisional.

---

## 12. Flatpak Distribution

Flatpak-first. App ID `org.gnome.Conservatory` if accepted into GNOME Circle, else `io.github.virinvictus.Conservatory`. Permissions kept tight: `network` (feeds, downloads, streaming), file access to the library root (via portal where possible), `pulseaudio`/PipeWire socket (playback), `org.freedesktop.secrets` (Basic-auth credentials), `org.freedesktop.portal.FileChooser` (import, OPML). Portal-mediated background execution for periodic feed refresh is post-1.0 polish.

---

## 13. Memory and Performance Targets

Conservatory carries no WebKit. It must stay responsive on large libraries (the deadbeef-cui design target is 50k-plus tracks).

- **Idle (no playback), 50k-track library warm:** < 200 MB.
- **Playback active, Now Playing open:** < 300 MB.
- **Cold start to first interactive frame, 50k-track library:** < 500 ms.
- **Facet selection-change to track-list repaint (50k tracks):** < 100 ms (the debounce plus memoized counts make this feasible; deadbeef-cui hits it in C).
- **Position-write latency (pause → committed):** < 50 ms.

GTK4 + libadwaita pull a ~150 MB C-side floor (measured in Viaduct); the targets account for it. Each phase ends with a `heaptrack` / `massif` note; features that miss budget get gated.

---

## 14. Out of Scope, Forever

- Recommendations, "discover" tabs, charts, social features, sharing.
- A built-in music or podcast directory beyond import / OPML.
- Cloud anything that is not an optional sync protocol; the app is fully functional offline.
- DRM.
- Transcripts (Belfry's stance inherited; 2.x maybe at most).
- Video as a first-class format (audio extraction only).
- Windows / macOS; GNOME-native, deliberately.
- Becoming a from-scratch metadata authority. Even if MusicBrainz tagging (§7.3) is taken on, Conservatory consumes a canonical source; it does not try to out-Picard Picard on match quality.

---

## 15. Naming, Branding, License

**Conservatory.** A building where a collection is kept and cultivated, and a school of music; it fits the architectural-structure naming line (Atrium, Belfry, Hermitage, Framework, Lattice, Viaduct) while reading as music-coded. The icon should evoke architecture (a glasshouse or hall), not audio clichés: no waveforms, headphones, or play triangles. A conservatory silhouette in libadwaita accent does the work (the Belfry branding discipline).

App ID: `org.gnome.Conservatory` (GNOME Circle) or `io.github.virinvictus.Conservatory`.

**License: GPL-3.0-or-later.** Forced by librubberband (GPL-2-or-later) in the absorbed Smart Speed chain, the same constraint Belfry documents. No license relaxation without proposing a rubberband replacement. Record the full chain in `ATTRIBUTIONS.md`.

---

## 16. Risks and Open Questions

1. **Scope.** This is the largest thing in Brandon's backlog: a library manager, a daily-driver player, and an absorbed podcast client, with a unified queue. It competes with Atrium (still pre-1.0) for the "one big project" slot, and as of v0.0.1 the build has begun *concurrently* with Atrium rather than after it. That concurrency is the risk this section originally warned against; it is now accepted by deliberate decision. The mitigation is no longer deferral but hard phasing (§17): every phase must leave a usable artifact, so attention can swing back to Atrium between phases without leaving Conservatory half-built. If the concurrency proves to be a mistake, the phasing is what makes a pause cheap.
2. **Moving the user's files.** The file-ownership model is the headline risk. A move bug damages a real library. The dry-run, undo journal, and crash-safe replay (§5.4) are release-blocking, not nice-to-have.
3. **Genre instability.** Genre-first physical shelving amplifies the least stable tag into file moves. The shelf-genre field plus rendered template (§5.1–5.2) keep raw tags off disk and make re-shelving cheap, but this is the part most likely to need revision in practice. The genre-tree rollup is the escape hatch if flat shelving churns too much.
4. **Genre vocabulary seed (OPEN).** Ship a default alias map / whitelist (beets `lastgenre` or the MusicBrainz genre list) or start empty and user-built? Decide at implementation.
5. **MusicBrainz tagging (OPEN).** In scope or assume pre-tagged files? Default is out; revisit if curation friction demands it.
6. **EQ / DSP depth (OPEN).** None, a simple EQ, or a deadbeef-class DSP chain?
7. **ReplayGain scan vs read (OPEN).** Scan values in-app, or only read existing tags?
8. **Belfry absorption timing.** Belfry must not be retired until Conservatory reaches podcast parity; `belfry-core`'s worker migrates rather than being rewritten. The `~/.gitrepos` CLAUDE.md project map needs a note when that happens.
9. **libmpv per-item profile switching.** Swapping filter graphs between a music track and a podcast episode mid-queue needs prototyping; gapless within an album plus profile switching at album/kind boundaries is the tricky bit.

---

## 17. Phasing (the build is deferred)

The build was originally deferred until Atrium reached a real shipping milestone, on the reasoning that two concurrent flagship-scale projects is the failure mode to avoid. That deferral has been lifted by deliberate decision; the build has begun alongside Atrium. The discipline that replaces it is hard phasing: each stage below must be usable on its own, so work can move between Conservatory and Atrium without stranding either. The original rationale is kept here rather than deleted, because it is the thing to re-read if the concurrency turns out to be a mistake.

- **Phase 0 (done).** This spec; design. Workspace skeleton bootstrapped at v0.0.1: the four crates, portfolio docs, build files, CI scaffold. No feature code yet.
- **Phase 1.** `conservatory-core` foundation: SQLite worker + read pool + migrations + fixtures (port from `belfry-core`), tag read, the data model.
- **Phase 2.** Import + organize: path-template engine, shelf-genre resolver, file mover with dry-run + undo. The manager is usable headless via the CLI here.
- **Phase 3.** GTK browse: the Columns UI faceted view + search grammar + track list. A working library browser.
- **Phase 4.** Playback: libmpv engine, music profile, unified queue, Now-bar, MPRIS. A daily-driver music player.
- **Phase 5.** Bulk editing + embedded-tag write-back.
- **Phase 6.** Podcasts: absorb the Belfry subsystem behind the Podcasts tab, hook episodes into the unified queue. Podcast parity reached; Belfry can retire.

The manager half (Phases 1–3) must be usable before the player half is finished, and the player must be usable before podcasts arrive. No phase leaves the app non-functional.

---

## 18. Project Conventions

Standard portfolio layout:

- `README.md`, `spec.md` (this file), `roadmap.md`, `patchnotes.md`, `CLAUDE.md`, `ATTRIBUTIONS.md` (design lineage, dependency licenses, the GPL chain analysis).
- `VERSION` is the single source of truth; `Cargo.toml` (workspace and each member) matches.
- `LICENSE` (GPL-3.0-or-later), `logo.svg`.
- `data/` — `.ui` XML, icons, GSettings schema, AppStream metainfo, Flatpak manifest, bundled fonts (registered via fontconfig at first run; never assume host fonts).
- `conservatory-core/`, `conservatory-search/`, `conservatory-cli/`, `conservatory/` — workspace members.
- `tests/` — integration tests alongside in-crate unit tests; the file-mover dry-run/undo and the re-import contract (§5.6) get dedicated fixture-backed suites.
- `docs/` — schema, keymap, path-template reference, genre normalization notes, libmpv profile reference.

CI matches the portfolio: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on Linux. Tests required from day one.
