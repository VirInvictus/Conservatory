//! The faceted browse window (Phase 3b/3c). An `adw::ApplicationWindow` subclass
//! (programmatic children, no `.ui`) holding the read pool, the single-writer
//! worker, the facet panes, the filter bar, the Perspectives sidebar, and the
//! leaf list. Phase 3c adds the always-on filter bar (spec §3.4: the panes
//! filter, the grammar searches, they intersect on the leaf) and Perspectives:
//! named saved searches in the sidebar, saved through the worker and reloaded by
//! re-parsing their text.
//!
//! Phase 6b-i turns the single-view window into the multi-view shell of spec
//! §2.3: the music browse is one page of an `AdwViewStack`, with a header
//! `AdwViewSwitcher` and a Podcasts plugin page (feature-gated, lazy on `::map`,
//! empty until 6b-ii). An `AdwBreakpoint` collapses the switcher to a bottom
//! `AdwViewSwitcherBar` beneath the persistent Now-bar on narrow widths. A
//! music-only build keeps a single-page stack with no switcher chrome.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;

use conservatory_core::db::{
    EQ_CENTRES, EqState, FacetField, FacetFilter, MediaKind, Perspective, Playlist, PlaylistKind,
    PlaylistOrder, ReadPool, ResamplerQuality, WorkerHandle, facet_rows, get_album, get_artist,
    get_audio_state, get_eq_preset, get_eq_state, get_track, get_tracks, list_eq_presets,
    list_perspectives, list_playlists, load_queue_display, read_playback_state, show_settings_map,
    spawn_worker, static_playlist_track_ids, track_render_rows, writeback_rows,
};
use conservatory_core::mover::{self, MoveKind, MoveMode, MoveOp, organize_ops};
use conservatory_core::{
    Assignment, Config, ImportMode, PlaybackConfig, PlayerHandle, TagWrite, any_path_affecting,
    build_album_edit, build_track_edit, genres_assignment, parse_assignment, write_track_tags,
};

use crate::playqueue::{MixedQueueRow, build_mixed_queue, build_play_queue, fmt_position};
use crate::query::{materialize_smart, query_leaf};
use crate::ui::coalescing::CoalescingQueue;
use crate::ui::facet_pane::{FacetPane, build_pane};
use crate::ui::fields::{collect_assignments, inspector_fields};
use crate::ui::inspector::{Inspector, build_inspector};
use crate::ui::now_bar::{NowBar, build_now_bar};
use crate::ui::now_playing_panel::{NowPlayingPanel, build_now_playing_panel};
use crate::ui::objects::TrackRow;
use crate::ui::queue_panel::{QueuePanel, build_queue_panel};
use crate::ui::sound;
use crate::ui::track_list::{Leaf, build_leaf};

type Coalescer = CoalescingQueue<usize, Box<dyn FnMut(Vec<usize>)>>;
type FilterCoalescer = CoalescingQueue<(), Box<dyn FnMut(Vec<()>)>>;

/// The queue-drawer keyboard actions (spec §3.1: every gesture has a key).
#[derive(Clone, Copy)]
enum QueueKey {
    MoveUp,
    MoveDown,
    Remove,
    Clear,
}

/// A facet-pane context-menu verb (Phase 16a): all three act on the pane's
/// narrowed track set (the whole leaf after the facet selection cascades).
#[derive(Clone, Copy)]
enum FacetVerb {
    Play,
    PlayNext,
    Queue,
}

mod imp {
    use super::*;
    use std::cell::{Cell, OnceCell, RefCell};
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct ConservatoryWindow {
        pub pool: OnceCell<ReadPool>,
        pub panes: RefCell<Vec<FacetPane>>,
        pub leaf: OnceCell<Leaf>,
        pub coalescer: OnceCell<Coalescer>,
        pub filter_entry: OnceCell<gtk::SearchEntry>,
        pub filter_coalescer: OnceCell<FilterCoalescer>,
        pub sidebar_list: OnceCell<gtk::ListBox>,
        pub perspectives: RefCell<Vec<Perspective>>,
        pub suppress: Cell<bool>,
        // The volume to restore on un-mute (Phase 13e-ii, Ctrl+0); `None` when not
        // muted. Stored here rather than in the engine so the toggle is a pure GUI
        // convenience over the existing `set_volume`.
        pub pre_mute_volume: Cell<Option<i64>>,
        // Worker before runtime: on drop the handle closes the channel (the serve
        // loop exits cleanly) before the runtime it runs on is torn down.
        pub worker: OnceCell<WorkerHandle>,
        pub runtime: OnceCell<tokio::runtime::Runtime>,
        // Playback (Phase 4b-ii-a). `library_root` resolves the relative track
        // paths; `now_labels` maps the playing queue's track ids to title/artist
        // for the Now-bar; `last_shown` is the id the bar currently displays so
        // labels re-render only on change; `poll_source` is the 250 ms snapshot
        // timer, removed on close before the player shuts down.
        pub player: OnceCell<PlayerHandle>,
        pub library_root: OnceCell<PathBuf>,
        pub now_bar: OnceCell<NowBar>,
        pub poll_source: RefCell<Option<glib::SourceId>>,
        pub now_labels: RefCell<HashMap<i64, (String, String)>>,
        pub last_shown: Cell<Option<i64>>,
        // The queue slot currently displayed. Item-change detection keys on this
        // alongside `last_shown`: `track_id` alone is ambiguous (a track and an
        // episode can share an id), which left the drawer stale between songs.
        pub last_index: Cell<Option<usize>>,
        // The queue drawer (Phase 4b-ii-b). `queue_current` is the playing
        // position, shared with the panel's row factory for the highlight; the
        // window updates it from the snapshot and rebuilds the drawer.
        pub queue_panel: OnceCell<QueuePanel>,
        pub queue_current: OnceCell<Rc<Cell<Option<i64>>>>,
        // The bottom Now Playing drawer (v0.0.38): current-item metadata, the
        // future visualizer home. Refreshed on track change, toggled by the
        // Now-bar cover/title click, a header button, or Ctrl+I.
        pub now_playing: OnceCell<NowPlayingPanel>,
        pub inspector: OnceCell<Inspector>,
        // The status bar footer (Phase 11b, spec §3.2): `status_left` is the
        // playing track's technical line, `status_right` the active view's
        // (or selection's) "N tracks · playtime" aggregate. `last_play_state`
        // caches the (playing track id, paused) last applied to the leaf glyph
        // column, so the poll only walks the store when playback actually moves.
        pub status_left: OnceCell<gtk::Label>,
        pub status_right: OnceCell<gtk::Label>,
        pub last_play_state: Cell<(Option<i64>, bool)>,
        // The stateful "stop after current" menu action (Phase 11d), held so the
        // poll can sync its checked state when the engine disarms at the boundary.
        pub stop_action: OnceCell<gio::SimpleAction>,
        // The playing track's static technical fields (format / sample-rate /
        // bitrate), cached on track change so the per-tick tech-line refresh
        // (which folds in the live mpv channel count) needs no DB read. `None`s
        // when nothing (or a non-track) is playing.
        pub tech_static: RefCell<(Option<String>, Option<i32>, Option<i32>)>,
        // The active view's (count, total seconds), cached on leaf populate so a
        // selection change can show the selection total without re-summing the
        // whole view.
        pub view_total: Cell<(usize, f64)>,
        // The top-level view stack (Phase 6b-i): Music first, plus the
        // feature-gated Podcasts/Audiobooks plugin pages. `Alt+1/2/3` switch
        // its visible child by name.
        pub view_stack: OnceCell<adw::ViewStack>,
        // The toast host (Phase 13b): brief, non-modal confirmations for actions
        // that would otherwise complete silently or behind a modal "Done" dialog.
        pub toast_overlay: OnceCell<adw::ToastOverlay>,
        // The leaf right-click context menu (Phase 16a): a shared PopoverMenu
        // parented to the leaf ColumnView, and the row position last right-clicked
        // (so the "Play" verb starts from that row, not the selection anchor).
        pub track_menu: OnceCell<gtk::PopoverMenu>,
        pub context_row: Cell<Option<u32>>,
        // The facet-pane right-click menu (Phase 16a): a shared PopoverMenu
        // re-parented to the clicked pane's ColumnView (the panes differ), and the
        // pane index last right-clicked (its verbs act on that pane's narrowing).
        pub facet_menu: OnceCell<gtk::PopoverMenu>,
        pub context_pane: Cell<Option<usize>>,
        // The queue-drawer right-click menu (Phase 16a): parented to the (single,
        // stable) queue ListView, so no per-click re-parenting is needed.
        pub queue_menu: OnceCell<gtk::PopoverMenu>,
        // The Playlists sidebar section (Phase 16d-ii): the list widget and the
        // cached rows (parallel to `sidebar_list` / `perspectives`), plus the
        // dynamic "Add to Playlist" submenu of the track context menu, repopulated
        // from the static playlists whenever the playlist set changes.
        pub playlist_list: OnceCell<gtk::ListBox>,
        pub playlists: RefCell<Vec<conservatory_core::db::Playlist>>,
        pub add_to_playlist_menu: OnceCell<gio::Menu>,
        // Music-only header controls (Phase 16f): stored so they can be hidden on
        // the Podcasts / Audiobooks tabs, where selection-editing and the track
        // properties inspector do not apply.
        pub header_edit_group: OnceCell<gtk::Box>,
        pub header_props_btn: OnceCell<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConservatoryWindow {
        const NAME: &'static str = "ConservatoryWindow";
        type Type = super::ConservatoryWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for ConservatoryWindow {}
    impl WidgetImpl for ConservatoryWindow {}
    impl WindowImpl for ConservatoryWindow {}
    impl ApplicationWindowImpl for ConservatoryWindow {}
    impl AdwApplicationWindowImpl for ConservatoryWindow {}
}

glib::wrapper! {
    pub struct ConservatoryWindow(ObjectSubclass<imp::ConservatoryWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible,
                    gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root,
                    gtk::ShortcutManager;
}

/// The filter entry's resting tooltip (16.5b): a grammar primer, swapped for
/// the parser's live warnings while a query degrades (`refresh_leaf`).
const FILTER_GRAMMAR_TIP: &str = "Search grammar: field:value with AND / OR / NOT.\n\
    Fields: artist: album: title: genre: shelfgenre: year: added: rating: bitrate: \
    duration: format: is:played is:starred is:queued vl:Perspective\n\
    Examples: genre:jazz rating:>=4 · year:1990..1999 · artist:~^aphex · \
    ambient NOT genre:rock sort:-added\n\
    Bare text searches title, artist, and album.";

/// One sidebar row: a left-aligned, ellipsized name label. The tooltip carries
/// the full name, since the narrow sidebar ellipsizes long ones (16.5b).
fn perspective_row(name: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&label));
    row.set_tooltip_text(Some(name));
    row
}

/// A Playlists-sidebar row (Phase 16d-ii): a kind icon (static list vs smart
/// query) beside the name.
fn playlist_row(name: &str, is_smart: bool) -> gtk::ListBoxRow {
    let icon = gtk::Image::from_icon_name(if is_smart {
        "system-search-symbolic"
    } else {
        "view-list-symbolic"
    });
    icon.add_css_class("dim-label");
    let label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bx.set_margin_top(6);
    bx.set_margin_bottom(6);
    bx.set_margin_start(12);
    bx.set_margin_end(12);
    bx.append(&icon);
    bx.append(&label);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&bx));
    row.set_tooltip_text(Some(name));
    row
}

impl ConservatoryWindow {
    pub fn new(
        app: &adw::Application,
        db_path: Option<PathBuf>,
        library_root: Option<PathBuf>,
    ) -> Self {
        let win: Self = glib::Object::builder().property("application", app).build();
        win.set_title(Some("Conservatory"));
        win.set_default_size(1100, 700);
        if let Some(root) = library_root {
            let _ = win.imp().library_root.set(root);
        }
        win.build_contents(db_path);
        win
    }

    fn build_contents(&self, db_path: Option<PathBuf>) {
        let imp = self.imp();

        if let Some(path) = db_path.filter(|p| p.exists()) {
            // The single-writer worker comes up first: spawning it runs the
            // migrations (adding the perspectives table), so the read pool then
            // opens onto the migrated schema. The runtime drives the worker's
            // blocking task and the brief block_on writes from the GTK thread.
            if let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
            {
                let spawned = {
                    let _guard = rt.enter();
                    spawn_worker(path.clone())
                };
                if let Ok(worker) = spawned {
                    let _ = imp.worker.set(worker);
                    let _ = imp.runtime.set(rt);
                    // Stand up the player engine on the same runtime (Phase
                    // 4b-ii-a). A libmpv init failure leaves the player unset and
                    // the transport inert; browse still works.
                    if let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) {
                        match conservatory_core::player::spawn(worker.clone(), rt.handle().clone())
                        {
                            Ok(player) => {
                                let _ = imp.player.set(player);
                            }
                            Err(e) => {
                                eprintln!("player engine unavailable; transport disabled: {e}")
                            }
                        }
                    }
                }
            }
            if let Ok(pool) = ReadPool::new(path, 3) {
                let _ = imp.pool.set(pool);
            }

            // Serve MPRIS2 + the suspend inhibitor on the runtime (Phase 4c-i):
            // media keys, the GNOME overlay/lock screen, and don't-suspend-while-
            // playing. Torn down with the runtime at app exit.
            if let (Some(rt), Some(player), Some(pool)) =
                (imp.runtime.get(), imp.player.get(), imp.pool.get())
            {
                // The root resolves the album cover into mpris:artUrl (Phase 5d).
                let root = imp.library_root.get().cloned().unwrap_or_default();
                rt.spawn(conservatory_core::mpris::run(
                    player.clone(),
                    pool.clone(),
                    root,
                ));
            }
        }

        // The browse panes come from `config.toml [browse].panes` (Phase 10c),
        // resolved to facets (unknown keys dropped, capped at 5, default
        // hierarchy when empty). The cascade is already N-pane generic.
        let config = conservatory_core::config::load_default().unwrap_or_default();
        let panes: Vec<FacetPane> =
            conservatory_core::db::FacetField::panes_from_config(&config.browse.panes)
                .into_iter()
                .enumerate()
                .map(|(i, field)| {
                    let weak = self.downgrade();
                    build_pane(
                        field,
                        Rc::new(move |pos, x, y, cell| {
                            if let Some(win) = weak.upgrade() {
                                win.show_facet_context_menu(i, pos, x, y, cell);
                            }
                        }),
                    )
                })
                .collect();
        let weak = self.downgrade();
        let ctx_weak = weak.clone();
        let leaf = build_leaf(
            imp.library_root.get().cloned(),
            Rc::new(move |pos, x, y, cell| {
                if let Some(win) = ctx_weak.upgrade() {
                    win.show_track_context_menu(pos, x, y, cell);
                }
            }),
            Rc::new(move |pos, rating| {
                if let Some(win) = weak.upgrade() {
                    win.set_row_rating(pos, rating);
                }
            }),
        );

        // Facet panes in a row on top; the track table below (a draggable split,
        // the deadbeef-cui layout).
        let facet_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        facet_row.set_hexpand(true);
        for (i, pane) in panes.iter().enumerate() {
            if i > 0 {
                facet_row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
            }
            facet_row.append(&pane.view);
        }

        let split = gtk::Paned::new(gtk::Orientation::Vertical);
        split.set_start_child(Some(&facet_row));
        split.set_end_child(Some(&leaf.stack));
        split.set_resize_start_child(true);
        split.set_resize_end_child(true);
        split.set_position(300);
        // Fill both axes so the browse grows into space freed by a collapsing
        // side / bottom revealer (see the expand-sink note on `body`).
        split.set_hexpand(true);
        split.set_vexpand(true);

        let sidebar = self.build_sidebar();
        let body = gtk::Paned::new(gtk::Orientation::Horizontal);
        body.set_start_child(Some(&sidebar));
        body.set_end_child(Some(&split));
        body.set_resize_start_child(false);
        body.set_shrink_start_child(false);
        body.set_position(190);
        // The browse body is the expand-sink: when a right-docked revealer (queue /
        // inspector) or the bottom Now Playing drawer collapses, its freed space
        // must flow back here, not sit empty. The whole container chain up to the
        // AdwViewStack page must carry both expand flags or the browse parks at its
        // natural size in the top-left and the freed space reads as a dead gap.
        body.set_hexpand(true);
        body.set_vexpand(true);

        // The always-on filter bar (spec §3.4); Ctrl+F focuses it, no search mode.
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Filter: genre:ambient  rating:>=4  vl:Favourites")
            .tooltip_text(FILTER_GRAMMAR_TIP)
            .hexpand(true)
            .build();
        let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        filter_bar.add_css_class("toolbar");
        filter_bar.append(&filter);

        // The persistent Now-bar transport (Phase 4b-ii-a), wired to the engine.
        let now_bar = build_now_bar(imp.player.get().cloned());

        // The slide-in queue drawer (Phase 4b-ii-b). The shared `current` cell
        // drives the playing-row highlight; a finished drag is delegated back.
        let queue_current: Rc<Cell<Option<i64>>> = Rc::new(Cell::new(None));
        let weak = self.downgrade();
        let on_reorder: Rc<dyn Fn(usize, usize)> = Rc::new(move |from, to| {
            if let Some(win) = weak.upgrade() {
                win.on_queue_reorder(from, to);
            }
        });
        let weak = self.downgrade();
        let on_queue_context: crate::ui::track_list::RowContextFn =
            Rc::new(move |pos, x, y, cell| {
                if let Some(win) = weak.upgrade() {
                    win.show_queue_context_menu(pos, x, y, cell);
                }
            });
        let queue_panel = build_queue_panel(queue_current.clone(), on_reorder, on_queue_context);

        // The right-docked track properties inspector (Phase 11a), the twin of
        // the queue drawer; collapsed until toggled.
        let inspector = build_inspector();

        // Body + the queue drawer + the inspector, side by side.
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.set_vexpand(true);
        content.set_hexpand(true);
        content.append(&body);
        content.append(&queue_panel.revealer);
        content.append(&inspector.revealer);

        // The Music view: the always-on filter bar over the body. The filter bar
        // lives *inside* the page (not as a global top bar) so it does not show
        // over the Podcasts tab (spec §2.3). This is the only layout change to
        // the music browse; its behaviour is unchanged.
        let music_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        music_page.set_hexpand(true);
        music_page.set_vexpand(true);
        music_page.append(&filter_bar);
        music_page.append(&content);

        // The top-level view stack (spec §2.2, §2.3): Music first; the Podcasts
        // (and later Audiobooks) plugin pages are added, feature-gated, below.
        // AdwViewStack does not propagate its page's expand flags, so it is set to
        // fill explicitly; otherwise the page parks at its natural size and the
        // browse cannot grow into freed revealer space.
        let stack = adw::ViewStack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        // Which sections this launch shows (Phase 16e): a runtime toggle over what
        // is compiled in. A disabled section adds no page; Music stays as the
        // fallback when nothing else is enabled, so the window is never empty.
        let podcasts_on = cfg!(feature = "podcasts") && config.sections.podcasts;
        let audiobooks_on = cfg!(feature = "audiobooks") && config.sections.audiobooks;
        let show_music = config.sections.music || (!podcasts_on && !audiobooks_on);
        if show_music {
            stack.add_titled_with_icon(
                &music_page,
                Some("music"),
                "Music",
                "folder-music-symbolic",
            );
        }

        // The bottom Now Playing drawer (v0.0.38): built here so the toolbar
        // content can stack it above the Now-bar (it slides up from the bottom).
        let now_playing = build_now_playing_panel();

        let header = adw::HeaderBar::new();

