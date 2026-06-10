# Genre Normalization Notes

> **Status: design reference, not yet implemented.** The shelf-genre resolver lands at roadmap Phase 2b. This expands spec §5.2 and §7.2 and is the contract that resolver builds against.

## The problem

Genre is the least canonical, most multi-valued tag in a music library. The same album might be tagged `IDM`, `Electronic; Ambient`, `electronica`, or nothing. Conservatory shelves albums **by genre on disk** (the default template's top level, spec §5.1), which amplifies that instability into file moves. The whole design exists to absorb that instability without ever letting raw tags reach the filesystem.

## Two decoupled values

- **`track_genres`** (multi-valued, `track_genres` join table): the raw tags, preserved untouched, for facets and search (`genre:` in the grammar). Never normalized, never written to disk.
- **`shelf_genre`** (single-valued, `albums.shelf_genre`): the filed-under value. The **only** input to the genre folder level. Editable, overridable, bulk-editable. This is the Calibre `author_sort` trick: the shelving key is a separate field, not the raw tag.

These two are deliberately different fields. `genre:ambient` (raw) and `shelfgenre:Electronic` (filed-under) can both be true of the same album.

## The normalization layer

Runs before the resolution chain. Given a raw genre string:

1. **Split** on `;`, `/`, `,` into individual genres.
2. **Case-fold** each.
3. **Map** each through `genre_aliases` (`raw → canonical`). For example `IDM → Electronic`, `Hip Hop`/`Rap → Hip-Hop`.

The output is a set of normalized, canonical genre names. The seed source for the alias map is **OPEN** (spec §16.4): ship a default vocabulary (beets' `lastgenre` whitelist, or the MusicBrainz genre list) or start empty and let the user build it. Decide at implementation and record the choice here.

## The resolution chain

`shelf_genre` is auto-filled on import by a priority chain. The first rule that produces a value wins:

1. **Manual override**, if the user has set one (never overwritten by re-import).
2. else a **single album-level genre tag**, if the album carries one.
3. else the **most common normalized genre** across the album's tracks; ties broken by the user's `genre_priority` list (`genre → rank`), then by first-seen.
4. else the **`Unknown` bucket** (config `genre.default_unknown`, default `"Unknown"`).

## The tables (spec §4.1)

```sql
CREATE TABLE genre_aliases  (raw TEXT PRIMARY KEY, canonical TEXT NOT NULL);
CREATE TABLE genre_priority (genre TEXT PRIMARY KEY, rank INTEGER NOT NULL);
```

`genre_aliases` drives step 3 of normalization. `genre_priority` is the tie-break for step 3 of the resolution chain.

## Editing and its consequences

`shelf_genre` is always overridable per album and bulk-editable (spec §3.5). **Editing it moves the album's directory** (spec §5.4), as a job with a dry-run preview and undo. Raw `track_genres` are never touched by any of this.

## The flat-vs-tree decision

Resolution is **flat**: the normalized `shelf_genre` value is used verbatim as the top folder. A genre *tree* with rollup (leaf tags like `Synthwave` collapsing to a coarse parent like `Electronic`) was considered and deferred to a possible v2 (spec §16.3). The flat model plus a good alias map is the v1 bet; the tree is the escape hatch if flat shelving churns too much in practice.

## Worked example

An album with tracks tagged:

```
Track 1: "IDM; Electronic"
Track 2: "Electronic"
Track 3: "Ambient, Electronic"
```

Normalization (`IDM → Electronic`) yields per-track sets `{Electronic}`, `{Electronic}`, `{Ambient, Electronic}`. Most common across tracks is `Electronic` (3) over `Ambient` (1). With no manual override and no album-level tag, the chain lands on **`shelf_genre = "Electronic"`**, and the album files under `Electronic/`. The raw `Ambient` tag still surfaces the album under Ambient in the genre facet and matches `genre:ambient`.
