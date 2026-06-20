# File Mover Reference

> **Status: implemented (music) at Phase 2c.** `conservatory-core/src/mover/`. This expands spec §5.4 and is the contract that subsystem builds against.

Moving the user's files is the headline risk: a move bug damages a real library (CLAUDE.md hard rule). Every relocation is a journaled **job** with a dry-run preview, an undo journal written *before* any file is touched, and crash-safe roll-forward replay. The journal lives in SQLite (migration `0002`); the file I/O runs off the single-writer worker.

## The job lifecycle

1. **Plan** (`mover::plan`, pure): render targets (Phase 2a) into a set of `MoveOp`s, then check for conflicts (duplicate targets, missing sources, existing destinations). A plan with any conflict is **refused** by `apply`: no silent overwrite or auto-rename.
2. **Journal** (`create_move_job`, one transaction): the job and all operations are written `pending`, durable via WAL, **before any file is touched**. This is the crash-safety record.
3. **Execute** (off-worker): for each operation, `fsops::relocate` moves the file, then `complete_operation` (one transaction) marks the op `done` and applies the DB path it implies (`tracks.file_path`, and `albums.folder_path` = the new parent).
4. **Finalize**: the job is set `completed`.

## The crash-safety contract

The ordering is the guarantee:

1. Journal `pending` (committed) — before any file moves.
2. Move the file (`fsops::relocate`, idempotent).
3. `complete_operation` transaction — op `done` + DB path update.

A crash between (2) and (3) leaves the operation `pending` while the file is already at its target. On restart, `recover` (roll-forward) replays every `pending` op of every `in_progress` job; `relocate` sees the valid target and missing source and treats it as done, so only the DB is finalized. Recovery is idempotent and always drives a job to a consistent `completed` state. This is the spec §5.4 "journal written before the move, replayed on restart" guarantee.

**Roll-forward, not roll-back:** recovery completes the move the user asked for. Undo is a separate, explicit action (`mover::undo`) on a completed job: it reverts each `done` operation in reverse (move the file back for `Move`; delete the copy for `Copy`) and restores the pre-move DB path.

## The per-file primitive (`fsops`)

`relocate(src, dst, mode)`:
- same-filesystem `rename` (atomic) fast path;
- on a cross-device error (`EXDEV`), fall back to **copy → fsync → verify (size) → delete source** (Move) or keep source (Copy), via a same-dir temp file (`.conservatory-part`), modeled on Atrium's `write_atomic`;
- **idempotent**: `src` gone and a valid `dst` present is a no-op success (the op completed before a crash).

## Conflict policy

`apply` refuses the whole job if `plan` reports any conflict; nothing moves. Conflicts: a duplicate target across the batch, a missing source, or an existing destination. The user resolves them (re-tag, re-render) and re-runs. Sanitization and collision detection come from the Phase 2a path engine (`find_collisions`).

## Paths

The DB stores paths **relative** to the library root (the library stays relocatable); the journal stores **absolute** `src_path`/`dst_path` for direct filesystem ops. `library_root` is a parameter for now (config-driven root arrives with Phase 10).

## Out of scope (for now)

Pruning directories left empty after a move/undo (harmless leftovers); the full scan → tag → resolve → render → move import pipeline and the real `import`/`organize` verbs (Phase 2d); embedded-tag write-back (Phase 5b).