        // The header buttons grouped into clusters (Phase 13b) for visual
        // hierarchy: the panel-toggle trio (queue / props / info) reads as one
        // linked segment, the selection-edit pair (edit / embed) as another, and
        // the utility buttons (prefs / output / menu) sit apart on the far end.
        let queue_btn = gtk::Button::from_icon_name("view-list-symbolic");
        queue_btn.set_tooltip_text(Some("Show / hide the queue (Ctrl+U)"));
        let weak = self.downgrade();
        queue_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.toggle_queue();
            }
        });
        let props_btn = gtk::Button::from_icon_name("document-properties-symbolic");
        props_btn.set_tooltip_text(Some("Track properties (Ctrl+P)"));
        let weak = self.downgrade();
        props_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.toggle_inspector();
            }
        });
        let info_btn = gtk::Button::from_icon_name("dialog-information-symbolic");
        info_btn.set_tooltip_text(Some("Now Playing details (Ctrl+I)"));
        let weak = self.downgrade();
        info_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.toggle_now_playing();
            }
        });
        let panel_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        panel_group.add_css_class("linked");
        panel_group.append(&queue_btn);
        panel_group.append(&props_btn);
        panel_group.append(&info_btn);
        let _ = imp.header_props_btn.set(props_btn.clone());

        let prefs_btn = gtk::Button::from_icon_name("preferences-system-symbolic");
        prefs_btn.set_tooltip_text(Some("Preferences (Ctrl+comma)"));
        let weak = self.downgrade();
        prefs_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.open_preferences();
            }
        });
        // Utility cluster on the far right (right-to-left as packed): menu, output,
        // prefs, then the panel-toggle segment to their left.
        header.pack_end(&self.build_primary_menu());
        header.pack_end(&self.build_output_menu_button());
        header.pack_end(&prefs_btn);
        header.pack_end(&panel_group);

        let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
        edit_btn.set_tooltip_text(Some("Edit selected tracks (Ctrl+E)"));
        let weak = self.downgrade();
        edit_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.prompt_bulk_edit();
            }
        });
        let embed_btn = gtk::Button::from_icon_name("document-save-symbolic");
        embed_btn.set_tooltip_text(Some(
            "Write library metadata into the selected audio files on disk",
        ));
        let weak = self.downgrade();
        embed_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.prompt_embed_tags();
            }
        });
        let edit_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        edit_group.add_css_class("linked");
        edit_group.append(&edit_btn);
        edit_group.append(&embed_btn);
        // Both verbs act on the selection, so they start insensitive and follow
        // it (16.5b); before this they silently no-opped with nothing selected.
        edit_group.set_sensitive(false);
        header.pack_start(&edit_group);
        let _ = imp.header_edit_group.set(edit_group);

        // The toolbar content is the view stack with the Now Playing drawer
        // stacked beneath it: the drawer slides up from the bottom of the content
        // area, above the persistent Now-bar (v0.0.38).
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content_box.append(&stack);
        content_box.append(&now_playing.revealer);

        // The status bar footer (Phase 11b, spec §3.2): the playing track's
        // technical line on the left, the active view's count + playtime on the
        // right. A thin bottom bar that sits directly above the Now-bar.
        let status_left = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_start(12)
            .css_classes(["caption", "dim-label", "tech"])
            .build();
        let status_right = gtk::Label::builder()
            .xalign(1.0)
            .margin_end(12)
            .css_classes(["caption", "dim-label"])
            .build();
        let status_bar = gtk::CenterBox::builder()
            .margin_top(2)
            .margin_bottom(2)
            .css_classes(["status-bar"])
            .build();
        status_bar.set_start_widget(Some(&status_left));
        status_bar.set_end_widget(Some(&status_right));

        // The toast host (Phase 13b) wraps the main content area, so confirmations
        // float at the bottom of the browse, above the status / Now-bar chrome.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_box));

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&toast_overlay));
        // The status bar sits above the Now-bar, which is the stable innermost
        // bottom bar (spec §2.3); the adaptive view-switcher bar reveals *beneath*
        // the Now-bar at the narrow breakpoint (added in `attach_podcasts_view`).
        toolbar.add_bottom_bar(&status_bar);
        toolbar.add_bottom_bar(&now_bar.root);

        // The Now-bar cover/title cluster toggles the drawer (the click handle).
        let click = gtk::GestureClick::new();
        let weak = self.downgrade();
        click.connect_released(move |_, _, _, _| {
            if let Some(win) = weak.upgrade() {
                win.toggle_now_playing();
            }
        });
        now_bar.left.add_controller(click);

        // The transport's podcast affordance opens the playing episode's per-show
        // playback settings (speed / Smart Speed / Voice Boost) right by play/pause.
        let weak = self.downgrade();
        now_bar.podcast_btn.connect_clicked(move |btn| {
            if let Some(win) = weak.upgrade() {
                win.open_playing_show_settings(btn);
            }
        });

        // The plugin pages (lazily built) plus the shared multi-view chrome
        // (switcher in the header, the adaptive bottom switcher bar, the
        // breakpoint), which exists only when a second view is compiled in. A
        // music-only build (`--no-default-features`) keeps a single-page stack
        // with no switcher: visually unchanged.
        #[cfg(feature = "podcasts")]
        if podcasts_on {
            self.attach_podcasts_view(&stack);
        }
        #[cfg(feature = "audiobooks")]
        if audiobooks_on {
            self.attach_audiobooks_view(&stack);
        }
        // The view chrome (switcher / bottom bar / breakpoint) is only useful when
        // more than one tab is actually showing this launch.
        #[cfg(any(feature = "podcasts", feature = "audiobooks"))]
        if podcasts_on || audiobooks_on {
            self.install_view_chrome(&stack, &header, &toolbar);
        }

        self.set_content(Some(&toolbar));
        let _ = imp.view_stack.set(stack);
        let _ = imp.toast_overlay.set(toast_overlay);

        // Music-only header controls follow the active tab (Phase 16f): only
        // compiled when a second tab exists (a music-only build never switches).
        #[cfg(any(feature = "podcasts", feature = "audiobooks"))]
        if let Some(view_stack) = imp.view_stack.get() {
            let weak = self.downgrade();
            view_stack.connect_visible_child_notify(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.update_header_for_view();
                }
            });
            self.update_header_for_view();
        }

        *imp.panes.borrow_mut() = panes;
        let _ = imp.leaf.set(leaf);
        let _ = imp.filter_entry.set(filter.clone());
        let _ = imp.now_bar.set(now_bar);
        let _ = imp.now_playing.set(now_playing);
        let _ = imp.inspector.set(inspector);
        let _ = imp.status_left.set(status_left);
        let _ = imp.status_right.set(status_right);
        let _ = imp.queue_current.set(queue_current);
        self.install_queue_keys(&queue_panel.list);
        self.install_view_keys();
        self.install_queue_context_menu(&queue_panel.list);
        let _ = imp.queue_panel.set(queue_panel);

        // Double-click / Enter on a track plays the visible list from that row
        // (spec §3.6, the deadbeef idiom).
        if let Some(leaf) = imp.leaf.get() {
            let weak = self.downgrade();
            leaf.column_view.connect_activate(move |_, pos| {
                if let Some(win) = weak.upgrade() {
                    win.on_track_activated(pos);
                }
            });

            // Refresh the properties inspector (Phase 11a) as the selection
            // moves; a no-op while the panel is closed.
            let weak = self.downgrade();
            leaf.selection.connect_selection_changed(move |_, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.refresh_inspector();
                    win.refresh_status_aggregate();
                    win.refresh_edit_sensitivity();
                }
            });

            // Ctrl+Enter appends the selection (plain Enter / double-click
            // replaces, via `connect_activate` above).
            let append = gtk::ShortcutController::new();
            let weak = self.downgrade();
            append.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>Return"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade() {
                        win.queue_append_selection();
                    }
                    glib::Propagation::Stop
                })),
            ));
            // Ctrl+E opens the bulk-edit dialog over the selection (spec §3.5).
            let weak = self.downgrade();
            append.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string("<Control>e"),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade() {
                        win.prompt_bulk_edit();
                    }
                    glib::Propagation::Stop
                })),
            ));
            leaf.column_view.add_controller(append);

            // The right-click context menus (Phase 16a): actions + the shared
            // PopoverMenus for the leaf and the facet panes.
            self.install_track_context_menu();
            self.install_facet_context_menu();
        }

        // Window-scoped feedback actions (16.5c): the self-contained tab
        // modules (podcasts / audiobooks) reach the toast overlay and the
        // queue drawer through the widget tree (`activate_action("win.…")`)
        // without holding a window reference.
        let toast_action = gio::SimpleAction::new("toast", Some(glib::VariantTy::STRING));
        let weak = self.downgrade();
        toast_action.connect_activate(move |_, param| {
            if let (Some(win), Some(msg)) = (weak.upgrade(), param.and_then(|p| p.str())) {
                win.toast(msg);
            }
        });
        self.add_action(&toast_action);
        let reload_queue = gio::SimpleAction::new("reload-queue", None);
        let weak = self.downgrade();
        reload_queue.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.reload_queue_panel();
            }
        });
        self.add_action(&reload_queue);

        // The debounced cascade: a burst of selection changes flushes once,
        // recomputing from the earliest changed pane.
        let weak = self.downgrade();
        let coalescer: Coalescer = CoalescingQueue::new(
            Duration::from_millis(120),
            Duration::from_secs(2),
            Box::new(move |batch: Vec<usize>| {
                if let (Some(win), Some(earliest)) = (weak.upgrade(), batch.into_iter().min()) {
                    win.recompute_from(earliest);
                }
            }),
        );
        let _ = imp.coalescer.set(coalescer);

        // The filter bar debounces the same way; a flush just re-narrows the leaf
        // (the panes are unaffected by the grammar).
        let weak = self.downgrade();
        let filter_coalescer: FilterCoalescer = CoalescingQueue::new(
            Duration::from_millis(180),
            Duration::from_secs(2),
            Box::new(move |_: Vec<()>| {
                if let Some(win) = weak.upgrade() {
                    win.set_leaf();
                }
            }),
        );
        let _ = imp.filter_coalescer.set(filter_coalescer);

        for (i, pane) in imp.panes.borrow().iter().enumerate() {
            let weak = self.downgrade();
            pane.selection.connect_selection_changed(move |_, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.on_pane_changed(i);
                }
            });
            // Double-click / Enter on a facet value plays its filtered set
            // (Phase 13e-i, deadbeef-cui activate-to-play).
            let weak = self.downgrade();
            pane.column_view.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.on_facet_activated(i);
                }
            });
        }

        let weak = self.downgrade();
        filter.connect_search_changed(move |_| {
            if let Some(win) = weak.upgrade()
                && let Some(c) = win.imp().filter_coalescer.get()
            {
                c.add(());
            }
        });

        self.install_filter_shortcut(&filter);

        // Poll the player snapshot to refresh the Now-bar (a sampled transport
        // display; ~4×/s is plenty). The SourceId is removed on close.
        if imp.player.get().is_some() {
            let weak = self.downgrade();
            let id =
                glib::timeout_add_local(Duration::from_millis(250), move || match weak.upgrade() {
                    Some(win) => {
                        win.refresh_now_bar();
                        win.refresh_queue_highlight();
                        win.refresh_play_glyphs();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                });
            *imp.poll_source.borrow_mut() = Some(id);
        }

        // Teardown order on close: stop the poll (so no tick hits a dead handle),
        // then shut down + join the player (its terminal flush block_on's the
        // worker, still alive), then the worker/runtime drop as the window is
        // finalized.
        let weak = self.downgrade();
        self.connect_close_request(move |_| {
            if let Some(win) = weak.upgrade() {
                let imp = win.imp();
                if let Some(id) = imp.poll_source.borrow_mut().take() {
                    id.remove();
                }
                if let Some(player) = imp.player.get() {
                    player.shutdown();
                }
                // A manually-parented PopoverMenu must be unparented before the
                // window finalizes, or GTK warns about a disposed widget with a
                // parent (Phase 16a).
                if let Some(menu) = imp.track_menu.get() {
                    menu.unparent();
                }
                if let Some(menu) = imp.facet_menu.get() {
                    menu.unparent();
                }
                if let Some(menu) = imp.queue_menu.get() {
                    menu.unparent();
                }
            }
            glib::Propagation::Proceed
        });

        if imp.pool.get().is_some() {
            self.populate_initial();
            self.refresh_perspectives();
            self.refresh_playlists();
        }
        // Debug mode (Phase 14): a one-shot RSS sample once the library is loaded,
        // then a periodic sampler, so `--debug` can be checked against the spec §13
        // memory budget. No-ops when debug mode is off.
        conservatory_core::debug::log_memory("library-loaded");
        if let Some(rt) = imp.runtime.get() {
            conservatory_core::debug::spawn_memory_sampler(rt.handle());
        }
        // Apply the persisted equalizer so it is active from the first track
        // (Phase 5.5b-ii).
        self.apply_persisted_eq();
        // Apply the persisted DSP / output config so they are active from the
        // first track (Phase 5.5c-ii). The playback defaults (ReplayGain / gapless)
        // are not pushed here: they are resolved per-queue via `playback_config`.
        self.apply_persisted_audio();
        // Load the saved queue paused at the cursor (Phase 4b-ii-c).
        self.resume_saved_queue();
    }

    /// Push the persisted EQ state into the engine at startup (Phase 5.5b-ii).
    fn apply_persisted_eq(&self) {
        let imp = self.imp();
        let (Some(pool), Some(player)) = (imp.pool.get(), imp.player.get()) else {
            return;
        };
        if let Ok(conn) = pool.open()
            && let Ok(eq) = get_eq_state(&conn)
        {
            player.set_eq(eq);
        }
    }

    /// Push the persisted DSP modules + output backend / resampler into the engine
    /// at startup (Phase 5.5c-ii). 5.5c-i stored the DSP state but the GUI never
    /// applied it; this fixes that and adds the output half.
    fn apply_persisted_audio(&self) {
        let imp = self.imp();
        let (Some(pool), Some(player)) = (imp.pool.get(), imp.player.get()) else {
            return;
        };
        if let Ok(conn) = pool.open()
            && let Ok(state) = get_audio_state(&conn)
        {
            player.set_dsp(state.dsp);
            player.set_smart_speed_level(conservatory_core::player::SmartSpeedLevel::from_db(
                &state.smart_speed_level,
            ));
            player.set_output_backend(state.output_backend);
            player.set_resampler_quality(state.resampler);
        }
    }

    /// The persisted playback defaults (ReplayGain mode / preamp / clip, gapless),
    /// read fresh for each newly built queue (Phase 5.5c-ii). Falls back to the
    /// spec §10 defaults if the read is unavailable. A change applies to the next
    /// queue built, not retroactively to the playing one (the profile is baked per
    /// `PlayableItem`).
    fn playback_config(&self) -> PlaybackConfig {
        let Some(pool) = self.imp().pool.get() else {
            return PlaybackConfig::default();
        };
        if let Ok(conn) = pool.open()
            && let Ok(state) = get_audio_state(&conn)
        {
            PlaybackConfig::from_audio_state(&state)
        } else {
            PlaybackConfig::default()
        }
    }

    /// Open the "Sound" preferences dialog (Phase 5.5b-ii): the app's first
    /// The `adw::PreferencesDialog` (Phase 10b). The General + Library pages edit
    /// `config.toml` (the 10a loader); the Sound page hosts the 10-band graphic
    /// equalizer plus the ReplayGain / DSP / output groups, which persist to the
    /// DB singletons. Built fresh each open from the stored state.
    fn open_preferences(&self) {
        let Some(pool) = self.imp().pool.get() else {
            return;
        };
        let (state, presets) = {
            let Ok(conn) = pool.open() else { return };
            (
                get_eq_state(&conn).unwrap_or_else(|_| EqState::flat()),
                list_eq_presets(&conn).unwrap_or_default(),
            )
        };
        // Suppress the slider/combo feedback loop while we set values
        // programmatically (a preset load / reset), so it is not seen as an edit.
        let suppress = Rc::new(Cell::new(false));

        // The EQ sliders, one per band, under a centre-frequency label.
        let slider_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        slider_box.set_homogeneous(true);
        slider_box.set_margin_top(6);
        slider_box.set_margin_bottom(6);
        let mut sliders = Vec::with_capacity(EQ_CENTRES.len());
        for (i, centre) in EQ_CENTRES.iter().enumerate() {
            let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
            col.set_hexpand(true);
            let slider = sound::eq_slider(state.bands[i]);
            let label = gtk::Label::new(Some(&fmt_centre(*centre)));
            label.add_css_class("caption");
            label.add_css_class("dim-label");
            col.append(&slider);
            col.append(&label);
            slider_box.append(&col);
            sliders.push(slider);
        }
        let sliders = Rc::new(sliders);

        let eq_group = adw::PreferencesGroup::new();
        eq_group.set_title("Equalizer");
        eq_group.set_description(Some("Drag a band to hear it change live (dB)."));
        eq_group.add(&slider_box);

        // Preset picker: the named presets plus a trailing "Custom" marker.
        let preset_names: Vec<String> = presets.iter().map(|p| p.name.clone()).collect();
        let custom_index = preset_names.len() as u32;
        let mut items: Vec<&str> = preset_names.iter().map(String::as_str).collect();
        items.push(sound::CUSTOM_LABEL);
        let model = gtk::StringList::new(&items);
        let preset_row = adw::ComboRow::new();
        preset_row.set_title("Preset");
        preset_row.set_model(Some(&model));
        let initial = match sound::match_preset(&state.bands, &presets) {
            Some(name) => preset_names
                .iter()
                .position(|n| *n == name)
                .map_or(custom_index, |i| i as u32),
            None => custom_index,
        };
        preset_row.set_selected(initial);

        let save_btn = gtk::Button::with_label("Save as…");
        let delete_btn = gtk::Button::with_label("Delete");
        let reset_btn = gtk::Button::with_label("Reset");
        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btns.append(&save_btn);
        btns.append(&delete_btn);
        btns.append(&reset_btn);
        let presets_group = adw::PreferencesGroup::new();
        presets_group.set_title("Presets");
        presets_group.set_header_suffix(Some(&btns));
        presets_group.add(&preset_row);

        let page = adw::PreferencesPage::new();
        page.set_title("Sound");
        page.set_icon_name(Some("audio-card-symbolic"));
        page.add(&eq_group);
        page.add(&presets_group);

        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Preferences");

        // The config-backed pages (Phase 10b) come first, so Ctrl+, opens on
        // General; the Sound page (DB-backed audio) follows. The config is loaded
        // once into a shared cell the row handlers mutate, then saved on close.
        let config = Rc::new(RefCell::new(
            conservatory_core::config::load_default().unwrap_or_default(),
        ));
        dialog.add(&self.build_general_page(&config));
        dialog.add(&self.build_library_page(&config));
        dialog.add(&page);
        {
            let config = config.clone();
            dialog.connect_closed(move |_| {
                if let Err(e) = conservatory_core::config::save_default(&config.borrow()) {
                    tracing::warn!("saving config failed: {e}");
                }
            });
        }

        // The ReplayGain / DSP / Output groups (Phase 5.5c-ii), backed by the
        // singleton `audio_state` (separate from the EQ's own table above).
        self.add_audio_groups(&page, &dialog);

        // Slider drag → live band change + mark the EQ "Custom".
        for (i, slider) in sliders.iter().enumerate() {
            let weak = self.downgrade();
            let suppress = suppress.clone();
            let preset_row = preset_row.clone();
            slider.connect_value_changed(move |s| {
                if suppress.get() {
                    return;
                }
                let gain = s.value();
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.set_eq_band(i, gain);
                }
                suppress.set(true);
                preset_row.set_selected(custom_index);
                suppress.set(false);
            });
        }

        // Preset selected → load it into the sliders + the engine.
        {
            let weak = self.downgrade();
            let suppress = suppress.clone();
            let sliders = sliders.clone();
            let names = preset_names.clone();
            preset_row.connect_selected_notify(move |row| {
                if suppress.get() || row.selected() == custom_index {
                    return;
                }
                if let (Some(win), Some(name)) =
                    (weak.upgrade(), names.get(row.selected() as usize))
                {
                    win.load_eq_preset(name, &sliders, &suppress);
                }
            });
        }

        // Save / Delete / Reset.
        {
            let weak = self.downgrade();
            let sliders = sliders.clone();
            save_btn.connect_clicked(move |btn| {
                if let Some(win) = weak.upgrade() {
                    win.prompt_save_eq_preset(&sliders, btn);
                }
            });
        }
        {
            let weak = self.downgrade();
            let preset_row = preset_row.clone();
            let names = preset_names.clone();
            delete_btn.connect_clicked(move |_| {
                let idx = preset_row.selected();
                if let (Some(win), Some(name)) = (weak.upgrade(), names.get(idx as usize)) {
                    win.delete_eq_preset(name);
                }
            });
        }
        {
            let weak = self.downgrade();
            let sliders = sliders.clone();
            let suppress = suppress.clone();
            let preset_row = preset_row.clone();
            reset_btn.connect_clicked(move |_| {
                let Some(win) = weak.upgrade() else { return };
                suppress.set(true);
                for s in sliders.iter() {
                    s.set_value(0.0);
                }
                preset_row.set_selected(0); // Flat is seeded first
                suppress.set(false);
                win.persist_and_apply_eq([0.0; EQ_CENTRES.len()], Some("Flat".to_string()));
            });
        }

        // Persist the final slider state on close (live drags apply instantly but
        // are saved here; explicit actions above persist immediately).
        {
            let weak = self.downgrade();
            let sliders = sliders.clone();
            dialog.connect_closed(move |_| {
                if let Some(win) = weak.upgrade() {
                    let bands = read_slider_bands(&sliders);
                    let preset = sound::match_preset(&bands, &presets);
                    win.persist_eq(bands, preset);
                }
            });
        }

        dialog.present(Some(self));
    }

    /// Toggle the track properties inspector (Phase 11a) and refresh it on open.
    fn toggle_inspector(&self) {
        let Some(inspector) = self.imp().inspector.get() else {
            return;
        };
        inspector.set_open(!inspector.is_open());
        self.refresh_inspector();
    }

    /// Repopulate the inspector from the first selected track (Phase 11a). A
    /// no-op while the panel is closed, so selection churn costs nothing then.
    fn refresh_inspector(&self) {
        let imp = self.imp();
        let (Some(inspector), Some(leaf), Some(pool)) =
            (imp.inspector.get(), imp.leaf.get(), imp.pool.get())
        else {
            return;
        };
        if !inspector.is_open() {
            return;
        }
        let selected = leaf.selection.selection();
        if selected.size() == 0 {
            inspector.clear();
            return;
        }
        let Some(obj) = leaf.selection.item(selected.nth(0)) else {
            inspector.clear();
            return;
        };
        let Ok(row) = obj.downcast::<TrackRow>() else {
            return;
        };
        let Ok(conn) = pool.open() else { return };
        let Ok(Some(track)) = get_track(&conn, row.brief().id) else {
            inspector.clear();
            return;
        };
        let album = track
            .album_id
            .and_then(|id| get_album(&conn, id).ok().flatten());
        let artist = track
            .artist_id
            .and_then(|id| get_artist(&conn, id).ok().flatten());
        let root = imp.library_root.get();
        let file_size = root
            .map(|r| r.join(&track.file_path))
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        let fields = inspector_fields(
            &track,
            album.as_ref(),
            artist.as_ref().map(|a| a.name.as_str()),
            file_size,
        );
        let cover_abs = root
            .zip(album.as_ref().and_then(|a| a.cover_path.as_deref()))
            .map(|(r, cover)| r.join(cover));
        let accent = album.as_ref().and_then(|a| a.accent_rgb);
        inspector.show(&track.title, &fields, cover_abs.as_deref(), accent);
    }

    /// The General preferences page (Phase 10b): the `[library]` and `[genre]`
    /// sections of `config.toml`. Each row mutates the shared `config`; the
    /// dialog saves it on close. The library root applies on the next launch
    /// (the running session holds the root it started with).
    fn build_general_page(&self, config: &Rc<RefCell<Config>>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        page.set_title("General");
        page.set_icon_name(Some("preferences-system-symbolic"));

        let lib_group = adw::PreferencesGroup::new();
        lib_group.set_title("Library");
        // The dialog edits config.toml on disk; the running app keeps its
        // startup snapshot, so every config-backed setting is next-launch.
        lib_group.set_description(Some(
            "These settings take effect on the next launch.",
        ));

        let root_row = adw::ActionRow::new();
        root_row.set_title("Library root");
        let current = config.borrow().library.root.clone();
        root_row.set_subtitle(
            &current
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none)".to_string()),
        );
        let choose = gtk::Button::with_label("Choose…");
        choose.set_valign(gtk::Align::Center);
        {
            let config = config.clone();
            let root_row = root_row.clone();
            let weak = self.downgrade();
            choose.connect_clicked(move |_| {
                let Some(win) = weak.upgrade() else { return };
                let chooser = gtk::FileDialog::new();
                chooser.set_title("Choose library root");
                let config = config.clone();
                let root_row = root_row.clone();
                chooser.select_folder(Some(&win), gtk::gio::Cancellable::NONE, move |res| {
                    if let Ok(file) = res
                        && let Some(path) = file.path()
                    {
                        root_row.set_subtitle(&path.display().to_string());
                        config.borrow_mut().library.root = Some(path);
                    }
                });
            });
        }
        root_row.add_suffix(&choose);
        lib_group.add(&root_row);

        let tmpl = adw::EntryRow::new();
        tmpl.set_title("Music path template");
        tmpl.set_text(&config.borrow().library.path_template);
        {
            let config = config.clone();
            tmpl.connect_changed(move |e| {
                config.borrow_mut().library.path_template = e.text().to_string();
            });
        }
        lib_group.add(&tmpl);

        let modes = gtk::StringList::new(&["Copy", "Move"]);
        let import = adw::ComboRow::new();
        import.set_title("Import mode");
        import.set_subtitle("Copy leaves originals; Move consumes them");
        import.set_model(Some(&modes));
        import.set_selected(import_mode_index(config.borrow().library.import_mode));
        {
            let config = config.clone();
            import.connect_selected_notify(move |r| {
                config.borrow_mut().library.import_mode = import_mode_from_index(r.selected());
            });
        }
        lib_group.add(&import);

        let embed = adw::SwitchRow::new();
        embed.set_title("Embed tags on edit");
        embed.set_subtitle("Write curated metadata back into files");
        embed.set_active(config.borrow().library.embed_tags_on_edit);
        {
            let config = config.clone();
            embed.connect_active_notify(move |s| {
                config.borrow_mut().library.embed_tags_on_edit = s.is_active();
            });
        }
        lib_group.add(&embed);
        page.add(&lib_group);

        let genre_group = adw::PreferencesGroup::new();
        genre_group.set_title("Genre");
        genre_group.set_description(Some("Takes effect on the next launch."));
        let unknown = adw::EntryRow::new();
        unknown.set_title("Default unknown genre");
        unknown.set_text(&config.borrow().genre.default_unknown);
        {
            let config = config.clone();
            unknown.connect_changed(move |e| {
                config.borrow_mut().genre.default_unknown = e.text().to_string();
            });
        }
        genre_group.add(&unknown);
        page.add(&genre_group);

        // Sections (Phase 16e): which media tabs to show. Disabling one skips
        // building its tab and starting its subsystem at the next launch. The
        // Podcasts / Audiobooks toggles appear only when those plugins are compiled
        // in (a music-only build shows just Music).
        let sections_group = adw::PreferencesGroup::new();
        sections_group.set_title("Sections");
        sections_group.set_description(Some(
            "Which media tabs to show. Takes effect on the next launch.",
        ));
        let music_sw = adw::SwitchRow::new();
        music_sw.set_title("Music");
        music_sw.set_active(config.borrow().sections.music);
        {
            let config = config.clone();
            music_sw.connect_active_notify(move |s| {
                config.borrow_mut().sections.music = s.is_active();
            });
        }
        sections_group.add(&music_sw);
        #[cfg(feature = "podcasts")]
        {
            let podcasts_sw = adw::SwitchRow::new();
            podcasts_sw.set_title("Podcasts");
            podcasts_sw.set_active(config.borrow().sections.podcasts);
            let config = config.clone();
            podcasts_sw.connect_active_notify(move |s| {
                config.borrow_mut().sections.podcasts = s.is_active();
            });
            sections_group.add(&podcasts_sw);
        }
        #[cfg(feature = "audiobooks")]
        {
            let audiobooks_sw = adw::SwitchRow::new();
            audiobooks_sw.set_title("Audiobooks");
            audiobooks_sw.set_active(config.borrow().sections.audiobooks);
            let config = config.clone();
            audiobooks_sw.connect_active_notify(move |s| {
                config.borrow_mut().sections.audiobooks = s.is_active();
            });
            sections_group.add(&audiobooks_sw);
        }
        page.add(&sections_group);

        page
    }

    /// The Library preferences page (Phase 10b): the `[podcasts]` and
    /// `[audiobooks]` sections of `config.toml`. The facet-pane configuration
    /// (spec §3.2) joins this page in Phase 10c. Kept `#[cfg]`-free: these are
    /// plain config rows, present in the music-only build too.
    fn build_library_page(&self, config: &Rc<RefCell<Config>>) -> adw::PreferencesPage {
        let page = adw::PreferencesPage::new();
        page.set_title("Library");
        page.set_icon_name(Some("folder-music-symbolic"));

        // Browse panes (Phase 10c): five ordered slots, each a facet field or
        // "(none)". The non-empty slots, top to bottom, become `[browse].panes`.
        let browse_group = adw::PreferencesGroup::new();
        browse_group.set_title("Browse panes");
        browse_group.set_description(Some(
            "Choose up to five browse columns, top to bottom. Takes effect on the next launch.",
        ));
        let mut items: Vec<&str> = vec!["(none)"];
        items.extend(FacetField::ALL.iter().map(|f| f.title()));
        let current = FacetField::panes_from_config(&config.borrow().browse.panes);
        let mut combo_rows = Vec::with_capacity(5);
        for slot in 0..5 {
            let model = gtk::StringList::new(&items);
            let row = adw::ComboRow::new();
            row.set_title(&format!("Pane {}", slot + 1));
            row.set_model(Some(&model));
            let selected = current
                .get(slot)
                .and_then(|f| FacetField::ALL.iter().position(|x| x == f))
                .map(|i| i as u32 + 1)
                .unwrap_or(0);
            row.set_selected(selected);
            browse_group.add(&row);
            combo_rows.push(row);
        }
        let combo_rows = Rc::new(combo_rows);
        for row in combo_rows.iter() {
            let config = config.clone();
            let combo_rows = combo_rows.clone();
            row.connect_selected_notify(move |_| {
                let panes: Vec<String> = combo_rows
                    .iter()
                    .filter_map(|r| {
                        r.selected()
                            .checked_sub(1)
                            .and_then(|k| FacetField::ALL.get(k as usize))
                            .map(|f| f.as_key().to_string())
                    })
                    .collect();
                config.borrow_mut().browse.panes = panes;
            });
        }
        page.add(&browse_group);

        let pod_group = adw::PreferencesGroup::new();
        pod_group.set_title("Podcasts");
        pod_group.set_description(Some("Takes effect on the next launch."));

        let pod_subdir = adw::EntryRow::new();
        pod_subdir.set_title("Library subfolder");
        pod_subdir.set_text(&config.borrow().podcasts.library_subdir);
        {
            let config = config.clone();
            pod_subdir.connect_changed(move |e| {
                config.borrow_mut().podcasts.library_subdir = e.text().to_string();
            });
        }
        pod_group.add(&pod_subdir);

        let pod_max = adw::SpinRow::with_range(1.0, 16.0, 1.0);
        pod_max.set_title("Max concurrent downloads");
        pod_max.set_value(config.borrow().podcasts.max_concurrent_downloads as f64);
        {
            let config = config.clone();
            pod_max.connect_value_notify(move |r| {
                config.borrow_mut().podcasts.max_concurrent_downloads = r.value() as u32;
            });
        }
        pod_group.add(&pod_max);
        page.add(&pod_group);

        let book_group = adw::PreferencesGroup::new();
        book_group.set_title("Audiobooks");
        book_group.set_description(Some("Takes effect on the next launch."));

        let book_subdir = adw::EntryRow::new();
        book_subdir.set_title("Library subfolder");
        book_subdir.set_text(&config.borrow().audiobooks.library_subdir);
        {
            let config = config.clone();
            book_subdir.connect_changed(move |e| {
                config.borrow_mut().audiobooks.library_subdir = e.text().to_string();
            });
        }
        book_group.add(&book_subdir);

        let book_tmpl = adw::EntryRow::new();
        book_tmpl.set_title("Path template");
        book_tmpl.set_text(&config.borrow().audiobooks.path_template);
        {
            let config = config.clone();
            book_tmpl.connect_changed(move |e| {
                config.borrow_mut().audiobooks.path_template = e.text().to_string();
            });
        }
        book_group.add(&book_tmpl);

        let book_speed = adw::SpinRow::with_range(0.5, 3.0, 0.05);
        book_speed.set_title("Default speed");
        book_speed.set_digits(2);
        book_speed.set_value(config.borrow().audiobooks.default_speed);
        {
            let config = config.clone();
            book_speed.connect_value_notify(move |r| {
                config.borrow_mut().audiobooks.default_speed = r.value();
            });
        }
        book_group.add(&book_speed);

        let book_ss = adw::SwitchRow::new();
        book_ss.set_title("Smart Speed");
        book_ss.set_active(config.borrow().audiobooks.smart_speed);
        {
            let config = config.clone();
            book_ss.connect_active_notify(move |s| {
                config.borrow_mut().audiobooks.smart_speed = s.is_active();
            });
        }
        book_group.add(&book_ss);

        let book_vb = adw::SwitchRow::new();
        book_vb.set_title("Voice Boost");
        book_vb.set_active(config.borrow().audiobooks.voice_boost);
        {
            let config = config.clone();
            book_vb.connect_active_notify(move |s| {
                config.borrow_mut().audiobooks.voice_boost = s.is_active();
            });
        }
        book_group.add(&book_vb);
        page.add(&book_group);

        page
    }

    /// Build the ReplayGain / DSP / Output groups of the Sound page (Phase
    /// 5.5c-ii) and wire them to the singleton `audio_state` (separate from the
    /// EQ's own table). DSP + output changes apply to the engine live; ReplayGain
    /// / gapless changes are resolved per-queue (so they take effect on the next
    /// built queue, not retroactively). The whole state persists on dialog close.
    fn add_audio_groups(&self, page: &adw::PreferencesPage, dialog: &adw::PreferencesDialog) {
        let Some(pool) = self.imp().pool.get() else {
            return;
        };
        let initial = {
            let Ok(conn) = pool.open() else { return };
            get_audio_state(&conn).unwrap_or_default()
        };
        let state = Rc::new(RefCell::new(initial));

        // "DSP changed → push the whole DspState to the engine" (a structural `af`
        // rebuild, gap-acceptable; DSP has no per-slider live path like the EQ).
        let apply_dsp: Rc<dyn Fn()> = {
            let weak = self.downgrade();
            let state = state.clone();
            Rc::new(move || {
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.set_dsp(state.borrow().dsp);
                }
            })
        };

        // --- ReplayGain ---
        let rg_group = adw::PreferencesGroup::new();
        rg_group.set_title("ReplayGain");
        rg_group.set_description(Some("Volume normalization; applies to the next queue."));

        let rg_mode = adw::ComboRow::new();
        rg_mode.set_title("Mode");
        rg_mode.set_model(Some(&gtk::StringList::new(&sound::option_labels(
            &sound::RG_MODES,
        ))));
        rg_mode.set_selected(sound::option_index(
            &sound::RG_MODES,
            &state.borrow().replaygain_mode,
        ));
        {
            let state = state.clone();
            rg_mode.connect_selected_notify(move |row| {
                state.borrow_mut().replaygain_mode =
                    sound::option_value(&sound::RG_MODES, row.selected()).to_string();
            });
        }

        let rg_preamp = adw::SpinRow::with_range(-15.0, 15.0, 0.5);
        rg_preamp.set_title("Preamp (dB)");
        rg_preamp.set_digits(1);
        rg_preamp.set_value(state.borrow().replaygain_preamp);
        {
            let state = state.clone();
            rg_preamp.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().replaygain_preamp = a.value();
            });
        }

        let rg_clip = adw::SwitchRow::new();
        rg_clip.set_title("Prevent clipping");
        rg_clip.set_subtitle("Cap the gain at 0 dB (no peak data)");
        rg_clip.set_active(state.borrow().replaygain_clip);
        {
            let state = state.clone();
            rg_clip.connect_active_notify(move |row| {
                state.borrow_mut().replaygain_clip = row.is_active();
            });
        }

        for row in [
            rg_mode.upcast_ref::<gtk::Widget>(),
            rg_preamp.upcast_ref(),
            rg_clip.upcast_ref(),
        ] {
            rg_group.add(row);
        }

        // --- Dynamics (DSP modules) ---
        let dsp_group = adw::PreferencesGroup::new();
        dsp_group.set_title("Dynamics");
        dsp_group.set_description(Some("Compressor, brick-wall limiter, and volume leveler."));

        // Compressor.
        let comp = adw::ExpanderRow::new();
        comp.set_title("Compressor");
        comp.set_show_enable_switch(true);
        comp.set_enable_expansion(state.borrow().dsp.comp.enabled);
        let comp_threshold = adw::SpinRow::with_range(-60.0, 0.0, 1.0);
        comp_threshold.set_title("Threshold (dB)");
        comp_threshold.set_value(state.borrow().dsp.comp.settings.threshold_db);
        let comp_ratio = adw::SpinRow::with_range(1.0, 20.0, 0.5);
        comp_ratio.set_title("Ratio (N:1)");
        comp_ratio.set_digits(1);
        comp_ratio.set_value(state.borrow().dsp.comp.settings.ratio);
        let comp_attack = adw::SpinRow::with_range(1.0, 200.0, 1.0);
        comp_attack.set_title("Attack (ms)");
        comp_attack.set_value(state.borrow().dsp.comp.settings.attack_ms);
        let comp_release = adw::SpinRow::with_range(10.0, 2000.0, 10.0);
        comp_release.set_title("Release (ms)");
        comp_release.set_value(state.borrow().dsp.comp.settings.release_ms);
        comp.add_row(&comp_threshold);
        comp.add_row(&comp_ratio);
        comp.add_row(&comp_attack);
        comp.add_row(&comp_release);
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            comp.connect_enable_expansion_notify(move |e| {
                state.borrow_mut().dsp.comp.enabled = e.enables_expansion();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            comp_threshold.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.comp.settings.threshold_db = a.value();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            comp_ratio.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.comp.settings.ratio = a.value();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            comp_attack.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.comp.settings.attack_ms = a.value();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            comp_release.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.comp.settings.release_ms = a.value();
                apply();
            });
        }

        // Limiter.
        let limiter = adw::ExpanderRow::new();
        limiter.set_title("Limiter");
        limiter.set_subtitle("A transparent peak catcher / ReplayGain clip net");
        limiter.set_show_enable_switch(true);
        limiter.set_enable_expansion(state.borrow().dsp.limiter.enabled);
        let limiter_ceiling = adw::SpinRow::with_range(-30.0, 0.0, 0.5);
        limiter_ceiling.set_title("Ceiling (dB)");
        limiter_ceiling.set_digits(1);
        limiter_ceiling.set_value(state.borrow().dsp.limiter.settings.ceiling_db);
        limiter.add_row(&limiter_ceiling);
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            limiter.connect_enable_expansion_notify(move |e| {
                state.borrow_mut().dsp.limiter.enabled = e.enables_expansion();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            limiter_ceiling
                .adjustment()
                .connect_value_changed(move |a| {
                    state.borrow_mut().dsp.limiter.settings.ceiling_db = a.value();
                    apply();
                });
        }

        // Leveler.
        let leveler = adw::ExpanderRow::new();
        leveler.set_title("Leveler");
        leveler.set_show_enable_switch(true);
        leveler.set_enable_expansion(state.borrow().dsp.leveler.enabled);
        let leveler_target = adw::SpinRow::with_range(0.0, 1.0, 0.05);
        leveler_target.set_title("Target peak");
        leveler_target.set_digits(2);
        leveler_target.set_value(state.borrow().dsp.leveler.settings.target_peak);
        let leveler_gauss = adw::SpinRow::with_range(3.0, 301.0, 2.0);
        leveler_gauss.set_title("Window size");
        leveler_gauss.set_value(state.borrow().dsp.leveler.settings.gausssize as f64);
        leveler.add_row(&leveler_target);
        leveler.add_row(&leveler_gauss);
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            leveler.connect_enable_expansion_notify(move |e| {
                state.borrow_mut().dsp.leveler.enabled = e.enables_expansion();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            leveler_target.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.leveler.settings.target_peak = a.value();
                apply();
            });
        }
        {
            let state = state.clone();
            let apply = apply_dsp.clone();
            leveler_gauss.adjustment().connect_value_changed(move |a| {
                state.borrow_mut().dsp.leveler.settings.gausssize = a.value() as u32;
                apply();
            });
        }

        dsp_group.add(&comp);
        dsp_group.add(&limiter);
        dsp_group.add(&leveler);

        // --- Output ---
        let out_group = adw::PreferencesGroup::new();
        out_group.set_title("Output");

        let backend = adw::ComboRow::new();
        backend.set_title("Backend");
        backend.set_model(Some(&gtk::StringList::new(&sound::option_labels(
            &sound::BACKENDS,
        ))));
        backend.set_selected(sound::option_index(
            &sound::BACKENDS,
            &state.borrow().output_backend,
        ));
        {
            let state = state.clone();
            let weak = self.downgrade();
            backend.connect_selected_notify(move |row| {
                let val = sound::option_value(&sound::BACKENDS, row.selected()).to_string();
                state.borrow_mut().output_backend = val.clone();
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.set_output_backend(val);
                }
            });
        }
        out_group.add(&backend);

        // Device: a second, write-through surface for the header picker (Phase
        // 4c-ii). Listed from the engine's queried device list; skipped if the
        // engine is unavailable.
        if let Some(player) = self.imp().player.get() {
            let snap = player.snapshot();
            if !snap.audio_devices.is_empty() {
                let names: Vec<String> =
                    snap.audio_devices.iter().map(|d| d.name.clone()).collect();
                let labels: Vec<String> = snap
                    .audio_devices
                    .iter()
                    .map(|d| {
                        if d.description.is_empty() {
                            d.name.clone()
                        } else {
                            d.description.clone()
                        }
                    })
                    .collect();
                let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let selected = snap
                    .audio_device
                    .as_deref()
                    .and_then(|c| names.iter().position(|n| n == c))
                    .unwrap_or(0) as u32;
                let device = adw::ComboRow::new();
                device.set_title("Device");
                device.set_model(Some(&gtk::StringList::new(&label_refs)));
                device.set_selected(selected);
                let weak = self.downgrade();
                device.connect_selected_notify(move |row| {
                    if let Some(name) = names.get(row.selected() as usize)
                        && let Some(win) = weak.upgrade()
                        && let Some(player) = win.imp().player.get()
                    {
                        player.set_audio_device(name.clone());
                    }
                });
                out_group.add(&device);
            }
        }

        let resampler = adw::ComboRow::new();
        resampler.set_title("Resampler");
        resampler.set_model(Some(&gtk::StringList::new(&sound::option_labels(
            &sound::RESAMPLERS,
        ))));
        resampler.set_selected(sound::option_index(
            &sound::RESAMPLERS,
            state.borrow().resampler.as_str(),
        ));
        {
            let state = state.clone();
            let weak = self.downgrade();
            resampler.connect_selected_notify(move |row| {
                let q: ResamplerQuality = sound::option_value(&sound::RESAMPLERS, row.selected())
                    .parse()
                    .unwrap_or_default();
                state.borrow_mut().resampler = q;
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.set_resampler_quality(q);
                }
            });
        }
        out_group.add(&resampler);

        let gapless = adw::SwitchRow::new();
        gapless.set_title("Gapless playback");
        gapless.set_active(state.borrow().gapless);
        {
            let state = state.clone();
            gapless.connect_active_notify(move |row| {
                state.borrow_mut().gapless = row.is_active();
            });
        }
        out_group.add(&gapless);

        // --- Spoken word (Smart Speed level) ---
        let ss_group = adw::PreferencesGroup::new();
        ss_group.set_title("Spoken word");
        ss_group.set_description(Some(
            "How aggressively Smart Speed trims dead air. Turn Smart Speed on per \
             show or per book; this sets how hard it cuts (applies live).",
        ));
        let ss_level = adw::ComboRow::new();
        ss_level.set_title("Smart Speed level");
        ss_level.set_model(Some(&gtk::StringList::new(&sound::option_labels(
            &sound::SMART_SPEED_LEVELS,
        ))));
        ss_level.set_selected(sound::option_index(
            &sound::SMART_SPEED_LEVELS,
            &state.borrow().smart_speed_level,
        ));
        {
            let state = state.clone();
            let weak = self.downgrade();
            ss_level.connect_selected_notify(move |row| {
                let value = sound::option_value(&sound::SMART_SPEED_LEVELS, row.selected());
                state.borrow_mut().smart_speed_level = value.to_string();
                // Apply live to the current episode / book (persisted on close).
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.set_smart_speed_level(
                        conservatory_core::player::SmartSpeedLevel::from_db(value),
                    );
                }
            });
        }
        ss_group.add(&ss_level);

        page.add(&rg_group);
        page.add(&dsp_group);
        page.add(&ss_group);
        page.add(&out_group);

        // Persist the whole audio config on close (live changes applied above are
        // saved here, the EQ slider precedent).
        {
            let weak = self.downgrade();
            let state = state.clone();
            dialog.connect_closed(move |_| {
                if let Some(win) = weak.upgrade()
                    && let (Some(worker), Some(rt)) =
                        (win.imp().worker.get(), win.imp().runtime.get())
                {
                    let _ = rt.block_on(worker.set_audio_state(state.borrow().clone()));
                }
            });
        }
    }

    /// Load a named preset into the sliders + the engine (Phase 5.5b-ii).
    fn load_eq_preset(&self, name: &str, sliders: &[gtk::Scale], suppress: &Rc<Cell<bool>>) {
        let Some(pool) = self.imp().pool.get() else {
            return;
        };
        let bands = {
            let Ok(conn) = pool.open() else { return };
            match get_eq_preset(&conn, name) {
                Ok(Some(b)) => b,
                _ => return,
            }
        };
        suppress.set(true);
        for (s, g) in sliders.iter().zip(bands.iter()) {
            s.set_value(*g);
        }
        suppress.set(false);
        self.persist_and_apply_eq(bands, Some(name.to_string()));
    }

    /// Prompt for a name and save the current sliders as a preset (Phase 5.5b-ii).
    fn prompt_save_eq_preset(&self, sliders: &[gtk::Scale], anchor: &gtk::Button) {
        let bands = read_slider_bands(sliders);
        let entry = gtk::Entry::builder()
            .placeholder_text("Preset name")
            .build();
        let dialog = adw::AlertDialog::new(Some("Save EQ preset"), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "save" {
                return;
            }
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some(win) = weak.upgrade() {
                if let (Some(worker), Some(rt)) = (win.imp().worker.get(), win.imp().runtime.get())
                {
                    let _ = rt.block_on(worker.save_eq_preset(name.clone(), bands));
                }
                win.persist_and_apply_eq(bands, Some(name));
            }
        });
        dialog.present(Some(anchor));
    }

    /// Delete a named preset (Phase 5.5b-ii); `Flat` is protected.
    fn delete_eq_preset(&self, name: &str) {
        if name == "Flat" {
            return;
        }
        if let (Some(worker), Some(rt)) = (self.imp().worker.get(), self.imp().runtime.get()) {
            let _ = rt.block_on(worker.delete_eq_preset(name.to_string()));
        }
    }

    /// Persist the active EQ state through the worker (Phase 5.5b-ii).
    fn persist_eq(&self, bands: [f64; EQ_CENTRES.len()], preset: Option<String>) {
        if let (Some(worker), Some(rt)) = (self.imp().worker.get(), self.imp().runtime.get()) {
            let _ = rt.block_on(worker.set_eq_state(EqState { bands, preset }));
        }
    }

    /// Persist the EQ state *and* push it to the engine (Phase 5.5b-ii): the
    /// preset-load / reset path, where the whole band set changes at once.
    fn persist_and_apply_eq(&self, bands: [f64; EQ_CENTRES.len()], preset: Option<String>) {
        if let Some(player) = self.imp().player.get() {
            player.set_eq(EqState {
                bands,
                preset: preset.clone(),
            });
        }
        self.persist_eq(bands, preset);
    }

    /// Double-click / Enter on a track: play the visible leaf list from that row
    /// (spec §3.6).
    fn on_track_activated(&self, pos: u32) {
        self.play_leaf_from(pos);
    }

    /// Double-click / Enter on a facet value (Phase 13e-i): play that facet's
    /// filtered set, the deadbeef-cui activate-to-play move. The activated row is
    /// already selected, but the cascade that narrows the leaf to it is debounced,
    /// so flush it synchronously (`recompute_from`) before reading the leaf, then
    /// play from the top. The `[All]` row plays everything under the other panes
    /// (its constraint is empty).
    fn on_facet_activated(&self, pane: usize) {
        self.recompute_from(pane);
        self.play_leaf_from(0);
    }

    /// Play the visible leaf list from row `pos`. The selection model presents
    /// rows in display (sorted) order, so its index range is the queue order and
    /// `pos` is the start. Shared by track double-click and facet activate-to-play.
    fn play_leaf_from(&self, pos: u32) {
        let imp = self.imp();
        // Playback needs the library root to resolve the managed relative track
        // paths; a bare `conservatory <db>` launch can browse but not play. Log a
        // hint rather than failing silently (the config-sourced root is Phase 10).
        if imp.player.get().is_some() && imp.library_root.get().is_none() {
            eprintln!(
                "conservatory: can't play without a library root \u{2014} launch as \
                 `conservatory <db> <root>`"
            );
        }
        let (Some(pool), Some(leaf), Some(player), Some(root)) = (
            imp.pool.get(),
            imp.leaf.get(),
            imp.player.get(),
            imp.library_root.get(),
        ) else {
            return;
        };

        let model = &leaf.selection;
        let n = model.n_items();
        let mut ordered_ids = Vec::with_capacity(n as usize);
        let mut labels = std::collections::HashMap::new();
        for i in 0..n {
            if let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok()) {
                let brief = row.brief();
                let id = brief.id;
                ordered_ids.push(id);
                labels.insert(id, (brief.title, brief.artist.unwrap_or_default()));
            }
        }

        let Ok(conn) = pool.open() else { return };
        let tracks = get_tracks(&conn, &ordered_ids).unwrap_or_default();
        drop(conn);

        let (items, start) = build_play_queue(
            &ordered_ids,
            pos as usize,
            &tracks,
            root,
            &self.playback_config(),
        );
        if items.is_empty() {
            return;
        }

        // Write the DB queue through so it mirrors what the engine plays (the
        // spec §4.3 source of truth) and the drawer can render + edit it.
        let queue_ids: Vec<i64> = items.iter().map(|i| i.track_id).collect();
        if let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) {
            let _ = rt.block_on(worker.replace_queue_with_tracks(queue_ids));
        }

        *imp.now_labels.borrow_mut() = labels;
        imp.last_shown.set(None); // force a label refresh on the next poll
        imp.last_index.set(None);
        if let Some(cur) = imp.queue_current.get() {
            cur.set(Some(start as i64));
        }
        player.play_queue(items, start);
        self.reload_queue_panel();
    }

    /// Re-read the queue from the DB and repopulate the drawer (the playing-row
    /// highlight comes from the shared `queue_current` the factory reads).
    fn reload_queue_panel(&self) {
        let imp = self.imp();
        let (Some(pool), Some(panel)) = (imp.pool.get(), imp.queue_panel.get()) else {
            return;
        };
        let rows = pool
            .open()
            .ok()
            .and_then(|conn| load_queue_display(&conn).ok())
            .unwrap_or_default();
        panel.set_rows(&rows);
    }

    fn toggle_queue(&self) {
        if let Some(panel) = self.imp().queue_panel.get() {
            panel.toggle();
            if panel.revealer.reveals_child() {
                self.reload_queue_panel();
            }
        }
    }

    /// The output-device picker (Phase 4c-ii, spec §6.5): a header menu built
    /// fresh on each open from the engine snapshot's device list; selecting a row
    /// switches the mpv output and closes.
    fn build_output_menu_button(&self) -> gtk::MenuButton {
        let button = gtk::MenuButton::new();
        button.set_icon_name("audio-speakers-symbolic");
        button.set_tooltip_text(Some("Output device"));

        let weak = self.downgrade();
        button.set_create_popup_func(move |button| {
            let popover = gtk::Popover::new();
            let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
            list.set_margin_top(4);
            list.set_margin_bottom(4);

            if let Some(win) = weak.upgrade()
                && let Some(player) = win.imp().player.get()
            {
                let snap = player.snapshot();
                let current = snap.audio_device.as_deref();
                for dev in snap.audio_devices.iter() {
                    let selected = current == Some(dev.name.as_str())
                        || (current.is_none() && dev.name == "auto");
                    let row = gtk::Button::new();
                    row.add_css_class("flat");
                    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    let check = gtk::Image::from_icon_name("object-select-symbolic");
                    check.set_visible(selected);
                    let label = gtk::Label::new(Some(if dev.description.is_empty() {
                        &dev.name
                    } else {
                        &dev.description
                    }));
                    label.set_xalign(0.0);
                    label.set_hexpand(true);
                    row_box.append(&check);
                    row_box.append(&label);
                    row.set_child(Some(&row_box));

                    let name = dev.name.clone();
                    let win_weak = win.downgrade();
                    let pop_weak = popover.downgrade();
                    row.connect_clicked(move |_| {
                        if let Some(win) = win_weak.upgrade()
                            && let Some(player) = win.imp().player.get()
                        {
                            player.set_audio_device(name.clone());
                        }
                        if let Some(pop) = pop_weak.upgrade() {
                            pop.popdown();
                        }
                    });
                    list.append(&row);
                }
            }

            let scroller = gtk::ScrolledWindow::builder()
                .propagate_natural_height(true)
                .max_content_height(360)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&list)
                .build();
            popover.set_child(Some(&scroller));
            button.set_popover(Some(&popover));
        });

        button
    }

    /// On startup, load the saved DB queue into the engine paused at the cursor
    /// (Phase 4b-ii-c): reopening the app resumes where playback left off, silent
    /// until the user presses play.
    fn resume_saved_queue(&self) {
        let imp = self.imp();
        let (Some(pool), Some(player), Some(root)) =
            (imp.pool.get(), imp.player.get(), imp.library_root.get())
        else {
            return;
        };
        let Ok(conn) = pool.open() else { return };
        let rows = load_queue_display(&conn).unwrap_or_default();
        if rows.is_empty() {
            return;
        }
        let saved = read_playback_state(&conn).ok().flatten();
        // The Now-bar label map is keyed by the item's id (a track id, or an
        // episode id for an episode item, mirroring PlayableItem.track_id).
        let mut labels = std::collections::HashMap::new();
        for r in &rows {
            if let Some(id) = r.track_id.or(r.episode_id) {
                labels.insert(id, (r.title.clone(), r.artist.clone().unwrap_or_default()));
            }
        }
        let track_ids: Vec<i64> = rows.iter().filter_map(|r| r.track_id).collect();
        let tracks = get_tracks(&conn, &track_ids).unwrap_or_default();
        let show_ids: Vec<i64> = rows.iter().filter_map(|r| r.show_id).collect();
        let settings = show_settings_map(&conn, &show_ids).unwrap_or_default();
        drop(conn);

        let mixed: Vec<MixedQueueRow> = rows
            .iter()
            .map(|r| MixedQueueRow {
                kind: r.kind,
                track_id: r.track_id,
                episode_id: r.episode_id,
                book_id: r.book_id,
                show_id: r.show_id,
                audio_path: r.audio_path.clone(),
                audio_url: r.audio_url.clone(),
            })
            .collect();
        // The cursor's id is its track_id (track), episode_id (episode), or
        // book_id (audiobook, 7c-iii).
        let cursor_kind = saved.as_ref().map(|s| s.kind).unwrap_or(MediaKind::Track);
        let cursor_id = saved.as_ref().and_then(|s| match s.kind {
            MediaKind::Track => s.track_id,
            MediaKind::Episode => s.episode_id,
            MediaKind::Audiobook => s.book_id,
        });
        let (mut items, start) = build_mixed_queue(
            &mixed,
            &tracks,
            cursor_kind,
            cursor_id,
            root,
            &self.playback_config(),
            &settings,
        );
        crate::playqueue::attach_episode_chapters(&mut items, pool);
        crate::playqueue::attach_book_chapters(&mut items, pool, root);
        if items.is_empty() {
            return;
        }
        let position = saved.map(|s| s.position).unwrap_or(0.0);
        *imp.now_labels.borrow_mut() = labels;
        imp.last_shown.set(None);
        imp.last_index.set(None);
        if let Some(cur) = imp.queue_current.get() {
            cur.set(Some(start as i64));
        }
        player.resume(items, start, position);
        self.reload_queue_panel();
    }

    /// `Ctrl+Enter`: append the selected browse rows to the queue (DB tail +
    /// live engine tail), without disrupting playback.
    fn queue_append_selection(&self) {
        let imp = self.imp();
        let (Some(pool), Some(leaf), Some(player), Some(root), Some(rt), Some(worker)) = (
            imp.pool.get(),
            imp.leaf.get(),
            imp.player.get(),
            imp.library_root.get(),
            imp.runtime.get(),
            imp.worker.get(),
        ) else {
            return;
        };

        let model = &leaf.selection;
        let n = model.n_items();
        let mut ordered_ids = Vec::new();
        let mut labels = Vec::new();
        for i in 0..n {
            if model.is_selected(i)
                && let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok())
            {
                let brief = row.brief();
                ordered_ids.push(brief.id);
                labels.push((brief.id, (brief.title, brief.artist.unwrap_or_default())));
            }
        }
        if ordered_ids.is_empty() {
            return;
        }

        let Ok(conn) = pool.open() else { return };
        let tracks = get_tracks(&conn, &ordered_ids).unwrap_or_default();
        drop(conn);
        let (items, _start) =
            build_play_queue(&ordered_ids, 0, &tracks, root, &self.playback_config());
        if items.is_empty() {
            return;
        }

        let queue_ids: Vec<i64> = items.iter().map(|i| i.track_id).collect();
        let _ = rt.block_on(worker.enqueue_tracks(queue_ids));
        {
            let mut map = imp.now_labels.borrow_mut();
            for (id, lbl) in labels {
                map.insert(id, lbl);
            }
        }
        player.append(items);
        self.reload_queue_panel();
    }

    /// Play Next (Phase 16a): insert the selection just after the current item
    /// (or at the tail when nothing is playing). The DB queue and the live engine
    /// queue are inserted at the same index so they stay in lock-step (spec §4.3).
    fn play_next_selection(&self) {
        let imp = self.imp();
        let (Some(pool), Some(leaf), Some(player), Some(root), Some(rt), Some(worker)) = (
            imp.pool.get(),
            imp.leaf.get(),
            imp.player.get(),
            imp.library_root.get(),
            imp.runtime.get(),
            imp.worker.get(),
        ) else {
            return;
        };

        let model = &leaf.selection;
        let n = model.n_items();
        let mut ordered_ids = Vec::new();
        let mut labels = Vec::new();
        for i in 0..n {
            if model.is_selected(i)
                && let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok())
            {
                let brief = row.brief();
                ordered_ids.push(brief.id);
                labels.push((brief.id, (brief.title, brief.artist.unwrap_or_default())));
            }
        }
        if ordered_ids.is_empty() {
            return;
        }

        let Ok(conn) = pool.open() else { return };
        let tracks = get_tracks(&conn, &ordered_ids).unwrap_or_default();
        drop(conn);
        let (items, _start) =
            build_play_queue(&ordered_ids, 0, &tracks, root, &self.playback_config());
        if items.is_empty() {
            return;
        }

        let snap = player.snapshot();
        let at = match snap.current_index {
            Some(i) => i + 1,
            None => snap.queue_len,
        };

        let queue_ids: Vec<i64> = items.iter().map(|i| i.track_id).collect();
        let _ = rt.block_on(worker.insert_queue_tracks_at(at as i64, queue_ids));
        {
            let mut map = imp.now_labels.borrow_mut();
            for (id, lbl) in labels {
                map.insert(id, lbl);
            }
        }
        player.insert_items(at, items);
        self.reload_queue_panel();
    }

    /// The track ids currently selected in the leaf list (display order).
    fn selected_track_ids(&self) -> Vec<i64> {
        let imp = self.imp();
        let Some(leaf) = imp.leaf.get() else {
            return Vec::new();
        };
        let model = &leaf.selection;
        let n = model.n_items();
        let mut ids = Vec::new();
        for i in 0..n {
            if model.is_selected(i)
                && let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok())
            {
                ids.push(row.brief().id);
            }
        }
        ids
    }

    /// Install the leaf right-click context menu (Phase 16a, spec §3.1: every
    /// gesture has a keyboard equivalent, so each verb reuses an existing action /
    /// shortcut path). The `PopoverMenu` is parented to the leaf `ColumnView` once
    /// and re-pointed per click; the `win.`-prefixed actions drive the verbs.
    fn install_track_context_menu(&self) {
        let imp = self.imp();
        let Some(leaf) = imp.leaf.get() else {
            return;
        };

        // Play from the right-clicked row (the deadbeef "play from here"); the row
        // position is stashed by `show_track_context_menu`.
        let play = gio::SimpleAction::new("track-play", None);
        let weak = self.downgrade();
        play.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                let pos = win.imp().context_row.get().unwrap_or(0);
                win.play_leaf_from(pos);
            }
        });
        self.add_action(&play);

        let play_next = gio::SimpleAction::new("track-play-next", None);
        let weak = self.downgrade();
        play_next.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.play_next_selection();
            }
        });
        self.add_action(&play_next);

        let queue = gio::SimpleAction::new("track-queue", None);
        let weak = self.downgrade();
        queue.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.queue_append_selection();
            }
        });
        self.add_action(&queue);

        let edit = gio::SimpleAction::new("track-edit", None);
        let weak = self.downgrade();
        edit.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_bulk_edit();
            }
        });
        self.add_action(&edit);

        // Rating: a parameterised action, 0..=5, applied across the selection.
        let rate = gio::SimpleAction::new("track-rate", Some(glib::VariantTy::INT32));
        let weak = self.downgrade();
        rate.connect_activate(move |_, param| {
            if let (Some(win), Some(n)) = (weak.upgrade(), param.and_then(|p| p.get::<i32>())) {
                win.rate_selection(n);
            }
        });
        self.add_action(&rate);

        let reveal = gio::SimpleAction::new("track-reveal", None);
        let weak = self.downgrade();
        reveal.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.reveal_selected();
            }
        });
        self.add_action(&reveal);

        let remove = gio::SimpleAction::new("track-remove", None);
        let weak = self.downgrade();
        remove.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_remove_from_library();
            }
        });
        self.add_action(&remove);

        // Add to Playlist: a parameterised action (the target is the playlist id).
        let add_to_playlist =
            gio::SimpleAction::new("track-add-to-playlist", Some(glib::VariantTy::INT64));
        let weak = self.downgrade();
        add_to_playlist.connect_activate(move |_, param| {
            if let (Some(win), Some(id)) = (weak.upgrade(), param.and_then(|p| p.get::<i64>())) {
                win.append_selection_to_playlist(id);
            }
        });
        self.add_action(&add_to_playlist);

        // The model: Play / Add to Queue · Edit + Rating + Add to Playlist · Reveal.
        let menu = gio::Menu::new();
        let top = gio::Menu::new();
        top.append(Some("Play"), Some("win.track-play"));
        top.append(Some("Play Next"), Some("win.track-play-next"));
        top.append(Some("Add to Queue"), Some("win.track-queue"));
        menu.append_section(None, &top);

        let mid = gio::Menu::new();
        mid.append(Some("Edit\u{2026}"), Some("win.track-edit"));
        let ratings = gio::Menu::new();
        for n in 0i32..=5 {
            let label = if n == 0 {
                "No rating".to_string()
            } else {
                "\u{2605}".repeat(n as usize)
            };
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("win.track-rate"), Some(&n.to_variant()));
            ratings.append_item(&item);
        }
        mid.append_submenu(Some("Rating"), &ratings);
        // The Add-to-Playlist submenu is repopulated by `refresh_playlists`; stash
        // the gio::Menu so it can be rebuilt as playlists come and go.
        let add_menu = gio::Menu::new();
        mid.append_submenu(Some("Add to Playlist"), &add_menu);
        let _ = imp.add_to_playlist_menu.set(add_menu);
        menu.append_section(None, &mid);

        let bottom = gio::Menu::new();
        bottom.append(Some("Reveal in Files"), Some("win.track-reveal"));
        menu.append_section(None, &bottom);

        // A separate trailing section for the one destructive verb.
        let danger = gio::Menu::new();
        danger.append(
            Some("Remove from Library\u{2026}"),
            Some("win.track-remove"),
        );
        menu.append_section(None, &danger);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&leaf.column_view);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let _ = imp.track_menu.set(popover);
    }

    /// Pop the leaf context menu at the pointer (Phase 16a). A right-click on a row
    /// outside the current selection selects just that row first (the familiar
    /// file-manager behaviour); a right-click inside a multi-selection keeps it.
    fn show_track_context_menu(&self, pos: u32, x: f64, y: f64, cell: gtk::Widget) {
        let imp = self.imp();
        let (Some(leaf), Some(popover)) = (imp.leaf.get(), imp.track_menu.get()) else {
            return;
        };
        imp.context_row.set(Some(pos));
        if !leaf.selection.is_selected(pos) {
            leaf.selection.select_item(pos, true);
        }
        // The pointer arrives in the clicked cell's space; the popover is parented
        // to the ColumnView, so translate before pointing.
        let (cx, cy) = cell
            .compute_point(
                &leaf.column_view,
                &gtk::graphene::Point::new(x as f32, y as f32),
            )
            .map(|p| (p.x() as i32, p.y() as i32))
            .unwrap_or((x as i32, y as i32));
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(cx, cy, 1, 1)));
        popover.popup();
    }

    /// Install the facet-pane right-click menu (Phase 16a): Play / Play Next / Add
    /// to Queue over the facet's narrowed set. The popover is re-parented per click
    /// (each pane is a distinct ColumnView), so it is built parentless here.
    fn install_facet_context_menu(&self) {
        let imp = self.imp();

        let play = gio::SimpleAction::new("facet-play", None);
        let weak = self.downgrade();
        play.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.facet_apply(FacetVerb::Play);
            }
        });
        self.add_action(&play);

        let play_next = gio::SimpleAction::new("facet-play-next", None);
        let weak = self.downgrade();
        play_next.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.facet_apply(FacetVerb::PlayNext);
            }
        });
        self.add_action(&play_next);

        let queue = gio::SimpleAction::new("facet-queue", None);
        let weak = self.downgrade();
        queue.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.facet_apply(FacetVerb::Queue);
            }
        });
        self.add_action(&queue);

        let menu = gio::Menu::new();
        menu.append(Some("Play"), Some("win.facet-play"));
        menu.append(Some("Play Next"), Some("win.facet-play-next"));
        menu.append(Some("Add to Queue"), Some("win.facet-queue"));

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let _ = imp.facet_menu.set(popover);
    }

    /// Pop the facet context menu at the pointer (Phase 16a). Right-clicking a
    /// facet value selects just it (so the cascade narrows the leaf to it), then
    /// the menu's verbs act on that narrowing.
    fn show_facet_context_menu(&self, pane: usize, pos: u32, x: f64, y: f64, cell: gtk::Widget) {
        let imp = self.imp();
        let Some(popover) = imp.facet_menu.get() else {
            return;
        };
        {
            let panes = imp.panes.borrow();
            let Some(p) = panes.get(pane) else {
                return;
            };
            if !p.selection.is_selected(pos) {
                p.selection.select_item(pos, true);
            }
        }
        imp.context_pane.set(Some(pane));
        // The panes are distinct ColumnViews, so re-parent to the clicked one.
        if let Some(cv) = cell.ancestor(gtk::ColumnView::static_type()) {
            popover.unparent();
            popover.set_parent(&cv);
            if let Some(pt) =
                cell.compute_point(&cv, &gtk::graphene::Point::new(x as f32, y as f32))
            {
                popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                    pt.x() as i32,
                    pt.y() as i32,
                    1,
                    1,
                )));
            }
        }
        popover.popup();
    }

    /// Apply a facet verb (Phase 16a): narrow the leaf to the right-clicked facet
    /// value (synchronously, ahead of the debounced cascade), then act on the set.
    /// Play plays the whole narrowed leaf; the queue verbs select it all first and
    /// reuse the selection-based paths.
    fn facet_apply(&self, verb: FacetVerb) {
        let Some(pane) = self.imp().context_pane.get() else {
            return;
        };
        self.recompute_from(pane);
        match verb {
            FacetVerb::Play => self.play_leaf_from(0),
            FacetVerb::PlayNext => {
                self.select_all_leaf();
                self.play_next_selection();
            }
            FacetVerb::Queue => {
                self.select_all_leaf();
                self.queue_append_selection();
            }
        }
    }

    /// Select every row in the leaf (used by the facet queue verbs, which act on
    /// the whole narrowed set rather than a manual selection).
    fn select_all_leaf(&self) {
        if let Some(leaf) = self.imp().leaf.get() {
            leaf.selection.select_all();
        }
    }

    /// Apply a rating (0..=5) across the selection (Phase 16a). Reuses the bulk-edit
    /// path, so it goes through the same worker write; rating is not path-affecting,
    /// so no move is triggered.
    fn rate_selection(&self, rating: i32) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        match parse_assignment(&format!("rating={rating}")) {
            Ok(a) => self.apply_bulk_edit(&ids, vec![a]),
            Err(e) => eprintln!("conservatory: rating not applied: {e}"),
        }
    }

    /// Set one row's rating from a click on the star column (Phase 16b): write it
    /// through the worker and repaint just that row via its `rating` property, with
    /// no full reload. (The context-menu / bulk path stays `rate_selection`.)
    fn set_row_rating(&self, pos: u32, rating: u8) {
        let imp = self.imp();
        let (Some(leaf), Some(rt), Some(worker)) =
            (imp.leaf.get(), imp.runtime.get(), imp.worker.get())
        else {
            return;
        };
        let Some(row) = leaf.selection.item(pos).and_downcast::<TrackRow>() else {
            return;
        };
        let id = row.brief().id;
        let Ok(a) = parse_assignment(&format!("rating={rating}")) else {
            return;
        };
        let edit = build_track_edit(&[a]);
        let _ = rt.block_on(worker.update_track(id, edit));
        row.update_rating(rating);
        // Keep the inspector's Rating field in step if it is showing this row.
        self.refresh_inspector();
    }

    /// Open the first selected track's folder in the file manager (Phase 16a).
    fn reveal_selected(&self) {
        let imp = self.imp();
        let (Some(pool), Some(root)) = (imp.pool.get(), imp.library_root.get()) else {
            return;
        };
        let ids = self.selected_track_ids();
        let Some(&id) = ids.first() else {
            return;
        };
        let Ok(conn) = pool.open() else {
            return;
        };
        let track = get_track(&conn, id).ok().flatten();
        drop(conn);
        let Some(track) = track else {
            return;
        };
        let path = root.join(&track.file_path);
        let target = path.parent().unwrap_or(&path);
        if let Err(e) = std::process::Command::new("xdg-open").arg(target).spawn() {
            eprintln!("conservatory: could not reveal {}: {e}", target.display());
        }
    }

    /// Confirm and remove the selection from the library (Phase 16a). Destructive
    /// (drops the DB rows), but the files stay on disk and it is re-importable, so
    /// the confirm defaults to Cancel rather than blocking hard.
    fn prompt_remove_from_library(&self) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        let body = format!(
            "Remove {} track(s) from the library? The files stay on disk, so you can re-import them.",
            ids.len()
        );
        let dialog = adw::AlertDialog::new(Some("Remove from library?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp == "remove"
                && let Some(win) = weak.upgrade()
            {
                win.remove_from_library(&ids);
            }
        });
        dialog.present(Some(self));
    }

    /// Delete the given tracks from the library through the worker, then refresh.
    /// The queue rows cascade away (`ON DELETE CASCADE`) and the playback cursor
    /// nulls (`ON DELETE SET NULL`); a live engine item keeps its own resolved
    /// path, so playback in progress is unaffected.
    fn remove_from_library(&self, ids: &[i64]) {
        let imp = self.imp();
        let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
            return;
        };
        for &id in ids {
            let _ = rt.block_on(worker.delete_track(id));
        }
        self.toast(&format!("Removed {} track(s) from the library", ids.len()));
        self.populate_initial();
        self.reload_queue_panel();
    }

    /// The value shared across the selection for each bulk-edit field, or `None`
    /// when the tracks differ ("multiple values"), for pre-filling the dialog
    /// (Phase 16c). Best-effort: an unreadable source yields an all-empty common.
    /// Order is irrelevant (the collapse only checks for agreement).
    fn bulk_edit_commons(
        &self,
        ids: &[i64],
    ) -> std::collections::HashMap<&'static str, Option<String>> {
        use std::collections::{HashMap, HashSet};

        let mut out: HashMap<&'static str, Option<String>> = HashMap::new();
        let imp = self.imp();
        let (Some(pool), Some(leaf)) = (imp.pool.get(), imp.leaf.get()) else {
            return out;
        };
        let Ok(conn) = pool.open() else {
            return out;
        };
        let idset: HashSet<i64> = ids.iter().copied().collect();

        // Album-level (album artist, album, year, shelf genre) + track fields from
        // the render rows; the album artist display is resolved once per album.
        let (mut albumartist, mut album, mut year, mut shelfgenre, mut artist, mut title) =
            (vec![], vec![], vec![], vec![], vec![], vec![]);
        let mut aa: HashMap<i64, String> = HashMap::new();
        for r in track_render_rows(&conn)
            .unwrap_or_default()
            .iter()
            .filter(|r| idset.contains(&r.track_id))
        {
            let aa_name = match r.album_id {
                Some(aid) => aa
                    .entry(aid)
                    .or_insert_with(|| {
                        get_album(&conn, aid)
                            .ok()
                            .flatten()
                            .and_then(|al| al.album_artist_id)
                            .and_then(|artist_id| get_artist(&conn, artist_id).ok().flatten())
                            .map(|ar| ar.name)
                            .unwrap_or_default()
                    })
                    .clone(),
                None => String::new(),
            };
            albumartist.push(aa_name);
            album.push(r.album.clone().unwrap_or_default());
            year.push(r.year.map(|y| y.to_string()).unwrap_or_default());
            shelfgenre.push(r.shelf_genre.clone().unwrap_or_default());
            artist.push(r.track_artist.clone().unwrap_or_default());
            title.push(r.title.clone());
        }
        drop(conn);

        // Genres and rating live on the leaf briefs, not the render row.
        let (mut genre, mut rating) = (vec![], vec![]);
        let model = &leaf.selection;
        for i in 0..model.n_items() {
            if model.is_selected(i)
                && let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok())
            {
                let b = row.brief();
                genre.push(b.genres);
                rating.push(b.rating.to_string());
            }
        }

        out.insert("albumartist", common_value(albumartist));
        out.insert("album", common_value(album));
        out.insert("year", common_value(year));
        out.insert("shelfgenre", common_value(shelfgenre));
        out.insert("artist", common_value(artist));
        out.insert("title", common_value(title));
        out.insert("genre", common_value(genre));
        out.insert("rating", common_value(rating));
        out
    }

    /// The bulk-edit dialog (Phase 5a-ii, spec §3.5; Phase 16c checkboxes + mixed
    /// values): a checkbox, label, and entry per field. The entry pre-fills the
    /// value shared across the selection, or reads "multiple values" when they
    /// differ (the foobar/MusicBee affordance). Only ticked fields are written;
    /// editing a field ticks it. Path-affecting edits are confirmed with a move
    /// preview after the values are written.
    fn prompt_bulk_edit(&self) {
        self.prompt_bulk_edit_prefilled(None);
    }

    /// The dialog body, optionally pre-filled from a rejected attempt (Phase
    /// 16.5a): after a parse failure the dialog re-presents with the entered
    /// values and tick states intact, so fixing one bad field loses nothing.
    fn prompt_bulk_edit_prefilled(&self, prefill: Option<Vec<(String, bool, String)>>) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        let commons = self.bulk_edit_commons(&ids);
        let prefill: Option<std::collections::HashMap<String, (bool, String)>> = prefill.map(|p| {
            p.into_iter()
                .map(|(key, ticked, value)| (key, (ticked, value)))
                .collect()
        });

        // (field key as `edit::Field::parse` accepts it, display label).
        let fields: [(&str, &str); 8] = [
            ("albumartist", "Album artist"),
            ("album", "Album"),
            ("year", "Year"),
            ("shelfgenre", "Shelf genre"),
            ("artist", "Artist (track)"),
            ("title", "Title"),
            ("genre", "Genres (; separated)"),
            ("rating", "Rating (0-5)"),
        ];
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        let mut entries: Vec<(String, gtk::CheckButton, gtk::Entry)> = Vec::new();
        for (r, (key, label)) in fields.iter().enumerate() {
            let check = gtk::CheckButton::builder()
                .tooltip_text("Write this field to every selected track (overwrites differing values)")
                .valign(gtk::Align::Center)
                .build();
            let lbl = gtk::Label::builder().label(*label).xalign(1.0).build();
            let entry = gtk::Entry::builder().hexpand(true).build();
            // Pre-fill the shared value; hint "multiple values" when they differ.
            match commons.get(*key).cloned().flatten() {
                Some(v) if !v.is_empty() => {
                    entry.set_text(&v);
                    entry.set_placeholder_text(Some("unchanged"));
                }
                Some(_) => entry.set_placeholder_text(Some("unchanged")),
                None => entry.set_placeholder_text(Some("multiple values")),
            }
            // A rejected attempt's entered text overrides the shared value.
            let prefilled = prefill.as_ref().and_then(|p| p.get(*key));
            if let Some((_, value)) = prefilled {
                entry.set_text(value);
            }
            // Editing a field ticks its checkbox. Connected after the pre-fill so
            // the initial `set_text` does not tick it.
            let check_edit = check.clone();
            entry.connect_changed(move |_| check_edit.set_active(true));
            // Restore the attempt's tick state (after the changed hook, so an
            // untouched-but-prefilled field stays authoritative either way).
            if let Some((ticked, _)) = prefilled {
                check.set_active(*ticked);
            }
            grid.attach(&check, 0, r as i32, 1, 1);
            grid.attach(&lbl, 1, r as i32, 1, 1);
            grid.attach(&entry, 2, r as i32, 1, 1);
            entries.push(((*key).to_string(), check, entry));
        }

        let dialog = adw::AlertDialog::new(
            Some("Edit metadata"),
            Some(&format!(
                "Apply to {} selected track(s). Tick a field to write it; shared values are \
                 shown, differing ones read \u{201c}multiple values\u{201d}.",
                ids.len()
            )),
        );
        dialog.set_extra_child(Some(&grid));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("apply", "Apply");
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "apply" {
                return;
            }
            let Some(win) = weak.upgrade() else { return };
            let entered: Vec<(String, bool, String)> = entries
                .iter()
                .map(|(key, check, entry)| {
                    (key.clone(), check.is_active(), entry.text().to_string())
                })
                .collect();
            let (assignments, errors) = collect_assignments(&entered);
            if !errors.is_empty() {
                // Reject the whole set rather than apply a partly-valid edit;
                // the error dialog re-presents this one pre-filled (16.5a).
                win.present_bulk_edit_errors(errors, entered);
                return;
            }
            if !assignments.is_empty() {
                win.apply_bulk_edit(&ids, assignments);
            }
        });
        dialog.present(Some(self));
    }

    /// List the parse failures that rejected a bulk edit, then reopen the edit
    /// dialog pre-filled with the attempt so the fix loses nothing (16.5a).
    fn present_bulk_edit_errors(&self, errors: Vec<String>, entered: Vec<(String, bool, String)>) {
        let dialog = adw::AlertDialog::new(Some("Edit not applied"), Some(&errors.join("\n")));
        dialog.add_response("ok", "Fix Values");
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_bulk_edit_prefilled(Some(entered.clone()));
            }
        });
        dialog.present(Some(self));
    }

    /// Apply parsed assignments across the selected tracks through the worker,
    /// then handle a path-affecting move and refresh the browse.
    fn apply_bulk_edit(&self, track_ids: &[i64], assignments: Vec<Assignment>) {
        let imp = self.imp();
        let (Some(rt), Some(worker), Some(pool)) =
            (imp.runtime.get(), imp.worker.get(), imp.pool.get())
        else {
            return;
        };

        // Map the selected tracks to their distinct albums (for album-level edits
        // and the scoped move) via the render rows.
        let albums: Vec<i64> = {
            let Ok(conn) = pool.open() else { return };
            let rows = track_render_rows(&conn).unwrap_or_default();
            let idset: std::collections::HashSet<i64> = track_ids.iter().copied().collect();
            let mut albums = Vec::new();
            for r in &rows {
                if idset.contains(&r.track_id)
                    && let Some(a) = r.album_id
                    && !albums.contains(&a)
                {
                    albums.push(a);
                }
            }
            albums
        };

        let track_edit = build_track_edit(&assignments);
        let album_edit = build_album_edit(&assignments);
        let genres = genres_assignment(&assignments);

        if !track_edit.is_empty() {
            for &tid in track_ids {
                let _ = rt.block_on(worker.update_track(tid, track_edit.clone()));
            }
        }
        if let Some(g) = &genres {
            for &tid in track_ids {
                let _ = rt.block_on(worker.set_track_genres(tid, g.clone()));
            }
        }
        if !album_edit.is_empty() {
            for &aid in &albums {
                let _ = rt.block_on(worker.update_album(aid, album_edit.clone()));
            }
        }

        self.toast(&format!("Updated {} track(s)", track_ids.len()));

        if any_path_affecting(&assignments) {
            match imp.library_root.get() {
                Some(root) => {
                    self.confirm_and_move(&albums, root.clone());
                    return; // the confirm dialog refreshes when it closes
                }
                None => eprintln!(
                    "conservatory: the edit changed the tree layout, but no library root \
                     is set (launch as `conservatory <db> <root>`); files not moved"
                ),
            }
        }
        self.populate_initial();
    }

    /// Preview the move a path-affecting edit implies and, on confirm, run it.
    fn confirm_and_move(&self, albums: &[i64], root: std::path::PathBuf) {
        let imp = self.imp();
        let (Some(pool), Some(rt), Some(worker)) =
            (imp.pool.get(), imp.runtime.get(), imp.worker.get())
        else {
            return;
        };
        let _ = rt.block_on(mover::recover(worker, pool));

        let preview = mover::plan(self.build_scoped_ops(albums, &root));
        if preview.ops.is_empty() {
            self.populate_initial();
            return;
        }
        let body = if preview.conflicts.is_empty() {
            format!("{} file(s) will move to match the edit.", preview.ops.len())
        } else {
            format!(
                "{} file(s) will move; {} conflict(s) will be skipped.",
                preview.ops.len(),
                preview.conflicts.len()
            )
        };
        let dialog = adw::AlertDialog::new(Some("Move files?"), Some(&body));
        dialog.add_response("cancel", "Keep in place");
        dialog.add_response("move", "Move");
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("move"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        let albums = albums.to_vec();
        dialog.connect_response(None, move |_, resp| {
            let Some(win) = weak.upgrade() else { return };
            if resp == "move" {
                win.run_scoped_move(&albums, &root);
            }
            win.populate_initial();
        });
        dialog.present(Some(self));
    }

    /// Build the `organize` move ops for the given albums (re-render from the DB).
    fn build_scoped_ops(&self, albums: &[i64], root: &std::path::Path) -> Vec<MoveOp> {
        let imp = self.imp();
        let Some(pool) = imp.pool.get() else {
            return Vec::new();
        };
        let Ok(conn) = pool.open() else {
            return Vec::new();
        };
        let rows = track_render_rows(&conn).unwrap_or_default();
        organize_ops(&rows, root, Some(albums))
    }

    fn run_scoped_move(&self, albums: &[i64], root: &std::path::Path) {
        let imp = self.imp();
        let (Some(rt), Some(worker), Some(pool)) =
            (imp.runtime.get(), imp.worker.get(), imp.pool.get())
        else {
            return;
        };
        let ops = self.build_scoped_ops(albums, root);
        let created_at = chrono::Utc::now().timestamp();
        // Moving files is the headline risk (CLAUDE.md): never fail silently. The
        // move is journaled + roll-forward-recoverable, so surface the error and
        // let the user retry rather than swallow it.
        if let Err(e) = rt.block_on(mover::apply(
            worker,
            pool,
            MoveKind::Organize,
            MoveMode::Move,
            root,
            created_at,
            ops,
        )) {
            self.error_dialog("Move failed", &e.to_string());
            return;
        }
        // Covers follow their albums after the move (Phase 5d).
        let _ = rt.block_on(conservatory_core::covers::resync_album_covers(
            worker, pool, root,
        ));
    }

    /// Present a simple error dialog (used for the file-move failure path).
    fn error_dialog(&self, title: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(title), Some(body));
        dialog.add_response("ok", "OK");
        dialog.present(Some(self));
    }

    /// Embed the curated DB metadata into the selected files (Phase 5b-ii, spec
    /// §5.5): an explicit action (not auto-on-edit), behind a confirm.
    fn prompt_embed_tags(&self) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        if self.imp().library_root.get().is_none() {
            let dialog = adw::AlertDialog::new(
                Some("No library root"),
                Some("Launch as `conservatory <db> <root>` to write tags into the files."),
            );
            dialog.add_response("ok", "OK");
            dialog.present(Some(self));
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("Embed metadata into files?"),
            Some(&format!(
                "Write the database metadata into {} file(s) on disk. The files become \
                 self-describing; the database stays the source of truth.",
                ids.len()
            )),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("write", "Write");
        dialog.set_response_appearance("write", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("write"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "write" {
                return;
            }
            if let Some(win) = weak.upgrade() {
                win.run_embed_tags(&ids);
            }
        });
        dialog.present(Some(self));
    }

    fn run_embed_tags(&self, ids: &[i64]) {
        let imp = self.imp();
        let (Some(pool), Some(root)) = (imp.pool.get(), imp.library_root.get()) else {
            return;
        };
        let rows = {
            let Ok(conn) = pool.open() else { return };
            writeback_rows(&conn, ids).unwrap_or_default()
        };
        let (mut written, mut errors) = (0usize, 0usize);
        for r in &rows {
            match write_track_tags(&root.join(&r.file_path), &TagWrite::from(r)) {
                Ok(()) => written += 1,
                Err(e) => {
                    eprintln!("embed-tags: {}: {e}", r.file_path);
                    errors += 1;
                }
            }
        }
        // A toast rather than a modal "Done" dialog (Phase 13b): a successful
        // write needs an acknowledgement, not an interruption. Errors still get a
        // toast (the per-file detail is on the terminal).
        let body = if errors == 0 {
            format!("Embedded tags into {written} file(s)")
        } else {
            format!("Embedded {written} file(s); {errors} failed (see terminal)")
        };
        self.toast(&body);
    }

    /// Show a brief, non-modal confirmation (Phase 13b). A no-op before the
    /// overlay is built (it is set at the end of `build_contents`).
    fn toast(&self, message: &str) {
        if let Some(overlay) = self.imp().toast_overlay.get() {
            overlay.add_toast(adw::Toast::new(message));
        }
    }

    /// Keep the drawer's playing-row highlight in step with the engine: when the
    /// current index changes, update the shared cell and (if the drawer is open)
    /// repopulate so the factory restyles.
    fn refresh_queue_highlight(&self) {
        let imp = self.imp();
        let (Some(player), Some(cur)) = (imp.player.get(), imp.queue_current.get()) else {
            return;
        };
        let snap = player.snapshot();
        let want = if snap.ended {
            None
        } else {
            snap.current_index.map(|i| i as i64)
        };
        if cur.get() != want {
            cur.set(want);
            if let Some(panel) = imp.queue_panel.get()
                && panel.revealer.reveals_child()
            {
                self.reload_queue_panel();
            }
        }
    }

    /// A drag-and-drop reorder finished: apply `(from, to)` to both the DB queue
    /// and the live engine queue (identical, so positions stay aligned), then
    /// repopulate.
    fn on_queue_reorder(&self, from: usize, to: usize) {
        let imp = self.imp();
        if let (Some(rt), Some(worker), Some(player)) =
            (imp.runtime.get(), imp.worker.get(), imp.player.get())
        {
            let _ = rt.block_on(worker.reorder_queue(from as i64, to as i64));
            player.move_item(from, to);
            // The highlight follows on the next snapshot poll (the engine's
            // current_index shifts in lock-step with the DB positions).
        }
        self.reload_queue_panel();
    }

    /// Remove the selected queue row from the DB and the engine.
    fn queue_remove_selected(&self) {
        let imp = self.imp();
        let (Some(rt), Some(worker), Some(player), Some(panel)) = (
            imp.runtime.get(),
            imp.worker.get(),
            imp.player.get(),
            imp.queue_panel.get(),
        ) else {
            return;
        };
        let sel = panel.selection.selected();
        if sel == gtk::INVALID_LIST_POSITION {
            return;
        }
        let _ = rt.block_on(worker.remove_queue_item(sel as i64));
        player.remove_item(sel as usize);
        self.reload_queue_panel();
    }

    /// Move the selected queue row by `delta` (the `Alt+↑/↓` reorder).
    fn queue_move_selected(&self, delta: i32) {
        let panel = match self.imp().queue_panel.get() {
            Some(p) => p,
            None => return,
        };
        let sel = panel.selection.selected();
        let len = panel.store.n_items();
        if sel == gtk::INVALID_LIST_POSITION || len == 0 {
            return;
        }
        let to = (sel as i32 + delta).clamp(0, len as i32 - 1) as u32;
        if to != sel {
            self.on_queue_reorder(sel as usize, to as usize);
            panel.selection.set_selected(to);
        }
    }

    /// Clear the queue (DB + engine) and stop playback.
    fn queue_clear(&self) {
        let imp = self.imp();
        if let (Some(rt), Some(worker), Some(player)) =
            (imp.runtime.get(), imp.worker.get(), imp.player.get())
        {
            let _ = rt.block_on(worker.clear_queue());
            player.clear_queue();
            if let Some(cur) = imp.queue_current.get() {
                cur.set(None);
            }
        }
        self.reload_queue_panel();
    }

    /// Install the queue-drawer right-click menu (Phase 16a): Remove from Queue /
    /// Clear Queue, reusing the keyboard-op methods (which read the selection). The
    /// popover is parented to the single, stable queue `ListView`.
    fn install_queue_context_menu(&self, list: &gtk::ListView) {
        let imp = self.imp();

        let remove = gio::SimpleAction::new("queue-remove", None);
        let weak = self.downgrade();
        remove.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.queue_remove_selected();
            }
        });
        self.add_action(&remove);

        let clear = gio::SimpleAction::new("queue-clear", None);
        let weak = self.downgrade();
        clear.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.queue_clear();
            }
        });
        self.add_action(&clear);

        let menu = gio::Menu::new();
        menu.append(Some("Remove from Queue"), Some("win.queue-remove"));
        let danger = gio::Menu::new();
        danger.append(Some("Clear Queue"), Some("win.queue-clear"));
        menu.append_section(None, &danger);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(list);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let _ = imp.queue_menu.set(popover);
    }

    /// Pop the queue context menu at the pointer (Phase 16a). Right-clicking a row
    /// selects it (the reused verbs act on the selection).
    fn show_queue_context_menu(&self, pos: u32, x: f64, y: f64, cell: gtk::Widget) {
        let imp = self.imp();
        let (Some(panel), Some(popover)) = (imp.queue_panel.get(), imp.queue_menu.get()) else {
            return;
        };
        panel.selection.set_selected(pos);
        let (cx, cy) = cell
            .compute_point(&panel.list, &gtk::graphene::Point::new(x as f32, y as f32))
            .map(|p| (p.x() as i32, p.y() as i32))
            .unwrap_or((x as i32, y as i32));
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(cx, cy, 1, 1)));
        popover.popup();
    }

    /// Wire the queue keyboard shortcuts: `Ctrl+U` toggles the drawer (global);
    /// `Alt+↑/↓` reorder, `Delete` removes, `Ctrl+Shift+C` clears (on the list).
    fn install_queue_keys(&self, list: &gtk::ListView) {
        let global = gtk::ShortcutController::new();
        global.set_scope(gtk::ShortcutScope::Global);
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>u"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.toggle_queue();
                }
                glib::Propagation::Stop
            })),
        ));
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>i"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.toggle_now_playing();
                }
                glib::Propagation::Stop
            })),
        ));
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>comma"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.open_preferences();
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+P toggles the track properties inspector (Phase 11a).
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>p"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.toggle_inspector();
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+M toggles stop-after-current; Ctrl+J jumps to the playing track
        // (Phase 11d). Both also live in the header primary menu.
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>m"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.toggle_stop_after_current();
                }
                glib::Propagation::Stop
            })),
        ));
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>j"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.jump_to_current();
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+Shift+→/← skip to the next / previous chapter of the current item
        // (Phase 6c-iii-b); a no-op when it has no chapters.
        for (trigger, dir) in [
            ("<Control><Shift>Right", 1_i32),
            ("<Control><Shift>Left", -1_i32),
        ] {
            let weak = self.downgrade();
            global.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade()
                        && let Some(player) = win.imp().player.get()
                    {
                        player.skip_chapter(dir);
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        // Playback / navigation keys (Phase 13e-ii). Ctrl-modified so they do not
        // collide with list navigation or type-ahead; Space (play/pause) is the
        // separate capture controller below because it must beat list selection.
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>Right"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.next();
                }
                glib::Propagation::Stop
            })),
        ));
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>Left"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    player.previous();
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+Up / Ctrl+Down nudge the volume by 5 (clamped 0..=100); a manual
        // change also clears any active mute.
        for (trigger, delta) in [("<Control>Up", 5_i64), ("<Control>Down", -5_i64)] {
            let weak = self.downgrade();
            global.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade()
                        && let Some(player) = win.imp().player.get()
                    {
                        let vol = (player.snapshot().volume + delta).clamp(0, 100);
                        player.set_volume(vol);
                        win.imp().pre_mute_volume.set(None);
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        // Ctrl+0 toggles mute, remembering the level to restore.
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>0"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade()
                    && let Some(player) = win.imp().player.get()
                {
                    if let Some(prev) = win.imp().pre_mute_volume.take() {
                        player.set_volume(prev);
                    } else {
                        win.imp()
                            .pre_mute_volume
                            .set(Some(player.snapshot().volume));
                        player.set_volume(0);
                    }
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+L clears the filter bar (its coalescer re-runs the query).
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>l"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade()
                    && let Some(entry) = win.imp().filter_entry.get()
                {
                    entry.set_text("");
                }
                glib::Propagation::Stop
            })),
        ));
        // Ctrl+Q quits.
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>q"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    match win.application() {
                        Some(app) => app.quit(),
                        None => win.close(),
                    }
                }
                glib::Propagation::Stop
            })),
        ));
        // F1 opens the keyboard-shortcuts reference (Phase 13e-iii).
        let weak = self.downgrade();
        global.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("F1"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.show_shortcuts_window();
                }
                glib::Propagation::Stop
            })),
        ));
        self.add_controller(global);

        // `S` pops the Now-bar sleep-timer menu (Phase 6c-iii-d). A window-local
        // (non-global) controller so the bare letter does not fire while the filter
        // SearchEntry has focus: the entry consumes the keypress first.
        let sleep_keys = gtk::ShortcutController::new();
        let weak = self.downgrade();
        sleep_keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("s"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(win) = weak.upgrade()
                    && let Some(now) = win.imp().now_bar.get()
                    && now.sleep_btn.is_visible()
                {
                    now.sleep_btn.popup();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            })),
        ));
        self.add_controller(sleep_keys);

        // Space toggles play/pause everywhere except a text entry (Phase 13e-ii,
        // the foobar2000 rule). A capture-phase key controller so it intercepts
        // Space before a focused list consumes it for selection-toggle; it yields
        // to editables by checking the focused widget and proceeding when it is a
        // `gtk::Text` (the inner widget of every entry).
        let space = gtk::EventControllerKey::new();
        space.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.downgrade();
        space.connect_key_pressed(move |_, keyval, _, state| {
            if keyval != gtk::gdk::Key::space
                || state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            {
                return glib::Propagation::Proceed;
            }
            let Some(win) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if gtk::prelude::GtkWindowExt::focus(&win).is_some_and(|w| w.is::<gtk::Text>()) {
                return glib::Propagation::Proceed;
            }
            if let Some(player) = win.imp().player.get() {
                player.toggle_pause();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.add_controller(space);

        let local = gtk::ShortcutController::new();
        for (trigger, action) in [
            ("<Alt>Up", QueueKey::MoveUp),
            ("<Alt>Down", QueueKey::MoveDown),
            ("Delete", QueueKey::Remove),
            ("<Control><Shift>c", QueueKey::Clear),
        ] {
            let weak = self.downgrade();
            local.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade() {
                        match action {
                            QueueKey::MoveUp => win.queue_move_selected(-1),
                            QueueKey::MoveDown => win.queue_move_selected(1),
                            QueueKey::Remove => win.queue_remove_selected(),
                            QueueKey::Clear => win.queue_clear(),
                        }
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        list.add_controller(local);
    }

    /// Add the lazily-built Podcasts page to the view stack (Phase 6b-i). The
    /// shared multi-view chrome (switcher, bottom bar, breakpoint) is built once
    /// by [`install_view_chrome`], so this only owns the page. Compiled only with
    /// the `podcasts` feature.
    #[cfg(feature = "podcasts")]
    fn attach_podcasts_view(&self, stack: &adw::ViewStack) {
        // Lazy construction (spec §2.3): the page's child is built on its first
        // `::map`, not eagerly at startup, so switching to it is what pays for
        // it. Reads go through the pool; triage actions write through the worker
        // (dispatched on the runtime, the GUI write idiom).
        let podcasts_bin = adw::Bin::new();
        let built = Cell::new(false);
        let weak = self.downgrade();
        podcasts_bin.connect_map(move |bin| {
            if built.replace(true) {
                return;
            }
            if let Some(win) = weak.upgrade() {
                let imp = win.imp();
                if let (Some(pool), Some(worker), Some(rt)) =
                    (imp.pool.get().cloned(), imp.worker.get(), imp.runtime.get())
                {
                    bin.set_child(Some(&crate::ui::podcasts::build_podcasts_view(
                        pool,
                        worker.clone(),
                        rt.handle().clone(),
                        imp.player.get().cloned(),
                        imp.library_root.get().cloned(),
                    )));
                }
            }
        });
        stack.add_titled_with_icon(
            &podcasts_bin,
            Some("podcasts"),
            "Podcasts",
            "microphone-symbolic",
        );
    }

    /// Add the lazily-built Audiobooks page to the view stack (Phase 7b-i), the
    /// `attach_podcasts_view` mirror. The cover shelf + detail pane are built on
    /// first `::map`; the shared chrome is [`install_view_chrome`]'s job. Compiled
    /// only with the `audiobooks` feature.
    #[cfg(feature = "audiobooks")]
    fn attach_audiobooks_view(&self, stack: &adw::ViewStack) {
        let books_bin = adw::Bin::new();
        let built = Cell::new(false);
        let weak = self.downgrade();
        books_bin.connect_map(move |bin| {
            if built.replace(true) {
                return;
            }
            if let Some(win) = weak.upgrade() {
                let imp = win.imp();
                if let (Some(pool), Some(worker), Some(rt)) =
                    (imp.pool.get().cloned(), imp.worker.get(), imp.runtime.get())
                {
                    bin.set_child(Some(&crate::ui::audiobooks::build_audiobooks_view(
                        pool,
                        worker.clone(),
                        rt.handle().clone(),
                        imp.player.get().cloned(),
                        imp.library_root.get().cloned(),
                    )));
                }
            }
        });
        stack.add_titled_with_icon(
            &books_bin,
            Some("audiobooks"),
            "Audiobooks",
            "library-symbolic",
        );
    }

    /// Build the shared multi-view chrome (Phase 6b-i): the header view switcher,
    /// the adaptive bottom switcher bar, and the narrow breakpoint. Installed once
    /// when *any* second view (podcasts or audiobooks) is compiled in; a music-only
    /// build (`--no-default-features`) keeps a single-page stack with no switcher.
    #[cfg(any(feature = "podcasts", feature = "audiobooks"))]
    fn install_view_chrome(
        &self,
        stack: &adw::ViewStack,
        header: &adw::HeaderBar,
        toolbar: &adw::ToolbarView,
    ) {
        // The header switcher (libadwaita 1.4+ idiom; AdwViewSwitcherTitle is
        // deprecated and not used). `Wide` keeps the labels until the breakpoint.
        let switcher = adw::ViewSwitcher::builder()
            .stack(stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        header.set_title_widget(Some(&switcher));

        // The adaptive bottom bar: hidden when wide, revealed beneath the Now-bar
        // at the narrow breakpoint (the spec §2.3 stacking call).
        let switcher_bar = adw::ViewSwitcherBar::builder().stack(stack).build();
        toolbar.add_bottom_bar(&switcher_bar);

        let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            550.0,
            adw::LengthUnit::Sp,
        ));
        breakpoint.add_setter(header, "visible", Some(&false.to_value()));
        breakpoint.add_setter(&switcher_bar, "reveal", Some(&true.to_value()));
        self.add_breakpoint(breakpoint);
    }

    /// `Alt+1/2/3` switch the top-level view (spec §2.3; the AdwTabView `Alt+N`
    /// convention, leaving `Ctrl+N` free for the 6b-ii triage lists).
    fn install_view_keys(&self) {
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);
        for n in 1u8..=3 {
            let weak = self.downgrade();
            controller.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(&format!("<Alt>{n}")),
                Some(gtk::CallbackAction::new(move |_, _| {
                    if let Some(win) = weak.upgrade()
                        && let Some(name) = view_page_name(n)
                    {
                        win.switch_view(name);
                    }
                    glib::Propagation::Stop
                })),
            ));
        }
        self.add_controller(controller);
    }

    /// Switch to a named view if it exists (a no-op for a page not compiled in,
    /// e.g. `Alt+2` in a music-only build).
    fn switch_view(&self, name: &str) {
        if let Some(stack) = self.imp().view_stack.get()
            && stack.child_by_name(name).is_some()
        {
            stack.set_visible_child_name(name);
        }
    }

    /// Show the music-only header controls (Edit / Embed tags / Properties) on the
    /// Music tab and hide them on Podcasts / Audiobooks, where selection editing
    /// and the track-properties inspector do not apply (Phase 16f). Only built when
    /// a second tab is compiled in (a music-only build never switches views).
    #[cfg(any(feature = "podcasts", feature = "audiobooks"))]
    fn update_header_for_view(&self) {
        let imp = self.imp();
        let is_music = imp
            .view_stack
            .get()
            .and_then(|s| s.visible_child_name())
            .map(|n| n == "music")
            .unwrap_or(true);
        if let Some(g) = imp.header_edit_group.get() {
            g.set_visible(is_music);
        }
        if let Some(b) = imp.header_props_btn.get() {
            b.set_visible(is_music);
        }
    }

    /// Refresh the Now-bar from the player snapshot (the 250 ms poll). Title and
    /// artist re-render only when the track changes; position/seek/icon every tick.
    fn refresh_now_bar(&self) {
        let imp = self.imp();
        let (Some(player), Some(now)) = (imp.player.get(), imp.now_bar.get()) else {
            return;
        };
        let snap = player.snapshot();

        // Gate the spectrum capture on real playback (Phase 12d isolation): the tap
        // targets our own mpv output node, which exists only while audio flows, so
        // the visualizer reacts to Conservatory alone and never the microphone.
        if let Some(panel) = imp.now_playing.get() {
            let playing =
                snap.track_id.is_some() && !snap.paused && !snap.ended && !snap.buffering;
            panel.set_playing(playing);
        }

        // Keep the stop-after-current toggle's checkmark in step with the engine,
        // which disarms the flag once the boundary fires (Phase 11d).
        if let Some(a) = imp.stop_action.get() {
            let cur = a.state().and_then(|v| v.get::<bool>()).unwrap_or(false);
            if cur != snap.stop_after_current {
                a.set_state(&snap.stop_after_current.to_variant());
            }
        }

        if snap.ended || snap.track_id.is_none() {
            if imp.last_shown.get().is_some() {
                imp.last_shown.set(None);
                imp.last_index.set(None);
                now.clear();
                self.refresh_now_playing(None, None);
                *imp.tech_static.borrow_mut() = (None, None, None);
                if let Some(label) = imp.status_left.get() {
                    label.set_text("");
                }
            }
            return;
        }

        // Detect an item change on the queue slot as well as the id: a track and
        // an episode can share an id, so `track_id` alone missed some advances and
        // left the drawer / Now-bar showing the previous song.
        if imp.last_shown.get() != snap.track_id || imp.last_index.get() != snap.current_index {
            imp.last_shown.set(snap.track_id);
            imp.last_index.set(snap.current_index);
            if let Some(id) = snap.track_id {
                // Resolve title / artist / cover by (kind, id). A track and an
                // episode share the snapshot's id field, so the kind decides which
                // read to use: without this an episode read nothing from
                // `track_metadata` and the Now-bar kept the previous song's cover.
                let np =
                    imp.pool
                        .get()
                        .and_then(|pool| pool.open().ok())
                        .and_then(|conn| match snap.kind {
                            Some(MediaKind::Episode) => {
                                conservatory_core::db::episode_metadata(&conn, id)
                                    .ok()
                                    .flatten()
                            }
                            Some(MediaKind::Audiobook) => {
                                conservatory_core::db::book_metadata(&conn, id)
                                    .ok()
                                    .flatten()
                            }
                            _ => conservatory_core::db::track_metadata(&conn, id)
                                .ok()
                                .flatten(),
                        });
                let (title, artist, album, cover, accent) = match np {
                    Some(np) => (
                        np.title,
                        np.artist.unwrap_or_default(),
                        np.album,
                        np.album_cover_path,
                        np.album_accent_rgb,
                    ),
                    None => ("\u{2014}".to_string(), String::new(), None, None, None),
                };
                now.title.set_text(&title);
                now.artist.set_text(&crate::ui::now_bar::now_bar_subtitle(
                    &artist,
                    album.as_deref(),
                ));
                let abs = match (imp.library_root.get(), cover) {
                    (Some(root), Some(cp)) => Some(root.join(cp)),
                    _ => None,
                };
                now.set_cover(abs.as_deref(), accent);
                // Keep the Now Playing drawer in step with the new item.
                self.refresh_now_playing(snap.kind, Some(id));
                // Cache the playing track's static tech fields for the status bar
                // (Phase 11b). Only tracks carry these DB columns; an episode /
                // book leaves the line blank (channels still folds in per tick).
                *imp.tech_static.borrow_mut() = match snap.kind {
                    Some(MediaKind::Track) | None => imp
                        .pool
                        .get()
                        .and_then(|pool| pool.open().ok())
                        .and_then(|conn| conservatory_core::db::get_track(&conn, id).ok().flatten())
                        .map(|t| (t.format, t.sample_rate, t.bitrate))
                        .unwrap_or((None, None, None)),
                    _ => (None, None, None),
                };
            }
        }

        now.play_btn.set_icon_name(if snap.paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
        // Buffering spinner + streaming glyph (v0.0.38).
        now.set_status(snap.buffering, snap.streaming);
        // Chapter-skip buttons appear only for an item with chapters (6c-iii-b).
        now.set_chapter_nav_visible(snap.chapter_count > 0);
        // The per-show podcast playback affordance shows only for an episode.
        now.podcast_btn
            .set_visible(snap.kind == Some(MediaKind::Episode));
        // Sleep timer (6c-iii-d): the moon button shows whenever something is
        // loaded (the media-agnostic scope decision); its label tracks the timer.
        now.sleep_btn.set_visible(snap.track_id.is_some());
        now.set_sleep(snap.sleep);
        // Live drawer extras (6c-iii-c): the current-chapter highlight follows the
        // playhead and the Smart Speed saved time ticks up. Cheap no-ops when the
        // drawer is closed or there is nothing to show.
        if let Some(panel) = imp.now_playing.get()
            && panel.is_open()
        {
            panel.set_current_chapter(snap.current_chapter);
            panel.set_smart_speed(snap.smart_speed_active, snap.smart_speed_saved);
            panel.set_sleep(snap.sleep, snap.kind);
        }
        now.position
            .set_text(&fmt_position(snap.position, snap.duration));
        match snap.duration {
            Some(d) if d > 0.0 => {
                now.seek.set_sensitive(true);
                now.seek.set_range(0.0, d);
                now.seek.set_value(snap.position.min(d));
            }
            _ => now.seek.set_sensitive(false),
        }

        // Status bar tech line (Phase 11b): the cached static fields plus the
        // live mpv channel count. Cheap (no DB); the label no-ops on no change.
        if let Some(label) = imp.status_left.get() {
            let st = imp.tech_static.borrow();
            let channels = snap
                .channels
                .filter(|_| snap.kind == Some(MediaKind::Track));
            label.set_text(&crate::statusbar::tech_line(
                st.0.as_deref(),
                st.1,
                channels,
                st.2,
            ));
        }
    }

    /// Refresh the status bar's right-hand aggregate (Phase 11b): the selection
    /// total when two or more leaf rows are selected, else the whole view's
    /// cached total. The selection sum walks only the selected rows.
    /// Edit/Embed act on the selection, so they follow it: insensitive when
    /// nothing is selected (16.5b; they used to silently no-op).
    fn refresh_edit_sensitivity(&self) {
        let imp = self.imp();
        if let Some(g) = imp.header_edit_group.get() {
            g.set_sensitive(!self.selected_track_ids().is_empty());
        }
    }

    fn refresh_status_aggregate(&self) {
        let imp = self.imp();
        let (Some(leaf), Some(label)) = (imp.leaf.get(), imp.status_right.get()) else {
            return;
        };
        let selected = leaf.selection.selection();
        let (count, total, is_sel) = if selected.size() >= 2 {
            let mut total = 0.0;
            let mut n = 0usize;
            if let Some((iter, first)) = gtk::BitsetIter::init_first(&selected) {
                let mut push = |pos: u32| {
                    if let Some(row) = leaf.selection.item(pos).and_downcast::<TrackRow>() {
                        total += row.brief().duration.unwrap_or(0.0);
                        n += 1;
                    }
                };
                push(first);
                for pos in iter {
                    push(pos);
                }
            }
            (n, total, true)
        } else {
            let (c, t) = imp.view_total.get();
            (c, t, false)
        };
        label.set_text(&crate::statusbar::aggregate_label(count, total, is_sel));
    }

    /// Update the leaf play-status glyph column (Phase 11b) when playback moves.
    /// Walks the store only when the playing track id or pause state changed
    /// since the last apply, flipping each row's `playing` property; the bound
    /// glyph cells repaint themselves (no full-store rebind).
    fn refresh_play_glyphs(&self) {
        let imp = self.imp();
        let (Some(player), Some(leaf)) = (imp.player.get(), imp.leaf.get()) else {
            return;
        };
        let snap = player.snapshot();
        let is_track = snap.kind == Some(MediaKind::Track) && !snap.ended;
        let playing_id = if is_track { snap.track_id } else { None };
        let state = (playing_id, snap.paused);
        if imp.last_play_state.get() == state {
            return;
        }
        imp.last_play_state.set(state);
        let n = leaf.store.n_items();
        for i in 0..n {
            if let Some(row) = leaf.store.item(i).and_downcast::<TrackRow>() {
                let s =
                    crate::statusbar::play_state(row.brief().id, playing_id, is_track, snap.paused);
                if row.playing() != s {
                    row.set_playing(s);
                }
            }
        }
    }

    /// The keyboard-shortcuts reference (Phase 13e-iii, `F1`). Built as an
    /// `adw::PreferencesDialog` of grouped rows rather than a `gtk::ShortcutsWindow`
    /// (deprecated in recent GTK; `AdwShortcutsDialog` postdates our libadwaita), so
    /// it stays current and inherits the app's typography. The list is curated to
    /// match what is actually wired (no aspirational keys).
    fn show_shortcuts_window(&self) {
        let groups: [(&str, &[(&str, &str)]); 3] = [
            (
                "Playback",
                &[
                    ("Space", "Play / pause"),
                    ("Ctrl+Right", "Next track"),
                    ("Ctrl+Left", "Previous track"),
                    ("Ctrl+Up / Ctrl+Down", "Volume up / down"),
                    ("Ctrl+0", "Mute / unmute"),
                    ("Ctrl+M", "Stop after the current track"),
                    ("Ctrl+J", "Jump to the playing track"),
                    ("Ctrl+Shift+Right / Left", "Next / previous chapter"),
                    ("S", "Sleep timer"),
                ],
            ),
            (
                "Browse & Queue",
                &[
                    ("Double-click / Enter", "Play the track or facet"),
                    ("Ctrl+Enter", "Add the selection to the queue"),
                    ("Ctrl+E", "Edit the selected tracks"),
                    ("Ctrl+F", "Focus the filter"),
                    ("Ctrl+L", "Clear the filter"),
                    ("Alt+Up / Alt+Down", "Move the queued item"),
                    ("Delete", "Remove from the queue"),
                    ("Ctrl+Shift+C", "Clear the queue"),
                ],
            ),
            (
                "Panels & View",
                &[
                    ("Ctrl+U", "Queue"),
                    ("Ctrl+P", "Track properties"),
                    ("Ctrl+I", "Now Playing"),
                    ("Alt+1 / Alt+2 / Alt+3", "Music / Podcasts / Audiobooks"),
                    ("Ctrl+comma", "Preferences"),
                    ("F1", "This shortcuts window"),
                    ("Ctrl+Q", "Quit"),
                ],
            ),
        ];

        let page = adw::PreferencesPage::new();
        for (title, rows) in groups {
            let group = adw::PreferencesGroup::builder().title(title).build();
            for (accel, desc) in rows {
                let row = adw::ActionRow::builder().title(*desc).build();
                let keys = gtk::Label::builder()
                    .label(*accel)
                    .css_classes(["dim-label", "numeric"])
                    .build();
                row.add_suffix(&keys);
                group.add(&row);
            }
            page.add(&group);
        }

        let dialog = adw::PreferencesDialog::new();
        dialog.set_title("Keyboard Shortcuts");
        dialog.add(&page);
        dialog.present(Some(self));
    }

    /// The header primary menu (Phase 11d): the transport conveniences that are
    /// keyboard-first but want a visible home (spec §3.1). Registers the backing
    /// window actions and returns the `MenuButton`.
    fn build_primary_menu(&self) -> gtk::MenuButton {
        // Stop-after-current: a stateful toggle (rendered with a checkmark).
        let stop = gio::SimpleAction::new_stateful("stop-after-current", None, &false.to_variant());
        let weak = self.downgrade();
        stop.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.toggle_stop_after_current();
            }
        });
        self.add_action(&stop);
        let _ = self.imp().stop_action.set(stop);

        let jump = gio::SimpleAction::new("jump-to-current", None);
        let weak = self.downgrade();
        jump.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.jump_to_current();
            }
        });
        self.add_action(&jump);

        // Keyboard Shortcuts (Phase 13e-iii): also bound to F1.
        let shortcuts = gio::SimpleAction::new("show-shortcuts", None);
        let weak = self.downgrade();
        shortcuts.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.show_shortcuts_window();
            }
        });
        self.add_action(&shortcuts);

        // About (16.5b): the GNOME-convention dialog, version from the crate.
        let about = gio::SimpleAction::new("show-about", None);
        let weak = self.downgrade();
        about.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.show_about_dialog();
            }
        });
        self.add_action(&about);

        let menu = gio::Menu::new();
        menu.append(Some("Stop After Current"), Some("win.stop-after-current"));
        menu.append(Some("Jump to Current Track"), Some("win.jump-to-current"));
        let help = gio::Menu::new();
        help.append(Some("Keyboard Shortcuts"), Some("win.show-shortcuts"));
        help.append(Some("About Conservatory"), Some("win.show-about"));
        menu.append_section(None, &help);

        gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .menu_model(&menu)
            .build()
    }

    /// The About dialog (16.5b): version from the crate, GPL-3.0-or-later (the
    /// librubberband chain, spec §11), repo link.
    fn show_about_dialog(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Conservatory")
            .application_icon("audio-x-generic")
            .version(env!("CARGO_PKG_VERSION"))
            .developer_name("Brandon LaRocque")
            .license_type(gtk::License::Gpl30)
            .website("https://github.com/VirInvictus/Conservatory")
            .comments(
                "Calibre for audio: a library manager and player for music, \
                 podcasts, and audiobooks that owns its files.",
            )
            .build();
        about.present(Some(self));
    }

    /// Toggle stop-after-current (Phase 11d, `Ctrl+M`): the engine finishes the
    /// current item, then pauses at the boundary instead of playing on.
    fn toggle_stop_after_current(&self) {
        let imp = self.imp();
        let Some(player) = imp.player.get() else {
            return;
        };
        let new = !player.snapshot().stop_after_current;
        player.set_stop_after_current(new);
        if let Some(a) = imp.stop_action.get() {
            a.set_state(&new.to_variant());
        }
    }

    /// Jump to the playing track in the leaf list (Phase 11d, `Ctrl+J`): select
    /// and scroll to it. A no-op when the playing item is not a track in the
    /// current view (e.g. an episode, or filtered out).
    fn jump_to_current(&self) {
        let imp = self.imp();
        let (Some(player), Some(leaf)) = (imp.player.get(), imp.leaf.get()) else {
            return;
        };
        let snap = player.snapshot();
        if snap.kind != Some(MediaKind::Track) {
            return;
        }
        let Some(id) = snap.track_id else {
            return;
        };
        let model = &leaf.selection;
        let n = model.n_items();
        let ids: Vec<i64> = (0..n)
            .filter_map(|i| model.item(i).and_downcast::<TrackRow>())
            .map(|r| r.brief().id)
            .collect();
        if let Some(pos) = crate::statusbar::current_row_index(&ids, id) {
            model.select_item(pos, true);
            leaf.column_view
                .scroll_to(pos, None, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    /// Open the per-show playback settings (speed / Smart Speed / Voice Boost) for
    /// the currently-playing podcast episode's show, from the Now-bar affordance.
    /// The library-management fields (skip intro/outro, inbox policy) are preserved
    /// from the stored row; this is the transport-side shortcut to the show gear,
    /// so the controls live near play/pause where you reach for them.
    #[cfg(feature = "podcasts")]
    fn open_playing_show_settings(&self, anchor: &gtk::Button) {
        use crate::ui::podcasts::{MAX_SPEED, MIN_SPEED, default_settings, settings_from_form};
        use conservatory_core::db::{get_episode, get_show, get_show_settings};

        let imp = self.imp();
        let (Some(player), Some(pool)) = (imp.player.get(), imp.pool.get()) else {
            return;
        };
        let snap = player.snapshot();
        let (Some(MediaKind::Episode), Some(episode_id)) = (snap.kind, snap.track_id) else {
            return;
        };
        let Ok(conn) = pool.open() else { return };
        let Some(ep) = get_episode(&conn, episode_id).ok().flatten() else {
            return;
        };
        let show_id = ep.show_id;
        let show_title = get_show(&conn, show_id)
            .ok()
            .flatten()
            .map(|s| s.title)
            .unwrap_or_else(|| "Show".to_string());
        let current = get_show_settings(&conn, show_id).ok().flatten();
        let cur = current.clone().unwrap_or_else(|| default_settings(show_id));
        drop(conn);

        let group = adw::PreferencesGroup::new();
        group.set_description(Some(
            "Smart Speed trims dead air; Voice Boost lifts quiet, uneven speech. \
             They apply to this show's episodes when you play them.",
        ));
        let speed = adw::SpinRow::with_range(MIN_SPEED, MAX_SPEED, 0.05);
        speed.set_title("Playback speed");
        speed.set_digits(2);
        speed.set_value(cur.playback_speed);
        let smart = adw::SwitchRow::new();
        smart.set_title("Smart Speed");
        smart.set_active(cur.smart_speed);
        let voice = adw::SwitchRow::new();
        voice.set_title("Voice Boost");
        voice.set_active(cur.voice_boost);
        group.add(&speed);
        group.add(&smart);
        group.add(&voice);

        let dialog = adw::AlertDialog::new(Some(&show_title), None);
        dialog.set_extra_child(Some(&group));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "save" {
                return;
            }
            let Some(win) = weak.upgrade() else { return };
            let imp = win.imp();
            let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
                return;
            };
            let settings = settings_from_form(
                current.as_ref(),
                show_id,
                speed.value(),
                smart.is_active(),
                voice.is_active(),
                cur.skip_intro,
                cur.skip_outro,
                cur.inbox_policy,
            );
            let _ = rt.block_on(worker.upsert_show_settings(settings));
            // Apply to the episode playing now so the change is heard immediately,
            // not only from the next episode (the persisted settings cover that).
            if let Some(player) = imp.player.get() {
                player.set_spoken(speed.value(), smart.is_active(), voice.is_active());
            }
        });
        dialog.present(Some(anchor));
    }

    /// Music-only builds carry the Now-bar button but never show it (no episodes),
    /// so the handler is an inert stub.
    #[cfg(not(feature = "podcasts"))]
    fn open_playing_show_settings(&self, _anchor: &gtk::Button) {}

    /// Toggle the bottom Now Playing drawer; when opening, fill it from the
    /// current snapshot so it never shows stale content (v0.0.38). The toggle
    /// happens *before* the refresh: `refresh_now_playing` no-ops while the drawer
    /// reads closed, so refreshing first would leave the freshly-opened drawer
    /// stale.
    fn toggle_now_playing(&self) {
        let imp = self.imp();
        let Some(panel) = imp.now_playing.get() else {
            return;
        };
        let opening = !panel.is_open();
        panel.toggle();
        if opening
            && let Some(player) = imp.player.get()
        {
            let snap = player.snapshot();
            // Mirror the Now-bar's ended guard (16.5b): opening the drawer
            // after the queue finished shows the idle page, not the last item
            // dressed up as "Now Playing".
            let id = if snap.ended { None } else { snap.track_id };
            self.refresh_now_playing(snap.kind, id);
        }
    }

    /// Refresh the Now Playing drawer for `(kind, id)`: read the item's title /
    /// subtitle / cover and its chapter list (v0.0.38). A no-op while the drawer is
    /// closed, so the queue advancing does not do needless reads.
    fn refresh_now_playing(&self, kind: Option<MediaKind>, id: Option<i64>) {
        let imp = self.imp();
        let Some(panel) = imp.now_playing.get() else {
            return;
        };
        if !panel.is_open() {
            return;
        }
        let (Some(id), Some(pool)) = (id, imp.pool.get()) else {
            panel.clear();
            return;
        };
        let Ok(conn) = pool.open() else {
            return;
        };
        use conservatory_core::db::{
            book_chapters, book_metadata, episode_metadata, get_album, get_book, get_track,
            list_chapters, track_metadata,
        };
        match kind {
            Some(MediaKind::Audiobook) => {
                let (Ok(Some(np)), Ok(Some(book))) =
                    (book_metadata(&conn, id), get_book(&conn, id))
                else {
                    panel.clear();
                    return;
                };
                let chapters = book_chapters(&conn, id).unwrap_or_default();
                panel.set_now_playing(&np.title, &np.artist.clone().unwrap_or_default());
                let cover_abs = match (imp.library_root.get(), np.album_cover_path.as_deref()) {
                    (Some(root), Some(cp)) => Some(root.join(cp)),
                    _ => None,
                };
                panel.set_cover(cover_abs.as_deref(), book.accent_rgb);
                // The clickable chapter list speaks book-absolute time (the engine's
                // `Seek` is absolute too): synthesize a `Chapter` per mark.
                if let Some(player) = imp.player.get() {
                    let plan = conservatory_core::player::plan_book(&chapters);
                    let marks: Vec<conservatory_core::db::Chapter> = plan
                        .marks
                        .iter()
                        .enumerate()
                        .map(|(i, m)| conservatory_core::db::Chapter {
                            id: i as i64,
                            episode_id: 0,
                            start_time: m.start_time,
                            end_time: None,
                            title: m.title.clone(),
                            url: None,
                            image_path: None,
                        })
                        .collect();
                    panel.set_chapters(&marks, player);
                }
            }
            Some(MediaKind::Episode) => {
                let Ok(Some(np)) = episode_metadata(&conn, id) else {
                    panel.clear();
                    return;
                };
                panel.set_now_playing(&np.title, &np.artist.clone().unwrap_or_default());
                let cover_abs = match (imp.library_root.get(), np.album_cover_path.as_deref()) {
                    (Some(root), Some(cp)) => Some(root.join(cp)),
                    _ => None,
                };
                panel.set_cover(cover_abs.as_deref(), None);
                // The clickable chapter list tracks the playing episode (6c-iii-c);
                // the current-chapter highlight + Smart Speed line tick from the
                // per-poll snapshot in `refresh_now_bar`.
                if let Some(player) = imp.player.get() {
                    let chapters = list_chapters(&conn, id).unwrap_or_default();
                    panel.set_chapters(&chapters, player);
                }
            }
            _ => {
                let (Ok(Some(np)), Ok(Some(track))) =
                    (track_metadata(&conn, id), get_track(&conn, id))
                else {
                    panel.clear();
                    return;
                };
                let album = track
                    .album_id
                    .and_then(|aid| get_album(&conn, aid).ok().flatten());
                let subtitle = crate::ui::now_bar::now_bar_subtitle(
                    &np.artist.clone().unwrap_or_default(),
                    np.album.as_deref(),
                );
                panel.set_now_playing(&np.title, &subtitle);
                let cover_abs = match (imp.library_root.get(), np.album_cover_path.as_deref()) {
                    (Some(root), Some(cp)) => Some(root.join(cp)),
                    _ => None,
                };
                panel.set_cover(
                    cover_abs.as_deref(),
                    album.as_ref().and_then(|a| a.accent_rgb),
                );
                // A track has no chapters: hide the section.
                if let Some(player) = imp.player.get() {
                    panel.set_chapters(&[], player);
                }
            }
        }
    }

    /// The left Perspectives column: a list (Default + saved searches) over a
    /// save/delete action bar.
    fn build_sidebar(&self) -> gtk::Widget {
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.set_width_request(170);
        sidebar.add_css_class("background");

        let heading = gtk::Label::builder()
            .label("Perspectives")
            .xalign(0.0)
            .margin_top(8)
            .margin_bottom(4)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["heading"])
            .build();

        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::Single);
        let weak = self.downgrade();
        list.connect_row_activated(move |_, row| {
            if let Some(win) = weak.upgrade() {
                win.on_perspective_activated(row.index());
            }
        });
        let list_scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&list)
            .build();

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        actions.add_css_class("toolbar");
        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some("Save the current filter as a Perspective"));
        save_btn.set_hexpand(true);
        let del_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        del_btn.set_tooltip_text(Some("Delete the selected Perspective"));
        del_btn.set_hexpand(true);
        let weak = self.downgrade();
        save_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.prompt_save_perspective();
            }
        });
        let weak = self.downgrade();
        del_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.delete_selected_perspective();
            }
        });
        actions.append(&save_btn);
        actions.append(&del_btn);

        sidebar.append(&heading);
        sidebar.append(&list_scroller);
        sidebar.append(&actions);
        let _ = self.imp().sidebar_list.set(list);

        // --- Playlists section (Phase 16d-ii): below Perspectives, sharing the
        // vertical space. Static + smart playlists; activating one plays it. ---
        sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        let pl_heading = gtk::Label::builder()
            .label("Playlists")
            .xalign(0.0)
            .margin_top(8)
            .margin_bottom(4)
            .margin_start(12)
            .margin_end(12)
            .css_classes(["heading"])
            .build();

        let pl_list = gtk::ListBox::new();
        pl_list.add_css_class("navigation-sidebar");
        pl_list.set_selection_mode(gtk::SelectionMode::Single);
        let weak = self.downgrade();
        pl_list.connect_row_activated(move |_, row| {
            if let Some(win) = weak.upgrade() {
                win.on_playlist_activated(row.index());
            }
        });
        let pl_scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&pl_list)
            .build();

        // The "+" create menu and a delete button.
        let new_static = gio::SimpleAction::new("playlist-new-static", None);
        let weak = self.downgrade();
        new_static.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_new_static_playlist();
            }
        });
        self.add_action(&new_static);
        let new_smart = gio::SimpleAction::new("playlist-new-smart", None);
        let weak = self.downgrade();
        new_smart.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.prompt_new_smart_playlist();
            }
        });
        self.add_action(&new_smart);

        let create_menu = gio::Menu::new();
        create_menu.append(
            Some("New Static Playlist\u{2026}"),
            Some("win.playlist-new-static"),
        );
        create_menu.append(
            Some("New Smart Playlist\u{2026}"),
            Some("win.playlist-new-smart"),
        );
        let pl_create = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New playlist")
            .menu_model(&create_menu)
            .hexpand(true)
            .build();
        let pl_del = gtk::Button::from_icon_name("user-trash-symbolic");
        pl_del.set_tooltip_text(Some("Delete the selected playlist"));
        pl_del.set_hexpand(true);
        let weak = self.downgrade();
        pl_del.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.delete_selected_playlist();
            }
        });
        let pl_actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pl_actions.add_css_class("toolbar");
        pl_actions.append(&pl_create);
        pl_actions.append(&pl_del);

        sidebar.append(&pl_heading);
        sidebar.append(&pl_scroller);
        sidebar.append(&pl_actions);
        let _ = self.imp().playlist_list.set(pl_list);

        sidebar.upcast()
    }

    /// Ctrl+F focuses the filter bar (spec §3.4: no separate search mode).
    fn install_filter_shortcut(&self, filter: &gtk::SearchEntry) {
        let target = filter.downgrade();
        let shortcut = gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>f"),
            Some(gtk::CallbackAction::new(move |_, _| {
                if let Some(entry) = target.upgrade() {
                    entry.grab_focus();
                }
                glib::Propagation::Stop
            })),
        );
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);
        controller.add_shortcut(shortcut);
        self.add_controller(controller);
    }

    fn on_pane_changed(&self, pane: usize) {
        if self.imp().suppress.get() {
            return; // programmatic repopulate, not a user action
        }
        if let Some(c) = self.imp().coalescer.get() {
            c.add(pane);
        }
    }

    /// A sidebar row was chosen: row 0 is Default (clears the filter); the rest
    /// load their saved expression into the filter bar, which re-parses it.
    fn on_perspective_activated(&self, index: i32) {
        let imp = self.imp();
        let Some(entry) = imp.filter_entry.get() else {
            return;
        };
        if index <= 0 {
            entry.set_text("");
            return;
        }
        let text = imp
            .perspectives
            .borrow()
            .get((index - 1) as usize)
            .map(|p| p.expression.clone());
        if let Some(text) = text {
            entry.set_text(&text);
        }
    }

    fn prompt_save_perspective(&self) {
        let imp = self.imp();
        if imp.worker.get().is_none() {
            return;
        }
        let current = imp
            .filter_entry
            .get()
            .map(|e| e.text().to_string())
            .unwrap_or_default();

        let name_entry = gtk::Entry::builder()
            .placeholder_text("Perspective name")
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(
            Some("Save Perspective"),
            Some("Save the current filter as a named, reloadable search."),
        );
        dialog.set_extra_child(Some(&name_entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let weak = self.downgrade();
        let entry_weak = name_entry.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "save" {
                return;
            }
            let (Some(win), Some(name_entry)) = (weak.upgrade(), entry_weak.upgrade()) else {
                return;
            };
            let name = name_entry.text().trim().to_string();
            if !name.is_empty() {
                win.save_perspective(&name, &current);
            }
        });
        dialog.present(Some(self));
    }

    fn save_perspective(&self, name: &str, expression: &str) {
        let imp = self.imp();
        let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let _ = rt.block_on(worker.save_perspective(
            name.to_string(),
            expression.to_string(),
            "tracks".to_string(),
            now,
        ));
        self.refresh_perspectives();
    }

    /// Confirm, then delete the selected Perspective (16.5a: deletion is
    /// permanent, so it gets the destructive-confirm idiom).
    fn delete_selected_perspective(&self) {
        let imp = self.imp();
        let Some(list) = imp.sidebar_list.get() else {
            return;
        };
        let Some(row) = list.selected_row() else {
            return;
        };
        let index = row.index();
        if index <= 0 {
            return; // Default is not deletable
        }
        let Some((id, name)) = imp
            .perspectives
            .borrow()
            .get((index - 1) as usize)
            .map(|p| (p.id, p.name.clone()))
        else {
            return;
        };
        let body =
            format!("Delete the Perspective \u{201c}{name}\u{201d}? This cannot be undone.");
        let dialog = adw::AlertDialog::new(Some("Delete Perspective?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp == "delete"
                && let Some(win) = weak.upgrade()
            {
                let imp = win.imp();
                let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
                    return;
                };
                let _ = rt.block_on(worker.delete_perspective(id));
                win.refresh_perspectives();
            }
        });
        dialog.present(Some(self));
    }

    /// Reload the sidebar from storage: Default on top, then saved Perspectives.
    fn refresh_perspectives(&self) {
        let imp = self.imp();
        let (Some(pool), Some(list)) = (imp.pool.get(), imp.sidebar_list.get()) else {
            return;
        };
        let perspectives = pool
            .open()
            .ok()
            .and_then(|conn| list_perspectives(&conn).ok())
            .unwrap_or_default();

        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        list.append(&perspective_row("Default"));
        for p in &perspectives {
            list.append(&perspective_row(&p.name));
        }
        *imp.perspectives.borrow_mut() = perspectives;
    }

    // --- Playlists sidebar (Phase 16d-ii) ---

    /// Reload the Playlists list from storage (the `refresh_perspectives` twin).
    /// Also repopulates the track menu's "Add to Playlist" submenu so new static
    /// playlists appear there.
    fn refresh_playlists(&self) {
        let imp = self.imp();
        let (Some(pool), Some(list)) = (imp.pool.get(), imp.playlist_list.get()) else {
            return;
        };
        let playlists = pool
            .open()
            .ok()
            .and_then(|conn| list_playlists(&conn).ok())
            .unwrap_or_default();
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        for p in &playlists {
            list.append(&playlist_row(&p.name, p.kind == PlaylistKind::Smart));
        }
        *imp.playlists.borrow_mut() = playlists;
        self.rebuild_add_to_playlist_menu();
    }

    /// Activating a playlist row plays it: materialise its tracks and replace the
    /// queue (the `play_leaf_from` path, from an id list rather than the leaf).
    fn on_playlist_activated(&self, index: i32) {
        if index < 0 {
            return;
        }
        let pl = self.imp().playlists.borrow().get(index as usize).cloned();
        if let Some(pl) = pl {
            self.play_playlist(&pl);
        }
    }

    /// A playlist's ordered track ids: static from its entries, smart from the
    /// query (through the GUI materialiser, which has the search grammar).
    fn materialize_playlist_ids(&self, pl: &Playlist) -> Vec<i64> {
        let Some(pool) = self.imp().pool.get() else {
            return Vec::new();
        };
        match pl.kind {
            PlaylistKind::Static => pool
                .open()
                .ok()
                .and_then(|conn| static_playlist_track_ids(&conn, pl.id).ok())
                .unwrap_or_default(),
            PlaylistKind::Smart => {
                let order = pl.order_by.unwrap_or(PlaylistOrder::Added);
                let today = chrono::Utc::now().date_naive();
                materialize_smart(
                    pool,
                    pl.query.as_deref().unwrap_or(""),
                    order,
                    pl.limit_n,
                    today,
                )
            }
        }
    }

    fn play_playlist(&self, pl: &Playlist) {
        let imp = self.imp();
        let (Some(pool), Some(player), Some(root)) =
            (imp.pool.get(), imp.player.get(), imp.library_root.get())
        else {
            return;
        };
        let ids = self.materialize_playlist_ids(pl);
        if ids.is_empty() {
            return;
        }
        let Ok(conn) = pool.open() else {
            return;
        };
        let tracks = get_tracks(&conn, &ids).unwrap_or_default();
        drop(conn);
        let (items, start) = build_play_queue(&ids, 0, &tracks, root, &self.playback_config());
        if items.is_empty() {
            return;
        }
        let queue_ids: Vec<i64> = items.iter().map(|i| i.track_id).collect();
        if let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) {
            let _ = rt.block_on(worker.replace_queue_with_tracks(queue_ids));
        }
        // The Now-bar resolves title/artist from the DB on item change, so no
        // `now_labels` seeding is needed here.
        imp.last_shown.set(None);
        imp.last_index.set(None);
        if let Some(cur) = imp.queue_current.get() {
            cur.set(Some(start as i64));
        }
        player.play_queue(items, start);
        self.reload_queue_panel();
    }

    /// Create a playlist through the worker, then refresh the sidebar.
    fn create_playlist(
        &self,
        name: String,
        kind: PlaylistKind,
        query: Option<String>,
        limit: Option<i64>,
        order: Option<PlaylistOrder>,
    ) {
        let imp = self.imp();
        let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
            return;
        };
        let now = chrono::Utc::now().timestamp();
        let _ = rt.block_on(worker.create_playlist(name, kind, query, limit, order, now));
        self.refresh_playlists();
    }

    fn prompt_new_static_playlist(&self) {
        if self.imp().worker.get().is_none() {
            return;
        }
        let name_entry = gtk::Entry::builder()
            .placeholder_text("Playlist name")
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(
            Some("New Static Playlist"),
            Some(
                "A frozen, hand-ordered list. Add tracks with the right-click \u{201c}Add to \
                 Playlist\u{201d} menu.",
            ),
        );
        dialog.set_extra_child(Some(&name_entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("create", "Create");
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));
        dialog.set_close_response("cancel");
        let weak = self.downgrade();
        let entry_weak = name_entry.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "create" {
                return;
            }
            let (Some(win), Some(e)) = (weak.upgrade(), entry_weak.upgrade()) else {
                return;
            };
            let name = e.text().trim().to_string();
            if !name.is_empty() {
                win.create_playlist(name, PlaylistKind::Static, None, None, None);
            }
        });
        dialog.present(Some(self));
    }

    /// The smart-playlist rule builder: name, a query pre-filled from the current
    /// filter (so it doubles as "save current search"), an optional limit, and an
    /// order picker. Not a full condition-row builder yet, but not query-only-JSON.
    fn prompt_new_smart_playlist(&self) {
        let imp = self.imp();
        if imp.worker.get().is_none() {
            return;
        }
        let current_filter = imp
            .filter_entry
            .get()
            .map(|e| e.text().to_string())
            .unwrap_or_default();

        let name_entry = gtk::Entry::builder()
            .placeholder_text("Playlist name")
            .build();
        let query_entry = gtk::Entry::builder()
            .placeholder_text("rating:>=4 AND genre:jazz")
            .text(&current_filter)
            .hexpand(true)
            .build();
        let limit_entry = gtk::Entry::builder().placeholder_text("no limit").build();
        let orders = [
            "Added (newest first)",
            "Rating (highest first)",
            "Least recently played",
            "Title",
            "Artist",
        ];
        let order_dd = gtk::DropDown::from_strings(&orders);

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        for (r, (label, widget)) in [
            ("Name", name_entry.clone().upcast::<gtk::Widget>()),
            ("Query", query_entry.clone().upcast()),
            ("Limit", limit_entry.clone().upcast()),
            ("Order", order_dd.clone().upcast()),
        ]
        .into_iter()
        .enumerate()
        {
            let lbl = gtk::Label::builder().label(label).xalign(1.0).build();
            grid.attach(&lbl, 0, r as i32, 1, 1);
            grid.attach(&widget, 1, r as i32, 1, 1);
        }

        let dialog = adw::AlertDialog::new(
            Some("New Smart Playlist"),
            Some(
                "A live rule: a search that resolves fresh each time, with an optional limit and order.",
            ),
        );
        dialog.set_extra_child(Some(&grid));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("create", "Create");
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));
        dialog.set_close_response("cancel");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp != "create" {
                return;
            }
            let Some(win) = weak.upgrade() else {
                return;
            };
            let name = name_entry.text().trim().to_string();
            let query = query_entry.text().trim().to_string();
            if name.is_empty() || query.is_empty() {
                return;
            }
            let limit = limit_entry
                .text()
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0);
            let order = match order_dd.selected() {
                1 => PlaylistOrder::Rating,
                2 => PlaylistOrder::LastPlayed,
                3 => PlaylistOrder::Title,
                4 => PlaylistOrder::Artist,
                _ => PlaylistOrder::Added,
            };
            win.create_playlist(name, PlaylistKind::Smart, Some(query), limit, Some(order));
        });
        dialog.present(Some(self));
    }

    /// Confirm, then delete the selected playlist (16.5a: a curated static
    /// playlist is not reconstructible, so it gets the destructive confirm).
    fn delete_selected_playlist(&self) {
        let imp = self.imp();
        let Some(list) = imp.playlist_list.get() else {
            return;
        };
        let Some(row) = list.selected_row() else {
            return;
        };
        let index = row.index();
        if index < 0 {
            return;
        }
        let Some((id, name)) = imp
            .playlists
            .borrow()
            .get(index as usize)
            .map(|p| (p.id, p.name.clone()))
        else {
            return;
        };
        let body = format!("Delete the playlist \u{201c}{name}\u{201d}? This cannot be undone.");
        let dialog = adw::AlertDialog::new(Some("Delete Playlist?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = self.downgrade();
        dialog.connect_response(None, move |_, resp| {
            if resp == "delete"
                && let Some(win) = weak.upgrade()
            {
                let imp = win.imp();
                let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
                    return;
                };
                let _ = rt.block_on(worker.delete_playlist(id));
                win.refresh_playlists();
            }
        });
        dialog.present(Some(self));
    }

    /// Repopulate the track menu's "Add to Playlist" submenu from the static
    /// playlists (smart playlists can't be hand-added to).
    fn rebuild_add_to_playlist_menu(&self) {
        let imp = self.imp();
        let Some(menu) = imp.add_to_playlist_menu.get() else {
            return;
        };
        menu.remove_all();
        for p in imp.playlists.borrow().iter() {
            if p.kind == PlaylistKind::Static {
                let item = gio::MenuItem::new(Some(&p.name), None);
                item.set_action_and_target_value(
                    Some("win.track-add-to-playlist"),
                    Some(&p.id.to_variant()),
                );
                menu.append_item(&item);
            }
        }
    }

    /// Append the track-list selection to a static playlist (the context verb).
    fn append_selection_to_playlist(&self, playlist_id: i64) {
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        let imp = self.imp();
        let (Some(rt), Some(worker)) = (imp.runtime.get(), imp.worker.get()) else {
            return;
        };
        let n = ids.len();
        let _ = rt.block_on(worker.append_playlist_tracks(playlist_id, ids));
        self.toast(&format!("Added {n} track(s) to the playlist"));
    }

    fn recompute_from(&self, earliest: usize) {
        self.imp().suppress.set(true);
        self.cascade(earliest);
        self.imp().suppress.set(false);
    }

    fn populate_initial(&self) {
        self.imp().suppress.set(true);
        self.cascade_pane0();
        self.cascade(0);
        self.imp().suppress.set(false);
    }

    fn cascade_pane0(&self) {
        let imp = self.imp();
        let Some(pool) = imp.pool.get() else { return };
        let Ok(conn) = pool.open() else { return };
        let panes = imp.panes.borrow();
        let Some(first) = panes.first() else { return };
        let rows = facet_rows(&conn, first.field, &[]).unwrap_or_default();
        let total = rows.iter().map(|r| r.count).sum();
        first.set_rows(&rows, total);
    }

    /// The current effective facet filters across all panes (empty panes, i.e.
    /// `[All]`, contribute no constraint).
    fn current_filters(&self) -> Vec<FacetFilter> {
        self.imp()
            .panes
            .borrow()
            .iter()
            .filter_map(|pane| {
                let values = pane.effective_values();
                (!values.is_empty()).then_some(FacetFilter {
                    field: pane.field,
                    values,
                })
            })
            .collect()
    }

    /// Recompute the leaf from the current facet filters and filter-bar text,
    /// intersected (spec §3.4), and tint the bar when the grammar degraded.
    fn set_leaf(&self) {
        let imp = self.imp();
        let (Some(pool), Some(leaf)) = (imp.pool.get(), imp.leaf.get()) else {
            return;
        };
        let query = imp
            .filter_entry
            .get()
            .map(|e| e.text().to_string())
            .unwrap_or_default();
        let today = chrono::Utc::now().date_naive();
        let filters = self.current_filters();
        let (tracks, warnings) = query_leaf(pool, &filters, &query, today);
        // "Filtered" means the empty result is from a constraint (facet or filter
        // text), not an empty library: it picks the empty-state wording.
        let filtered = !query.trim().is_empty() || !filters.is_empty();
        leaf.set_tracks(&tracks, filtered);
        // Cache the view aggregate for the status bar (Phase 11b) and refresh it.
        imp.view_total
            .set(crate::statusbar::view_aggregate(&tracks));
        self.refresh_status_aggregate();
        // A reload rebuilds the selection model, so re-derive the header
        // Edit/Embed sensitivity rather than trust the last signal (16.5b).
        self.refresh_edit_sensitivity();
        // The fresh rows default to no glyph; force the play-status column to
        // re-apply so the playing track (if any) is marked in the new view.
        imp.last_play_state.set((None, false));
        self.refresh_play_glyphs();
        if let Some(entry) = imp.filter_entry.get() {
            // The yellow tint alone said nothing about *what* degraded (16.5b):
            // the tooltip carries the parser's warnings while they exist, and
            // reverts to the grammar hint when the query is clean again.
            if warnings.is_empty() {
                entry.remove_css_class("filter-warn");
                entry.set_tooltip_text(Some(FILTER_GRAMMAR_TIP));
            } else {
                entry.add_css_class("filter-warn");
                entry.set_tooltip_text(Some(&warnings.join("\n")));
            }
        }
    }

    /// Recompute panes after `earliest` and the leaf, from the selections of
    /// panes `0..=earliest` (downstream panes reset to `[All]`).
    fn cascade(&self, earliest: usize) {
        let imp = self.imp();
        let Some(pool) = imp.pool.get() else { return };
        let Ok(conn) = pool.open() else { return };
        {
            let panes = imp.panes.borrow();
            if panes.is_empty() {
                return;
            }
            let last_upstream = earliest.min(panes.len() - 1);

            let mut filters = Vec::new();
            for pane in panes.iter().take(last_upstream + 1) {
                let values = pane.effective_values();
                if !values.is_empty() {
                    filters.push(FacetFilter {
                        field: pane.field,
                        values,
                    });
                }
            }

            for pane in panes.iter().skip(last_upstream + 1) {
                let rows = facet_rows(&conn, pane.field, &filters).unwrap_or_default();
                let total = rows.iter().map(|r| r.count).sum();
                pane.set_rows(&rows, total);
            }
        }

        // Leaf goes through the filter-bar path so the active grammar still
        // applies after a facet change.
        self.set_leaf();
    }
}

