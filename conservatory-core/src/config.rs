//! Persisted user configuration (spec §10, Phase 10a).
//!
//! Conservatory's first `config.toml`, at `$XDG_CONFIG_HOME/conservatory/`
//! (falling back to `~/.config`). It owns the app/library-level settings that
//! are not otherwise persisted: the library root, the import/path-template
//! defaults, the genre fallback, the podcast/audiobook subdirs and book
//! defaults, and the browse facet-pane layout.
//!
//! It deliberately does **not** own the audio engine state (ReplayGain, EQ,
//! DSP, output): that stays in the SQLite singletons (`audio_state`,
//! `eq_state`), which the Sound dialog mutates live and the engine reads. The
//! spec §10 `[playback]`/`[audio]` blocks are DB-canonical for now; binding
//! them to the file is deferred (spec §10 note).
//!
//! Every section uses `#[serde(default)]`, so a partial or absent file loads to
//! the documented defaults and a round-trip is lossless. Loading a missing file
//! yields `Config::default()`; only a present-but-malformed file is an error.
//! The module is glib-free so it stays CLI-testable (spec §2.2).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};
use crate::mover::MoveMode;
use crate::path_template::{DEFAULT_AUDIOBOOK_TEMPLATE, DEFAULT_MUSIC_TEMPLATE};

/// The whole `config.toml`. Sections default independently.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub library: LibraryConfig,
    pub genre: GenreConfig,
    pub podcasts: PodcastsConfig,
    pub audiobooks: AudiobooksConfig,
    pub browse: BrowseConfig,
    pub sections: SectionsConfig,
    pub scrobble: ScrobbleConfig,
}

/// `[sections]`: which media tabs are enabled (Phase 16e). Disabling a section
/// hides its tab and, at the next launch, skips building it entirely: no page is
/// added and its subsystem is never started (the lazy `::map` init never runs), so
/// a disabled section costs nothing at runtime. This is a runtime toggle over what
/// is *compiled in*, distinct from the compile-time plugin features (§2.2): those
/// decide what is in the binary, this decides what a given launch shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionsConfig {
    pub music: bool,
    pub podcasts: bool,
    pub audiobooks: bool,
}

impl Default for SectionsConfig {
    fn default() -> Self {
        Self {
            music: true,
            podcasts: true,
            audiobooks: true,
        }
    }
}

/// `[library]`: where the managed tree lives and how music is filed into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    /// The managed library root. `None` until the user sets one (the GTK binary
    /// then has no library to browse, exactly as today with no CLI arg).
    pub root: Option<PathBuf>,
    /// The music save-to-disk template (spec §5.7).
    pub path_template: String,
    /// Copy (leave originals) or move (consume them) on import.
    pub import_mode: ImportMode,
    /// Write curated metadata back into files on edit (spec §5.5).
    pub embed_tags_on_edit: bool,
}

/// `[genre]`: the shelf-genre fallback (spec §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GenreConfig {
    /// The filed-under value for an album with no resolvable genre.
    pub default_unknown: String,
}

/// `[podcasts]`: subscription-wide defaults (spec §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PodcastsConfig {
    /// The subdir of the library root podcast downloads land under.
    pub library_subdir: String,
    /// Cap on simultaneous episode downloads.
    pub max_concurrent_downloads: u32,
}

/// `[audiobooks]`: book import defaults (spec §3.8). Per-book overrides live in
/// `book_playback`; these are the defaults a new book inherits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudiobooksConfig {
    pub library_subdir: String,
    /// The audiobook save-to-disk template (spec §5.7).
    pub path_template: String,
    pub default_speed: f64,
    pub smart_speed: bool,
    pub voice_boost: bool,
}

