# Search Grammar (`conservatory-search`)

> **Status: design reference, not yet implemented.** `conservatory-search` lands at roadmap Phase 3a. This document is the contract that crate builds against; it expands spec §3.4 and is the thing to read before touching the search crate. Where it describes behaviour in the present tense, read "will."

## The one-line design decision

`conservatory-search` takes its **structure from `atrium-search`**, its **domain semantics from CalibreQuarry**, and its **full-text plumbing from Viaduct**. It is an independent implementation of all three (the Belfry precedent: port the shape, write the code fresh, so the projects evolve without coupling).

That split is not arbitrary. Brandon has built the same idea three times in three shapes, and each got one part right for Conservatory:

## Three prior search engines, compared

| Axis | Atrium (`atrium-search`) | Viaduct | CalibreQuarry (`search.py`) |
|---|---|---|---|
| Shape | text expression grammar | structured conditions + raw FTS5 | text expression grammar |
| Pipeline | lex → parse → typed AST → {eval, SQL} | none (dialog-built) / `MATCH` | `re.Scanner` → nested-list AST |
| AST | typed enums (`Expr`, `Field`) | `Vec<Condition>` | Lisp-style nested lists |
| Boolean | AND/OR/NOT, precedence, implicit AND | **AND only** | AND/OR/NOT, precedence, implicit AND |
| Evaluation | per-item bool **or** SQL `WHERE` | SQL (conditions) / FTS5 | **candidate-set** algebra |
| Storage coupling | decoupled crate, dual-path | lives in DB layer | in-Python on read-only provider |
| Match kinds | substring/exact/regex/fuzzy/bool | fixed per condition | contains/exact/regex/accent |
| Ranking | bm25 + recency half-life | FTS5 bm25 / snippet | none (set membership) |
| Bad input | degrades to substring | n/a | raises `ParseException` |
| Persistence | text, re-parsed | JSON conditions | text (`vl:` exprs) |
| Origin | own design, Calibre-inspired | NetNewsWire lineage | direct Calibre port |

**Atrium** is a real compiler: `lex` (string → tokens) → `parse` (tokens → typed AST + extracted sort specs) → `ast` (a round-trippable `Expr`) → two consumers, `eval` (AST → bool in memory) and `sql_translate` (AST → SQL `WHERE`). Its defining move is the **all-or-nothing dual path**: `try_translate` emits SQL only if *every* node maps cleanly; if any node can't (regex, fuzzy, composite predicates), it returns `None` and the caller runs the in-memory evaluator instead, so the two paths can never silently diverge. On top sits a `bm25` + recency-half-life ranking for bare-text hits. The crate carries no storage types, so it can be fuzzed and reused.

**Viaduct** has no expression grammar at all. It has two unrelated mechanisms: AND-only structured **Smart Feeds** (a `Vec<Condition>` built by a dialog and persisted as JSON), and a separate `Search(String)` op that throws the user's string straight at FTS5 `MATCH` with `snippet()` excerpting. Useful as the full-text plumbing reference; explicitly *not* the model for Perspectives.

**CalibreQuarry** is a faithful port of Calibre's own parser. It tokenizes with `re.Scanner` into a Lisp-style nested-list AST and evaluates with **candidate-set algebra**: it works on sets of book IDs where `and` = intersection, `or` = union, `not` = difference, and each leaf returns its matching subset. Matching is **datatype-dispatched**: a field resolves to a datatype (`text`, `text_multi`, `hier`, `rating`, `int`, `float`, `date`, `bool`, `identifiers`, `vl`) and dispatches accordingly. It implements multi-valued matching, numeric relops with size suffixes, date precision and keywords, and `vl:` virtual-library references with cycle detection.

## What Conservatory takes from each

- **From Atrium (structure):** the lex/parse/typed-AST/eval/`sql_translate` layout; the all-or-nothing dual evaluation path; **forgiving degrade-to-substring** on malformed input (this is literally spec §3.4's "yellow filter-bar tint, never an error"); `sort:` lifted to result metadata rather than living in the predicate AST; bm25 + recency ranking for bare text; the decoupled, fuzzable crate boundary; **Perspectives as saved text re-parsed on load** (so they inherit later grammar additions), which is Atrium's round-trip discipline.
- **From CalibreQuarry (semantics):** Conservatory is "Calibre for audio," so the *matching* is Calibre's: datatype-dispatched fields, multi-valued `genre:` faceting (a track tagged `Electronic; Ambient` matches both), numeric relops (`rating:>=4`, `bitrate:`, `duration:`), date keywords and ranges (`added:thisweek`, `year:1998..2004`), `true`/`false` presence tests, and **Perspectives as composable saved searches with cycle detection** (Calibre's virtual library, `vl:`).
- **From Viaduct (plumbing):** the trigger-synced FTS5 virtual tables (`track_fts`, `album_fts`, plus `episode_fts`/`show_fts` at Phase 6 and `book_fts` at Phase 7, spec §4.4), the `MATCH` + `snippet()` query shape, and the shared client/timeout discipline where relevant.

