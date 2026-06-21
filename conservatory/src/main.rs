//! Conservatory GTK4/libadwaita binary. Phase 3b launches the faceted browse
//! window (spec §3.3); the player, podcasts, and audiobooks tabs follow in later
//! phases. All data logic lives in `conservatory-core`; this binary renders.

use std::path::PathBuf;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use gtk::glib;

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
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| load_css());

    app.connect_activate(|app| {
        let db = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .or_else(default_db_path);
        let window = ui::window::ConservatoryWindow::new(app, db);
        window.present();
    });

    // Pass only argv0 to GApplication so a positional DB path is not treated as a
    // file to "open"; the activate handler reads the real args itself.
    let argv0 = std::env::args().next().unwrap_or_default();
    app.run_with_args(&[argv0])
}

/// The default library location (XDG data dir). Browse is empty if it's absent.
fn default_db_path() -> Option<PathBuf> {
    let mut path = glib::user_data_dir();
    path.push("conservatory");
    path.push("library.db");
    Some(path)
}
