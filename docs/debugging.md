# Debugging

Conservatory has a `--debug` mode (Phase 14) that turns either binary into a
verbose diagnostic stream on **stderr**. It is off by default and adds no
overhead when off (the costly hooks are only installed when the flag is set).

## Turning it on

```bash
# The GUI
conservatory --debug <db> <library-root>

# The CLI (the flag is global; it works before or after the subcommand)
conservatory-cli --debug import <db> <source> <root>
conservatory-cli import <db> <source> <root> --debug
```

`-d` is the short form. Output goes to stderr, so stdout stays clean for a
program's actual results.

## The four channels

Every diagnostic line is tagged with one of four targets, so you can keep the
whole firehose or narrow it to one concern:

| Channel | What it logs |
|---|---|
| `conservatory::sql` | Every SQLite statement and its wall-clock time (`us=`), tagged `role=writer`/`reader`. |
| `conservatory::io` | Filesystem mutations: the file mover (rename / copy + fsync + rename / revert), cover writes, tag write-back, APE stripping, the import scan, podcast downloads and retention deletes, and playlist / OPML export. |
| `conservatory::net` | HTTP requests (podcasts only; there is no other network): feed fetches (GET / 304 / response), episode downloads, and chapter fetches. |
| `conservatory::mem` | Resident set size (RSS), sampled at lifecycle points (startup, library-loaded, CLI start/end) and every five seconds in the running GUI. |

## Narrowing with `RUST_LOG`

`--debug` is the one switch that installs the deep hooks (the SQL profiler and the
memory sampler). `RUST_LOG` then narrows what is printed, using the standard
[`tracing` env-filter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
syntax:

```bash
# SQL only
RUST_LOG=conservatory::sql=debug conservatory-cli --debug import <db> <src> <root>

# IO and network, nothing else
RUST_LOG=conservatory::io=debug,conservatory::net=debug conservatory-cli --debug podcast refresh <db>

# Everything our crates emit, at debug
RUST_LOG=conservatory=debug,conservatory_core=debug conservatory --debug <db> <root>
```

`RUST_LOG` overrides the default filter entirely, so if you set it, include the
channels you want.

## Memory and the budget

The `conservatory::mem` lines report RSS read from `/proc/self/status` (Linux),
so they cost nothing and need no profiler. They give a real number to check
against the spec §13 budget (under 200 MB idle, under 300 MB active on a
50k-track library). For heap-level detail, use the external tools the spec names
(`heaptrack`, `massif`); the built-in sampler is for a quick RSS read, not a heap
profile.

## The facet-switch bench

`CONSERVATORY_BENCH=1` turns one GUI launch into a facet-switch timing report on
stderr (the §13 facet-change latency budget; no-op without the env var):

```bash
conservatory-cli debug-fixture --scale gate /tmp/bench.db   # the 50k corpus
CONSERVATORY_BENCH=1 conservatory /tmp/bench.db
```

After startup it narrows the leaf by a sampled genre five times, lifts back to
`[All]` (the worst case: every master row re-enters), times a second lift (the
first big diff through the models carries a one-time warm-up cost), times a
master rebuild (the data-refresh path), and prints the pre-0.10.0 switch cost
(the full row projection plus a fresh row-object rebuild) for comparison, plus
the downstream facet-pane recompute. Reported numbers cover the model work; the
frame paints the visible rows afterwards, off that clock. Zero cost when unset.

## A note on the sleep-timer test

The `after_timer_fires_pauses_and_tap_extends` integration test drives a
real-time countdown through the live engine, so it is flaky under heavy build
load and is `#[ignore]`d from the default `cargo test`. Its behaviour is covered
deterministically by the `player::sleep` unit tests and the boundary sibling
integration tests. Run it explicitly with:

```bash
cargo test -p conservatory-core --test sleep -- --ignored
```
