//! Conservatory GTK4/libadwaita binary. Phase 3b launches the faceted browse
//! window (spec §3.3); the player, podcasts, and audiobooks tabs follow in later
//! phases. All data logic lives in `conservatory-core`; this binary renders.

use std::path::PathBuf;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use gtk::glib;

mod playqueue;
mod query;
mod ui;

const APP_ID: &str = "org.virinvictus.Conservatory";

/// Compact the Columns UI tables toward the dense deadbeef look (GTK's default
/// list padding is roomy). Theme/colour follow the system; Kanagawa Dragon is a
/// later pass.
const CSS: &str = "\
columnview.data-table > listview > row > cell { padding-top: 1px; padding-bottom: 1px; }
columnview.data-table > listview > row:hover { background: alpha(currentColor, 0.04); }
columnview > header > button { padding-top: 2px; padding-bottom: 2px; min-height: 0; }
.numeric { font-feature-settings: \"tnum\"; }
.rating-stars { color: @accent_color; }
.filter-warn text { background-color: alpha(@warning_color, 0.20); }
.now-bar { padding: 4px 10px; border-top: 1px solid alpha(currentColor, 0.12); }
.queue-row { padding: 4px 8px; }
.queue-row.playing { background: alpha(@accent_color, 0.16); }
.queue-list { border-left: 1px solid alpha(currentColor, 0.12); }
";

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn main() -> glib::ExitCode {
    init_tracing();

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| load_css());

    app.connect_activate(|app| {
        // Positional args are the DB path then the library root; flags (`--debug`)
        // are skipped so they don't get read as a path.
        let positionals: Vec<PathBuf> = std::env::args()
            .skip(1)
            .filter(|a| !a.starts_with('-'))
            .map(PathBuf::from)
            .collect();
        let db = positionals.first().cloned().or_else(default_db_path);
        // Optional library root (Phase 4b-ii-a): resolves relative track paths for
        // playback. Phase 10 config will source this instead of an argument.
        let root = positionals.get(1).cloned();
        let window = ui::window::ConservatoryWindow::new(app, db, root);
        window.present();
    });

    // Pass only argv0 to GApplication so a positional DB path is not treated as a
    // file to "open"; the activate handler reads the real args itself.
    let argv0 = std::env::args().next().unwrap_or_default();
    app.run_with_args(&[argv0])
}

/// Install the tracing subscriber (v0.0.38). Without one, the tracing calls
/// wired through the engine / worker / podcast fetch are silent no-ops, which is
/// why the player appeared to "do everything silently". Defaults to `info` (warn
/// and error always visible, normal use un-spammy); the `--debug` flag raises our
/// own crates to `debug` (the player load / advance / buffering transitions);
/// `RUST_LOG` overrides either. Mirrors the Atrium / Viaduct binaries.
fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};

    let default = if std::env::args().any(|a| a == "--debug" || a == "-d") {
        "info,conservatory=debug,conservatory_core=debug,conservatory_podcasts=debug"
    } else {
        "info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .compact()
        .init();
}

/// The default library location (XDG data dir). Browse is empty if it's absent.
fn default_db_path() -> Option<PathBuf> {
    let mut path = glib::user_data_dir();
    path.push("conservatory");
    path.push("library.db");
    Some(path)
}