/// Map an `Alt+N` view-switch key to the stack page name (spec §2.3). `None`
/// for an out-of-range key. Switching to a page that is not compiled in is a
/// no-op (handled in `switch_view`), so this stays a pure key→name mapping.
fn view_page_name(n: u8) -> Option<&'static str> {
    match n {
        1 => Some("music"),
        2 => Some("podcasts"),
        3 => Some("audiobooks"),
        _ => None,
    }
}

/// A short EQ band-centre label (`16000` → `16k`, `500` → `500`).
fn fmt_centre(centre: u32) -> String {
    if centre >= 1000 {
        format!("{}k", centre / 1000)
    } else {
        centre.to_string()
    }
}

/// Read the current band gains off the EQ sliders into the fixed band array.
fn read_slider_bands(sliders: &[gtk::Scale]) -> [f64; EQ_CENTRES.len()] {
    let mut bands = [0.0; EQ_CENTRES.len()];
    for (slot, s) in bands.iter_mut().zip(sliders.iter()) {
        *slot = s.value();
    }
    bands
}

/// The import-mode `ComboRow` index for an [`ImportMode`] (Copy = 0, Move = 1),
/// and its inverse (Phase 10b). The one non-trivial config-row projection.
fn import_mode_index(mode: ImportMode) -> u32 {
    match mode {
        ImportMode::Copy => 0,
        ImportMode::Move => 1,
    }
}

