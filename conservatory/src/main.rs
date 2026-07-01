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
mod statusbar;
mod ui;
mod viz;

const APP_ID: &str = "org.virinvictus.Conservatory";

/// The app's visual identity (Phase 12a): the Kanagawa Dragon palette mapped onto
/// libadwaita's named colours (the source-of-truth mapping is `docs/theme.md`),
/// plus the structural Columns UI compaction and the lifted album-cover cards.
/// The dense `.data-table` padding is deliberate (the deadbeef model); the covers
/// get an Amberol-style drop shadow, and the per-album accent ring is layered on
/// at runtime via `ui/accent.rs` (`.cover-acc-RRGGBB`).
const CSS: &str = "\
@define-color window_bg_color #181616;
@define-color window_fg_color #c5c9c5;
@define-color view_bg_color #12120f;
@define-color view_fg_color #c5c9c5;
@define-color headerbar_bg_color #1d1c19;
@define-color headerbar_fg_color #c5c9c5;
@define-color sidebar_bg_color #12120f;
@define-color sidebar_fg_color #c5c9c5;
@define-color secondary_sidebar_bg_color #181616;
@define-color card_bg_color #1d1c19;
@define-color card_fg_color #c5c9c5;
@define-color popover_bg_color #1d1c19;
@define-color popover_fg_color #c5c9c5;
@define-color dialog_bg_color #1d1c19;
@define-color accent_color #c4746e;
@define-color accent_bg_color #c4746e;
@define-color accent_fg_color #12120f;
@define-color warning_color #c4b28a;
@define-color error_color #c4746e;
@define-color success_color #87a987;

/* Typography (Phase 13d): bundled OFL fonts registered via fontconfig at startup
   so nothing is assumed installed. Inter body, Fraunces headers, IBM Plex Mono
   technical fields; each rule keeps a generic-family fallback so a missing font
   degrades to a sane default rather than breaking text. */
