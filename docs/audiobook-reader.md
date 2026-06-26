# Audiobook Reader Reference

> **Status: implemented at Phase 7a-ii (v0.0.54).** `conservatory-audiobooks`'s `read_book` turns a folder or single file into a `BookDraft` (metadata + ordered chapters). This is the headless, pre-database view the Phase 7a-iii import pipeline resolves into `books` / `book_people` / `series` / `book_chapters` rows. No DB writes, no file moves, no covers/accent here. Expands spec §4.5 and §5.7.

## Sources and precedence

Three metadata sources, merged **per field** by precedence:

```text
sidecar  >  embedded tags  >  folder structure
```

The Audiobookshelf sidecars are the explicit, user-curated signal and win; the embedded tags are the common case; the on-disk layout is a last-resort fallback. Every field is optional, so the merge takes the first source that supplies it.

## Embedded tag mapping (lofty)

Audiobook tagging is not standardized, so the mapping is the convention that matches real libraries (validated against the testdata), read tolerantly (a missing tag is `None`):

| Draft field | Tag source |
|---|---|
| title | `AlbumTitle` (the book title lives in the album tag), else `TrackTitle` |
| subtitle | `TrackSubtitle` (ID3 `TIT3`) |
| authors | `AlbumArtist`, else `TrackArtist`, split on credits |
| narrators | `Composer` (both testdata books carry it there) plus a custom `NARRATOR` frame, merged |
| series / sequence | custom `SERIES` / `SERIES-PART` frames (sequence parsed as a decimal) |
| year | `Year` / `RecordingDate` (leading four digits) |
| publisher | `Publisher` |
| isbn / asin | custom `ISBN` / `ASIN` frames |
| description | `Comment` |
| part number | `TrackNumber` (orders a multi-file book's parts) |
| cover | the first embedded picture, else a sibling `cover.jpg` |

**Custom frames** surface through lofty's unified tag as `ItemKey::Unknown(..)`: an ID3v2 `TXXX:NARRATOR` is `Unknown("NARRATOR")`, while an MP4 freeform atom is `Unknown("----:com.apple.iTunes:NARRATOR")`. The reader tries both spellings, so one path covers mp3 and m4b.

**People** are split on `, ; &` and ` and ` (a full-cast M4B packs every narrator into one field) and sorted **last-name-first** (`person_sort_name`: "Patrick Rothfuss" → "Rothfuss, Patrick"). This is the Calibre `author_sort` convention (spec §4.5), deliberately *not* `conservatory-core`'s `derive_sort_name`, which only moves a leading article for band/album names.

## Sidecars (Audiobookshelf conventions)

- `metadata.opf` — OPF 2 / Dublin Core (parsed with `quick-xml`): `dc:title`, `dc:creator` with `opf:role` (`aut` → author, `nrt` → narrator), `dc:description`, `dc:language`, `dc:publisher`, `dc:date`, `dc:identifier` (`ISBN` / `ASIN` schemes), plus Calibre `<meta name="calibre:series" / "calibre:series_index">`.
- `desc.txt` — a plain-text description (overrides the `.opf` description).
- `reader.txt` — the narrator(s) (overrides the `.opf` narrators).

## Folder-structure inference (fallback)

When neither tags nor a sidecar give author / series / title, the layout often does:

- `Author/Title/` — a standalone book;
- `Author/Series/NN - Title/` — a series entry (the `NN` is the decimal sequence).

A series entry is recognised conservatively: the title folder must carry a leading index **and** have a grandparent. A bare numeric folder (`1984`, no separator and remainder) is not an index, so a numerically-titled standalone is never mistaken for a series entry.

## Chapter resolution

A `BookChapter` is `(file_path, file_offset, duration)`, addressing either a standalone per-chapter file (`file_offset` 0) or a span inside one M4B. Three cases:

1. **one file with embedded chapters** — read via an `ffprobe -show_chapters` shell-out (lofty cannot read MP4 chapter atoms; this avoids a Rust MP4 dependency, the `rsgain` precedent, the `m4b-tool` technique). One draft per chapter, at its start offset.
2. **one file with no chapters** — a single whole-file chapter.
3. **a multi-file folder** — one draft per file, ordered by the part tag (a multi-part M4B carries `1/11 .. 11/11`, so a lexical filename sort is wrong). A per-file title that merely repeats the book title is replaced by a synthesized "Part N".

`ffprobe` is best-effort: when it is absent, a single file degrades to one whole-file chapter rather than failing. The `ffprobe` integration test skips cleanly without the binary; the pure resolver logic (the raw-chapter mapping and the part ordering) is unit-tested with no audio.

> Opt-in silence-based chapter detection (spec §16.11) is deferred.