/// `[browse]`: the facet-pane layout (spec §3.2). The pane field expressions in
/// left-to-right order; 1 to 5 panes. The configurable builder is Phase 10c;
/// 10a just persists and round-trips the list, seeded with the current default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowseConfig {
    pub panes: Vec<String>,
    /// The leaf columns, left-to-right, by catalog id (Phase 18b). Default is the
    /// pre-18b fixed set, so an unconfigured launch is visually unchanged. Unknown
    /// / duplicate ids are skipped when the leaf is built (the forgiving idiom).
    pub columns: Vec<String>,
}

/// The pre-18b fixed leaf column order (the [`BrowseConfig::columns`] default and
/// the catalog's canonical order). Shared so the config default and the GUI editor
/// agree on the baseline.
pub fn default_columns() -> Vec<String> {
    [
        "cover", "glyph", "artist", "album", "genre", "title", "duration", "rating",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `[scrobble]`: optional, off-by-default listening-history sync (spec §14
/// carve-out, Phase 9). The user token is **not** stored here; it lives in
/// libsecret, keyed per service (`ScrobbleService::token_ref`). With `enabled =
/// false` (the default) the whole subsystem is inert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrobbleConfig {
    pub enabled: bool,
    /// `"listenbrainz"` (default) | `"lastfm"`. Parsed forgivingly by
    /// `ScrobbleService::parse`; an unknown value degrades to ListenBrainz.
    pub service: String,
    /// Phase 9c: the Last.fm application key + shared secret. Deliberately
    /// config-backed (not baked into the binary), so each user registers their
    /// own API account. Absent (the default) means Last.fm cannot be used until
    /// the user fills these in; the per-user session key still lives in
    /// libsecret, never here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lastfm_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lastfm_api_secret: Option<String>,
}

impl Default for ScrobbleConfig {
    fn default() -> Self {
        // Spec §10 [scrobble] defaults: off, ListenBrainz.
        Self {
            enabled: false,
            service: "listenbrainz".to_string(),
            lastfm_api_key: None,
            lastfm_api_secret: None,
        }
    }
}

/// Import disposition, the serde-facing `"copy"` / `"move"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    Copy,
    Move,
}

impl ImportMode {
    /// Map to the mover's disposition (the engine-side type).
    pub fn to_move_mode(self) -> MoveMode {
        match self {
            ImportMode::Copy => MoveMode::Copy,
            ImportMode::Move => MoveMode::Move,
        }
    }
}

impl Default for LibraryConfig {
    fn default() -> Self {
        // Spec §10 [library] defaults.
        Self {
            root: None,
            path_template: DEFAULT_MUSIC_TEMPLATE.to_string(),
            import_mode: ImportMode::Copy,
            embed_tags_on_edit: true,
        }
    }
}

impl Default for GenreConfig {
    fn default() -> Self {
        Self {
            default_unknown: "Unknown".to_string(),
        }
    }
}

impl Default for PodcastsConfig {
    fn default() -> Self {
        Self {
            library_subdir: "Podcasts".to_string(),
            max_concurrent_downloads: 3,
        }
    }
}

impl Default for AudiobooksConfig {
    fn default() -> Self {
        Self {
            library_subdir: "Audiobooks".to_string(),
            path_template: DEFAULT_AUDIOBOOK_TEMPLATE.to_string(),
            default_speed: 1.0,
            smart_speed: true,
            voice_boost: false,
        }
    }
}

impl Default for BrowseConfig {
    fn default() -> Self {
        // The current hard-coded browse hierarchy (Phase 3b), the 10c seed.
        Self {
            panes: vec![
                "genre".to_string(),
                "albumartist".to_string(),
                "album".to_string(),
            ],
            columns: default_columns(),
        }
    }
}

/// The default config-file location: `$XDG_CONFIG_HOME/conservatory/config.toml`,
/// or `~/.config/conservatory/config.toml` when `XDG_CONFIG_HOME` is unset.
/// Hand-rolled (no glib) so core stays CLI-testable; the GTK binary's
/// `default_db_path` resolves the data dir the same way via `glib::user_data_dir`.
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("conservatory").join("config.toml")
}