## Two deliberate divergences

1. **SQL pushdown matters more here than in Atrium.** With a 50k-track library and a sub-100ms facet-repaint budget (spec §13), the SQL `WHERE` path is what hits budget; CalibreQuarry's pure-in-process candidate-set approach is fine for a CLI but Conservatory leans on `try_translate` harder. The in-memory fallback may still borrow the candidate-set algebra for the non-translatable subset.
2. **Forgiving, not raising.** Follow Atrium (degrade to substring) over CalibreQuarry (`ParseException`). The spec is explicit: malformed input is never an error.

## Grammar surface (spec §3.4)

One grammar, all three surfaces (music, podcasts, audiobooks). The filter bar above any list accepts the full expression language; `Ctrl+F` focuses it; there is no separate search mode. The typed domain is Track / Album / Artist / Show / Episode / Book.

### Fields

| Field | Datatype | Example |
|---|---|---|
| `artist:` | text-multi | `artist:"Aphex Twin"` |
| `albumartist:` | text-multi | `albumartist:"Boards of Canada"` |
| `album:` | text | `album:Geogaddi` |
| `title:` | text (FTS) | `title:roygbiv` |
| `genre:` | text-multi (raw tags) | `genre:ambient` |
| `shelfgenre:` | text (single-valued, filed-under) | `shelfgenre:Electronic` |
| `year:` | int / range | `year:1998..2004` |
| `added:` | date | `added:thisweek` |
| `rating:` | int (0–5) | `rating:>=4` |
| `bitrate:` | int | `bitrate:>=900` |
| `duration:` | float (seconds) | `duration:>600` |
| `format:` | text-multi | `format:flac` |
| `is:played` / `is:starred` / `is:queued` | state | `is:starred AND genre:jazz` |
| `show:` / `is:in_inbox` / `pub:` (Phase 6) | podcast | as Belfry §3.7 |
| `author:` / `narrator:` (Phase 7) | text-multi | `author:"Brandon Sanderson"` |
| `series:` (Phase 7) | text | `series:"The Stormlight Archive"` |
| `is:finished` (Phase 7) | state | `author:sanderson AND is:finished false` |

`genre:` vs `shelfgenre:` is the central decoupling (spec §5.2): `genre:` matches any of a track's raw multi-value tags (for facets and search); `shelfgenre:` matches the single filed-under value that drives the filesystem. They are deliberately different fields.

### Modifiers and operators

- **Match modifiers:** substring (default), `"quoted substring"`, `=exact`, `~regex`, `?fuzzy` (Damerau-Levenshtein), and `true`/`false` existence on optional fields.
- **Boolean:** `AND` / `OR` / `NOT` (case-insensitive), implicit `AND` between bare tokens, `!` prefix as `NOT`. Precedence `NOT > AND > OR`; parentheses group.
- **Comparison / range:** `=` `!=` `>` `<` `>=` `<=` on numeric and date fields; `lo..hi` inclusive ranges.
- **Date keywords:** `today`, `yesterday`, `thisweek`, `thismonth`, `thisyear`, `Ndaysago`, plus `YYYY`, `YYYY-MM`, `YYYY-MM-DD` with field-count precision.
- **Sort:** `sort:KEY` / `sort:-KEY`, metadata on the result set, not a predicate.

### Perspectives

Named saved expressions (Calibre saved searches; Atrium's term). Stored as **text** and re-parsed on load so they inherit later grammar additions. A Perspective can target tracks, albums, episodes, or books, can be referenced from another expression like a Calibre virtual library (with cycle detection), and can act as a queue source (spec §6.1).

## Architecture the crate implements

```text
input string
   │  lex          tokens
   ▼
 parse  ──────────►  ParseResult { expr: Expr, sorts: Vec<SortSpec> }
   │                       │
   │  (degrade-to-substring on malformed input; never errors)
   ▼                       ▼
 Expr (typed AST) ── try_translate ──► Some(SqlClause)  → SQL WHERE (fast path)
                       │  all-or-nothing
                       └► None          → eval(Expr, item) (in-memory fallback)
```

- `lex` / `parse` / `ast` / `eval` / `sql_translate` / `rank` modules, mirroring `atrium-search`.
- Typed `Field` enum; unknown field names degrade to substring (forward-compat).
- `SqlValue` carries no `rusqlite` types; the binary maps it to its driver. Keeps the crate GUI/storage-agnostic and fuzzable.

## Open items

- Fuzzy threshold and whether `?fuzzy` is worth the in-memory-only cost on a 50k-track library (Atrium has it; Conservatory may gate it).
- Whether the in-memory fallback adopts CalibreQuarry's candidate-set algebra wholesale or a per-item predicate like Atrium's `eval`.
- Podcast field set firms up when the Belfry subsystem is absorbed (Phase 6); the audiobook field set (`author:`, `narrator:`, `series:`, `is:finished`) firms up at Phase 7.
