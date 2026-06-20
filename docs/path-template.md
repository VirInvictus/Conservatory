# Path Template Reference

> **Status: implemented (music) at Phase 2a.** `conservatory-core/src/path_template.rs` renders music paths per this contract; the audiobook tokens (§5.7) land with Phase 7. This expands spec §5.1.

## Implementation notes (Phase 2a)

The engine is pure: `PathTemplate::parse` validates a template (unbalanced
braces, unknown tokens, and malformed format specs are errors), and
`PathTemplate::render(&TrackFields)` is **infallible** once parsed. A template
component is rendered, then empty-group artifacts are collapsed, then the result
is sanitized. Fallbacks keep structural folders non-empty: missing shelf genre →
`Unknown`, missing album artist → `Various Artists`, missing album →
`Unknown Album`, missing title → `Untitled`. Optional pieces (year, track, disc,
track artist) render empty and let their surrounding literals collapse: a missing
year drops ` (<year>)`, a missing track drops the leading `NN - `. Format specs
support zero-padding only (`{track:02}`); a value wider than the pad is not
truncated. `find_collisions` reports tracks that render to the same path, for the
Phase 2c mover to refuse or disambiguate before moving anything.

## The model

The database is truth; the on-disk tree is a **render** of a configurable path template, exactly as Calibre's "save to disk" template and beets' `paths:` config work. Re-shelving an album is a template-or-field change, not a lock-in: change the field, re-render, and the file mover (spec §5.4) relocates the album.

The default template:

```text
{shelf_genre}/{albumartist}/{album} ({year})/{track:02} - {title}
```

rendered under the library root as:

```text
<library root>/
└── <Shelf Genre>/
    └── <Album Artist sort_name>/
        └── <Album> (<Year>)/
            ├── 01 - <Title>.<ext>
            └── cover.jpg
```

## Tokens

| Token | Source | Notes |
|---|---|---|
| `{shelf_genre}` | `albums.shelf_genre` | The **only** genre input to the path (spec §5.2). Single-valued; never a raw tag. |
| `{albumartist}` | `artists.sort_name` of the album artist | "Beatles, The"; compilations resolve to **Various Artists**. |
| `{album}` | `albums.title` | |
| `{year}` | `albums.year` | |
| `{track}` | `tracks.track_no` | Zero-pad with a format spec: `{track:02}`. |
| `{disc}` | `tracks.disc_no` | Optional; for multi-disc layouts. |
| `{title}` | `tracks.title` | |
| `{artist}` | `artists.name` of the track artist | May differ from album artist. |
| `{ext}` | derived from `tracks.format` | Appended automatically; not written into the template body. |

Format specs follow the `{token:spec}` shape; `{track:02}` is the only one the music default uses. The exact spec mini-language firms up at implementation; zero-padding is the minimum.

## Audiobook tokens (Phase 7, spec §5.7)

Audiobooks are owned like music (the database renders the tree; the mover relocates them), but use their own template. Default:

```text
Audiobooks/{author}/{series}/{series_index:02}. {title} ({year})
```

| Token | Source | Notes |
|---|---|---|
| `{author}` | `book_people.sort_name` of the primary author | First credited author; multi-author books bucket under the primary. |
| `{narrator}` | `book_people.sort_name` of the narrator | Available, not in the default layout. |
| `{series}` | `series.name` | **Collapses** (whole component drops) when the book is standalone. |
| `{series_index}` | `books.series_sequence` | Decimal-aware (`1.5`); `{series_index:02}` zero-pads the integer part. Collapses with `{series}`. |
| `{title}` | `books.title` | |
| `{year}` | `books.year` | |
| `{shelf_genre}` | `books.shelf_genre` | Optional; same single-valued decoupling as music, not in the default audiobook layout. |

A single-file M4B book keeps its one file inside the rendered book folder; a multi-file book keeps its per-chapter files there. Cover art is `cover.jpg` in the book folder.

## Invariants

- **An album (or a book) is the unit that moves.** A single album resolves to exactly one path: one shelf genre and one album artist drive the directory, even when track-level genres or artists disagree. A book likewise resolves to one path (one primary author, one optional series). This is what keeps a move atomic and undoable.
- **Various Artists.** Compilations (no single album artist) resolve their album-artist component to a `Various Artists` bucket.
- **Raw tags never reach the filesystem.** Only `shelf_genre` (single-valued, resolved per spec §5.2) feeds `{shelf_genre}`. Multi-value `track_genres` are for facets and search only.
- **Cover art** is written as `cover.jpg` in the album folder (spec §7.4).

## Sanitization

Rendered components are made filesystem-safe before they touch disk:

- Strip or replace path separators (`/`, and `\` on principle) inside a component.
- Guard against reserved names and trailing dots/spaces.
- Cap component length to stay within common filesystem limits; the cap is applied per component, not per full path.
- Collapse whitespace; never emit an empty component (a missing `{year}` collapses the ` (<Year>)` suffix rather than leaving `()`).

Sanitization is a render concern, not a tag concern: the embedded tag keeps the true value (spec §5.5); only the on-disk name is sanitized.

## Editing the template

The template is user-editable (config `library.path_template`, spec §10). Changing it, or changing any field it renders (shelf genre, album artist, album, year), means the album's current location no longer matches its rendered location. `conservatory-cli organize` (and the GUI equivalent) re-renders the tree from the database and enqueues the resulting moves as a single job with a dry-run preview and undo (spec §5.4). The template engine only computes paths; it never moves files itself.

## Examples

| Album | Rendered path (default template) |
|---|---|
| Boards of Canada, *Geogaddi* (2002), Electronic | `Electronic/Boards of Canada/Geogaddi (2002)/03 - Music Is Math.flac` |
| Various, *Artificial Intelligence* (1992), Electronic | `Electronic/Various Artists/Artificial Intelligence (1992)/01 - I.A.O. (Polygon Window).flac` |
| Bill Evans, *Sunday at the Village Vanguard* (1961), Jazz | `Jazz/Evans, Bill/Sunday at the Village Vanguard (1961)/01 - Gloria's Step.flac` |