window, popover, dropdown, tooltip { font-family: \"Inter\", \"Adwaita Sans\", sans-serif; }
.title-1, .title-2, .title-3, .title-4, .large-title, .heading { font-family: \"Fraunces\", serif; }
.tech { font-family: \"IBM Plex Mono\", monospace; }

columnview.data-table > listview > row > cell { padding-top: 1px; padding-bottom: 1px; }
columnview.data-table > listview > row { transition: background-color 150ms ease; }
columnview.data-table > listview > row:hover { background: alpha(currentColor, 0.04); }
columnview > header > button { padding-top: 2px; padding-bottom: 2px; min-height: 0; transition: background-color 150ms ease; }
columnview > header > button:hover { background: alpha(currentColor, 0.08); }
.numeric { font-feature-settings: \"tnum\"; }
.rating-stars { color: @accent_color; }
.filter-warn text { background-color: alpha(@warning_color, 0.20); }
selection { background-color: alpha(@accent_color, 0.35); color: @window_fg_color; }
*:focus-visible { outline-color: @accent_color; outline-width: 2px; outline-offset: 1px; }
scrollbar slider { min-width: 8px; min-height: 8px; border-radius: 8px; }
.now-bar { padding: 6px 12px; border-top: 1px solid alpha(currentColor, 0.10); }
.now-bar-cover { border-radius: 6px; box-shadow: 0 1px 5px rgba(0,0,0,0.40); background: alpha(currentColor, 0.06); }
.queue-row { padding: 4px 8px; }
.queue-row.playing { background: alpha(@accent_color, 0.16); }
.queue-list { border-left: 1px solid alpha(currentColor, 0.12); }
.chapter-row { padding: 3px 6px; border-radius: 5px; transition: background-color 150ms ease; }
.chapter-row:hover { background: alpha(currentColor, 0.06); }
.chapter-row.current-chapter { background: alpha(@accent_color, 0.16); font-weight: bold; }
.sleep-menu-row { transition: background-color 150ms ease; }
.sleep-menu-row:hover { background: alpha(currentColor, 0.08); }
.book-tile { padding: 8px; border-radius: 10px; }
.book-tile:selected { background: alpha(@accent_color, 0.18); }
.cover-art { border-radius: 10px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 4px 14px rgba(0,0,0,0.28); background: alpha(currentColor, 0.05); }
.cover-thumb { border-radius: 4px; background: alpha(currentColor, 0.06); }
.book-cover { border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 4px 12px rgba(0,0,0,0.26); background: alpha(currentColor, 0.06); }
.inspector-cover { border-radius: 10px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 6px 18px rgba(0,0,0,0.30); background: alpha(currentColor, 0.06); }
.now-playing-cover { border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.30), 0 6px 18px rgba(0,0,0,0.30); background: alpha(currentColor, 0.06); }
.now-playing-drawer { border-top: 1px solid alpha(currentColor, 0.10); }
/* The chip floating over the full-bleed spectrum: a translucent scrim keeps the
   title / artist legible against the bars behind it. */
.now-playing-info { background-color: alpha(@window_bg_color, 0.72); border-radius: 10px; padding: 8px 14px; box-shadow: 0 2px 10px rgba(0,0,0,0.35); }
.spectrum { background: alpha(currentColor, 0.03); }
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

/// Register the bundled fonts (Phase 13d) so the typography (Inter body, Fraunces
/// headers, IBM Plex Mono technical fields) renders without assuming any font is
/// installed on the host (spec §7.2.9). Pango's `add_font_file` would be cleaner
/// but needs pango v1_56 and the workspace is on 0.20, so we go through fontconfig
/// the way the spec names: write a tiny config that includes the system config and
/// adds our bundled font dir, then point fontconfig at it via `FONTCONFIG_FILE`.
/// Runs before GTK lays out any text (fontconfig is read lazily on first layout),
/// hence the call is the first thing in `main()`. Every failure is a soft fallback
/// to the generic families in the CSS, so text never breaks.
fn register_bundled_fonts() {
    // Respect a user-provided fontconfig setup rather than clobbering it.
    if std::env::var_os("FONTCONFIG_FILE").is_some() {
        return;
    }
    let Some(font_dir) = bundled_font_dir() else {
        tracing::warn!("bundled font directory not found; using system font fallbacks");
        return;
    };
    let mut conf_path = glib::user_cache_dir();
    conf_path.push("conservatory");
    if let Err(e) = std::fs::create_dir_all(&conf_path) {
        tracing::warn!("could not create font cache dir: {e}");
        return;
    }
    conf_path.push("fonts.conf");
    // Include the system config (which transitively pulls in the user's own
    // fontconfig and ~/.local/share/fonts) so we add to it rather than replace it.
    let conf = format!(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <fontconfig>\n  \
         <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n  \
         <dir>{}</dir>\n\
         </fontconfig>\n",
        font_dir.display()
    );
    if let Err(e) = std::fs::write(&conf_path, conf) {
        tracing::warn!("could not write bundled font config: {e}");
        return;
    }
    // SAFETY: the very first thing main() does, before the app builds or any other
    // thread spawns, so there is no concurrent env access (the 2024-edition rule).
    unsafe { std::env::set_var("FONTCONFIG_FILE", &conf_path) };
    tracing::debug!("registered bundled fonts from {}", font_dir.display());
}

/// Locate the bundled `fonts/` directory across the dev and installed layouts.
/// Picks the first candidate that exists and actually holds a font file.
fn bundled_font_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("CONSERVATORY_FONT_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin_dir) = exe.parent()
    {
        if let Some(prefix) = bin_dir.parent() {
            candidates.push(prefix.join("share/conservatory/fonts"));
        }
        candidates.push(bin_dir.join("fonts"));
    }
    candidates.push(PathBuf::from("/app/share/fonts")); // Flatpak
    // Dev: the repo's data/fonts, relative to this crate.
    candidates.push(PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../data/fonts"
    )));

    candidates.into_iter().find(|dir| {
        std::fs::read_dir(dir)
            .map(|mut entries| {
                entries.any(|e| {
                    e.ok().is_some_and(|e| {
                        matches!(
                            e.path().extension().and_then(|s| s.to_str()),
                            Some("ttf" | "otf")
                        )
                    })
                })
            })
            .unwrap_or(false)
    })
}

fn main() -> glib::ExitCode {
    init_tracing();
    register_bundled_fonts();
    conservatory_core::debug::log_memory("startup");

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| {
        // Kanagawa Dragon is a dark palette (Phase 12a); force the dark scheme so
        // the `@define-color` overrides land on the dark variant, not the light.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        load_css();
    });

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

    let debug = std::env::args().any(|a| a == "--debug" || a == "-d");
    if debug {
        // The one switch for the deep hooks (SQL profiler, memory sampler);
        // RUST_LOG still narrows the output (Phase 14).
        conservatory_core::debug::set_enabled(true);
    }
    // In debug mode our crates plus the conservatory::{sql,io,net,mem} channels
    // (the `conservatory` directive covers the `conservatory::*` targets) go to
    // debug; everything else stays at info. RUST_LOG overrides either way.
    let default = if debug {
        "info,conservatory=debug,conservatory_core=debug,conservatory_podcasts=debug,conservatory_audiobooks=debug"
    } else {
        "info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
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
