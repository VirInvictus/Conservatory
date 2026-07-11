# Scrobbling reference (Phase 9)

> **Status: living reference.** Phase 9a (v0.3.1) landed the headless spine documented here: the outbox, the ListenBrainz client, the `[scrobble]` config, and the `scrobble` CLI verb. Phase 9b wires the engine's play-completion hook and the GUI "Sync" prefs; Phase 9c adds Last.fm. The companion to spec §14 (the scrobble carve-out) and spec §10 (`[scrobble]`).

## What it is, and what it is not

Scrobbling submits completed plays to an external listening-history service. It is the deliberate, scoped reversal of the spec §14 "no social" line, kept to one-way history submission, not a social product. It is **optional and off by default**: with `[scrobble] enabled = false` (the default) the subsystem is inert and the app is unchanged and fully offline. **ListenBrainz leads** (open, self-hostable, fits the offline-first house rule); Last.fm is an optional second target (Phase 9c).

## Local-first: the outbox

A play is never submitted synchronously. On a natural completion (Phase 9b), the play is written to the `scrobble_outbox` table (migration 0020) **first**, and a background submitter drains it. This is the local-first guarantee: a listen is recorded locally, then synced when the network returns, and is never lost if the service is down.

The outbox row **snapshots** the listen metadata (artist, track, album, track number, duration, recording MBID) at completion time, so:

- a later library rename cannot corrupt history, and
- submission needs no join back to the live tables.

Columns (`conservatory-core/src/db/migrations/0020_scrobble_outbox.sql`):

| Column | Meaning |
|---|---|
| `service` | `listenbrainz` \| `lastfm`, snapshotted so switching services later cannot misroute a queued listen |
| `kind` | `track` \| `episode` (scope + accounting; the services do not distinguish) |
| `listened_at` | unix seconds the play completed |
| `artist` / `track` / `album` / `track_number` / `duration_secs` / `recording_mbid` | the snapshotted listen metadata |
| `attempts` | failed-submission count, drives the backoff |
| `next_attempt_at` | unix seconds; the drain skips a row until this time |
| `created_at` | unix seconds enqueued |

## The drain loop

`conservatory-core/src/scrobble.rs`:

- `drain_ready(worker, pool, service, submitter, now, limit)` reads the ready rows (`next_attempt_at <= now`) via the pool, submits each through the `ListenSubmitter`, and writes the outcome through the single-writer worker:
  - **success** → delete the row;
  - **transient failure** (429 / 5xx / offline) → `bump_scrobble_attempt` with `next_attempt_at = now + backoff(attempts)`, exponential from 60 s and capped at 1 h;
  - **permanent failure** (a bad token / rejected payload) → parked 24 h out rather than deleted, so the listen is not lost and does not hammer.
- Rows whose snapshotted `service` differs from the draining service are left untouched (per-listen routing).
- `run(worker, pool, service, submitter)` is the background task: it wakes every 60 s and drains, returning when the worker channel closes. The GUI spawns it on its runtime (Phase 9b), the `mpris::run` precedent.

The pure pieces are unit-tested (`backoff_secs`, the ListenBrainz payload builder, the status classification); the drain loop is integration-tested against a fake submitter that simulates an offline window (`conservatory-core/tests/scrobble.rs`), and `ListenBrainzClient` against a wiremock server.

## Configuration

`[scrobble]` (spec §10), owned by `config.toml`:

```toml
[scrobble]
enabled = false            # off by default; true is required for 9b's engine hook to enqueue
service = "listenbrainz"   # "listenbrainz" | "lastfm"; parsed forgivingly (unknown -> listenbrainz)
```

The **user token is not in the config file.** It lives in the libsecret secret service (the app-wide `CredentialStore`, `conservatory-core::secret`), keyed per service (`scrobble.listenbrainz.token`, `scrobble.lastfm.session`), so switching services keeps both tokens.

## CLI (`scrobble` verb)

The headless surface (`conservatory-cli`):

```
conservatory-cli scrobble status <db>            # enabled / service / pending count
conservatory-cli scrobble token set <token>      # store the token in libsecret (configured service)
conservatory-cli scrobble token clear            # remove it
conservatory-cli scrobble flush <db>             # force one drain pass now
conservatory-cli scrobble test                   # validate the stored token against the service
```

`token set` / `test` / `flush` accept `--service` to target a specific service; otherwise they use the configured one.

## Scope

Music tracks and podcast episodes only; **audiobooks are excluded** (a 14-hour book is not a "listen"). The engine enforces this at the completion hook (Phase 9b): only `MediaKind::Track` and `MediaKind::Episode` EOFs enqueue.

## ListenBrainz protocol

`ListenBrainzClient` (base URL overridable for a self-hosted instance) speaks two endpoints:

- `POST /1/submit-listens` with `Authorization: Token <token>` and a `listen_type: "single"` body (`listenbrainz_submit_body`, pure). 2xx succeeds; 429 / 5xx are transient; other 4xx are permanent.
- `GET /1/validate-token` for the `scrobble test` verb.

See <https://listenbrainz.readthedocs.io/en/latest/users/api/core.html>.