fn import_mode_from_index(index: u32) -> ImportMode {
    match index {
        1 => ImportMode::Move,
        _ => ImportMode::Copy,
    }
}

/// The value shared by every entry in `vals`, or `None` when they differ (the
/// bulk-edit "multiple values" state, Phase 16c). An empty selection collapses to
/// a shared empty string. Pure, for testing without a realized dialog.
fn common_value(mut vals: Vec<String>) -> Option<String> {
    match vals.pop() {
        None => Some(String::new()),
        Some(first) if vals.iter().all(|v| *v == first) => Some(first),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ImportMode, common_value, import_mode_from_index, import_mode_index, view_page_name,
    };

    #[test]
    fn common_value_agrees_or_reports_mixed() {
        // All the same collapses to that value (the shared prefill).
        assert_eq!(
            common_value(vec!["Aphex Twin".into(), "Aphex Twin".into()]),
            Some("Aphex Twin".into())
        );
        // A single track is trivially "shared".
        assert_eq!(common_value(vec!["Solo".into()]), Some("Solo".into()));
        // Differing values are "multiple values" (None).
        assert_eq!(common_value(vec!["A".into(), "B".into()]), None);
        // All-empty is a shared empty string, not mixed.
        assert_eq!(
            common_value(vec![String::new(), String::new()]),
            Some(String::new())
        );
        // An empty selection collapses to empty, not mixed.
        assert_eq!(common_value(vec![]), Some(String::new()));
    }

    #[test]
    fn view_keys_map_to_page_names() {
        assert_eq!(view_page_name(1), Some("music"));
        assert_eq!(view_page_name(2), Some("podcasts"));
        assert_eq!(view_page_name(3), Some("audiobooks"));
        assert_eq!(view_page_name(0), None);
        assert_eq!(view_page_name(4), None);
    }

    #[test]
    fn import_mode_index_round_trips() {
        for mode in [ImportMode::Copy, ImportMode::Move] {
            assert_eq!(import_mode_from_index(import_mode_index(mode)), mode);
        }
        // Out-of-range index degrades to the safe Copy default.
        assert_eq!(import_mode_from_index(99), ImportMode::Copy);
    }
}
