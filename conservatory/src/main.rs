//! Conservatory GTK4 binary. The GUI arrives at Phase 3 (spec §17); until
//! then this is a placeholder so the workspace builds without the GTK
//! toolchain present.

/// The compile-time plugins this binary was built with (spec §2.2); the
/// Podcasts and Audiobooks tabs exist only when their feature is on. The match
/// on an empty slice (rather than `is_empty`) keeps clippy's compile-time-
/// constant lints quiet across both feature sets.
fn plugin_list() -> String {
    let plugins: &[&str] = &[
        #[cfg(feature = "podcasts")]
        "podcasts",
        #[cfg(feature = "audiobooks")]
        "audiobooks",
    ];
    match plugins {
        [] => "none (music-only build)".to_string(),
        _ => plugins.join(", "),
    }
}

fn main() {
    println!("Conservatory {}", conservatory_core::VERSION);
    println!("plugins: {}", plugin_list());
    println!("GUI not built yet (Phase 3, see spec.md §17). Use conservatory-cli.");
}
