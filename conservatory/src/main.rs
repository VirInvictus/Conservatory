//! Conservatory GTK4/libadwaita binary. Phase 3b launches the faceted browse
//! window (spec §3.3); the player, podcasts, and audiobooks tabs follow in later
//! phases. All data logic lives in `conservatory-core`; this binary renders.

use std::path::PathBuf;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use gtk::glib;

#[cfg(feature = "audiobooks")]
mod book_query;
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
.chapter-row { padding: 3px 6px; }
.chapter-row.current-chapter { background: alpha(@accent_color, 0.16); font-weight: bold; }
.book-tile { padding: 8px; border-radius: 10px; }
.book-tile:selected { background: alpha(@accent_color, 0.18); }
.book-cover { border-radius: 6px; background: alpha(currentColor, 0.06); }
.book-accent { box-shadow: inset 0 -4px 0 alpha(currentColor, 0.25); }
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
        // The library root (Phase 10a): a CLI positional still wins (dev / tooling
        // override), else it comes from `config.toml`'s `[library] root`. With
        // neither set there is simply no library to browse, as before.
        let config = conservatory_core::config::load_default().unwrap_or_else(|e| {
            tracing::warn!("config load failed, using defaults: {e}");
            conservatory_core::Config::default()
        });
        let root = resolve_root(positionals.get(1).cloned(), &config);
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

/// Resolve the library root: a CLI positional overrides (dev / tooling), else
/// the config's `[library] root`, else `None` (no library to browse). Pure, so
/// the precedence is unit-testable without a GTK display.
fn resolve_root(
    positional: Option<PathBuf>,
    config: &conservatory_core::Config,
) -> Option<PathBuf> {
    positional.or_else(|| config.library.root.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conservatory_core::Config;

    #[test]
    fn positional_root_overrides_config() {
        let mut config = Config::default();
        config.library.root = Some(PathBuf::from("/from/config"));
        let root = resolve_root(Some(PathBuf::from("/from/cli")), &config);
        assert_eq!(root, Some(PathBuf::from("/from/cli")));
    }

    #[test]
    fn config_root_used_when_no_positional() {
        let mut config = Config::default();
        config.library.root = Some(PathBuf::from("/from/config"));
        assert_eq!(
            resolve_root(None, &config),
            Some(PathBuf::from("/from/config"))
        );
    }

    #[test]
    fn no_root_anywhere_is_none() {
        assert_eq!(resolve_root(None, &Config::default()), None);
    }
}