/// Load the config at `path`. A missing file is not an error: it yields the
/// documented defaults (the app is usable on first run). A present-but-malformed
/// file is a real error so a typo is surfaced, not silently reset.
pub fn load(path: &Path) -> Result<Config> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e.into()),
    };
    toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
}

/// Render `config` as pretty TOML (the on-disk form), for `save` and for the
/// CLI `config show` verb so the toml dependency stays in core.
pub fn to_toml_string(config: &Config) -> Result<String> {
    toml::to_string_pretty(config).map_err(|e| Error::Config(format!("serialize: {e}")))
}

/// Serialize `config` to `path` (creating the parent dir), overwriting it.
pub fn save(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, to_toml_string(config)?)?;
    Ok(())
}

/// Load from the default [`config_path`].
pub fn load_default() -> Result<Config> {
    load(&config_path())
}

/// Save to the default [`config_path`].
pub fn save_default(config: &Config) -> Result<()> {
    save(&config_path(), config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn partial_file_fills_the_rest_from_defaults() {
        let text = r#"
            [library]
            root = "/music"
        "#;
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.library.root, Some(PathBuf::from("/music")));
        // Untouched fields are the documented defaults.
        assert_eq!(config.library.path_template, DEFAULT_MUSIC_TEMPLATE);
        assert_eq!(config.library.import_mode, ImportMode::Copy);
        assert!(config.library.embed_tags_on_edit);
        assert_eq!(config.genre.default_unknown, "Unknown");
        assert_eq!(config.podcasts.max_concurrent_downloads, 3);
        assert_eq!(config.browse.panes, Config::default().browse.panes);
        // The scrobble section defaults to off / ListenBrainz.
        assert!(!config.scrobble.enabled);
        assert_eq!(config.scrobble.service, "listenbrainz");
    }

    #[test]
    fn scrobble_section_round_trips() {
        let text = r#"
            [scrobble]
            enabled = true
            service = "lastfm"
            lastfm_api_key = "deadbeef"
            lastfm_api_secret = "cafef00d"
        "#;
        let config: Config = toml::from_str(text).unwrap();
        assert!(config.scrobble.enabled);
        assert_eq!(config.scrobble.service, "lastfm");
        assert_eq!(config.scrobble.lastfm_api_key.as_deref(), Some("deadbeef"));
        assert_eq!(
            config.scrobble.lastfm_api_secret.as_deref(),
            Some("cafef00d")
        );
        // And a full round-trip is lossless.
        let back: Config = toml::from_str(&to_toml_string(&config).unwrap()).unwrap();
        assert_eq!(config, back);
        // Absent Last.fm creds (the default) stay absent, not serialized as empty.
        assert!(Config::default().scrobble.lastfm_api_key.is_none());
    }

    #[test]
    fn explicit_fields_parse() {
        let text = r#"
            [library]
            root = "/srv/audio"
            path_template = "{albumartist}/{album}/{title}"
            import_mode = "move"
            embed_tags_on_edit = false
        "#;
        let config: Config = toml::from_str(text).unwrap();
        assert_eq!(config.library.root, Some(PathBuf::from("/srv/audio")));
        assert_eq!(
            config.library.path_template,
            "{albumartist}/{album}/{title}"
        );
        assert_eq!(config.library.import_mode, ImportMode::Move);
        assert_eq!(config.library.import_mode.to_move_mode(), MoveMode::Move);
        assert!(!config.library.embed_tags_on_edit);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let path = Path::new("/nonexistent/conservatory/config.toml");
        assert_eq!(load(path).unwrap(), Config::default());
    }

    #[test]
    fn malformed_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not valid = toml ===").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.library.root = Some(PathBuf::from("/music"));
        config.library.import_mode = ImportMode::Move;
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), config);
    }

    #[test]
    fn config_path_honours_xdg_config_home() {
        // SAFETY: single-threaded test; we set then restore the var.
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg-test") };
        assert_eq!(
            config_path(),
            PathBuf::from("/tmp/xdg-test/conservatory/config.toml")
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
    }
}
