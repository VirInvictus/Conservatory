//! The Phase 20 memory-gate harness (ported from Viaduct's `mem_check` bin,
//! roadmap "the mem_check harness port"): builds the synthetic 50k-track
//! corpus through the real single-writer worker, warms the exact browse load
//! the GUI performs at startup, then reads `VmHWM` / `VmRSS` (kB) from
//! `/proc/self/status` and reports pass/fail against the spec §13 budgets.
//!
//! Two checkpoints are reported:
//!
//! - **post-generate** — exercises the write path end to end (50k tracks,
//!   5k albums, 1k artists, genre links; every insert a worker round-trip).
//! - **post-warm-load** — the GUI's idle shape: `facet_rows` for the default
//!   panes plus the full `facet_tracks` leaf (the `Vec<TrackBrief>` the
//!   browse window holds), then a drop-and-settle delta as the leak check.
//!
//! The §13 idle target (< 200 MB) is the *app* number: GTK4's C-side floor
//! (~150 MB, spec §13) is deliberately absent here, so this harness gates the
//! core-side share and catches data-path regressions in one command. The full
//! gate runs the real binary against a working copy of the real library
//! (roadmap Phase 20); the two numbers together are the record.
//!
//! Usage:
//!
//! ```sh
//! cargo run --release --bin mem_check
//! ```
//!
//! Run in release mode — debug builds carry enough instrumentation that the
//! reported peak is misleading.

use std::path::PathBuf;

use conservatory_core::db::FacetField;
use conservatory_core::db::fixtures::{self, FixtureScale};
use conservatory_core::db::{ReadPool, facet_rows, facet_tracks, spawn_worker};

const IDLE_BUDGET_MB: u64 = 200;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The GUI's runtime shape (one worker thread shared with everything).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    // The DB goes into a tempdir, never the user's XDG state. Best-effort
    // cleanup on exit; not critical if it lingers on crash.
    let tmp = make_tempdir()?;
    let db = tmp.join("memcheck.db");

    let baseline = read_vm_hwm_mb().unwrap_or(0);
    println!("== conservatory memory checkpoint ==\ncorpus: FixtureScale::Gate (50,000 tracks)");
    println!("baseline peak RSS (VmHWM): {} MB", baseline);

    let worker = spawn_worker(db.clone())?;
    let start = std::time::Instant::now();
    fixtures::generate(&worker, FixtureScale::Gate).await?;
    let gen_elapsed = start.elapsed();

    let post_gen_peak = read_vm_hwm_mb().unwrap_or(0);
    let post_gen_rss = read_vm_rss_mb().unwrap_or(0);
    println!("-- post-generate checkpoint --");
    println!("insert time: {:?}", gen_elapsed);
    println!("peak RSS (VmHWM): {} MB", post_gen_peak);
    println!("current RSS (VmRSS): {} MB", post_gen_rss);

    // The warm idle load, mirroring the GUI startup: the three default facet
    // panes (config `[browse].panes` default) and the full unfiltered leaf.
    let pool = ReadPool::new(db, 3)?;
    let conn = pool.open()?;
    let panes = [
        FacetField::Genre,
        FacetField::AlbumArtist,
        FacetField::Album,
    ];
    let mut facet_rows_total = 0usize;
    for field in panes {
        let rows = facet_rows(&conn, field, &[])?;
        facet_rows_total += rows.len();
    }
    let leaf = facet_tracks(&conn, &[])?;
    let leaf_len = leaf.len();
    drop(conn);

    // Give SQLite's page cache a beat to settle, then measure.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let post_load_peak = read_vm_hwm_mb().unwrap_or(0);
    let post_load_rss = read_vm_rss_mb().unwrap_or(0);
    println!("-- post-warm-load checkpoint --");
    println!(
        "facet rows: {} (3 panes); leaf tracks: {}",
        facet_rows_total, leaf_len
    );
    println!("peak RSS (VmHWM): {} MB", post_load_peak);
    println!("current RSS (VmRSS): {} MB", post_load_rss);

    // The leak check (Viaduct's background-cycle analog): drop the browse
    // model, let the allocator settle, and report the delta. The harness has
    // no GUI to hold widgets, so a leak here is core-side by construction.
    drop(leaf);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let post_drop_rss = read_vm_rss_mb().unwrap_or(0);
    println!("-- post-drop checkpoint --");
    println!("current RSS (VmRSS): {} MB", post_drop_rss);
    println!(
        "RSS delta after drop: {} MB",
        post_load_rss as i64 - post_drop_rss as i64
    );

    worker.shutdown_ack().await?;
    let _ = std::fs::remove_dir_all(&tmp);

    println!("== results ==");
    let mut failed = false;
    // The §13 idle budget is the app number (GTK floor included); the
    // core-only harness failing at it means the data path alone blows the
    // whole envelope, which is exactly the regression this bin exists to catch.
    if post_load_rss > IDLE_BUDGET_MB {
        eprintln!(
            "FAIL: warm-load RSS {} MB exceeds the spec §13 idle budget of {} MB",
            post_load_rss, IDLE_BUDGET_MB
        );
        failed = true;
    } else {
        println!(
            "PASS: warm-load RSS {} MB under the {} MB idle budget (core-side share; the app gate adds the GTK floor)",
            post_load_rss, IDLE_BUDGET_MB
        );
    }
    if post_load_rss as i64 - post_drop_rss as i64 > leaf_len as i64 / 1000 + 8 {
        eprintln!(
            "WARN: dropping the leaf released little ({} MB); the browse model may be leaking",
            post_load_rss as i64 - post_drop_rss as i64
        );
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

fn make_tempdir() -> std::io::Result<PathBuf> {
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = std::env::temp_dir().join(format!("conservatory-memcheck-{}-{}", pid, ts));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

fn read_vm_hwm_mb() -> Option<u64> {
    read_proc_status_kb("VmHWM").map(|kb| kb / 1024)
}

fn read_vm_rss_mb() -> Option<u64> {
    read_proc_status_kb("VmRSS").map(|kb| kb / 1024)
}

fn read_proc_status_kb(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field)
            && let Some(rest) = rest.strip_prefix(':')
        {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if let Some(kb_str) = parts.first()
                && let Ok(kb) = kb_str.parse::<u64>()
            {
                return Some(kb);
            }
        }
    }
    None
}
