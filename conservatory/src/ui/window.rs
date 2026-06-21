//! The faceted browse window (Phase 3b/3c). An `adw::ApplicationWindow` subclass
//! (programmatic children, no `.ui`) holding the read pool, the single-writer
//! worker, the facet panes, the filter bar, the Perspectives sidebar, and the
//! leaf list. Phase 3c adds the always-on filter bar (spec §3.4: the panes
//! filter, the grammar searches, they intersect on the leaf) and Perspectives:
//! named saved searches in the sidebar, saved through the worker and reloaded by
//! re-parsing their text.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use conservatory_core::db::{
    FacetFilter, Perspective, ReadPool, WorkerHandle, facet_rows, get_tracks, list_perspectives,
    load_queue_display, read_playback_state, spawn_worker,
};
use conservatory_core::{PlaybackConfig, PlayerHandle};

use crate::playqueue::{build_play_queue, fmt_position};
use crate::query::query_leaf;
use crate::ui::coalescing::CoalescingQueue;
use crate::ui::facet_pane::{FacetPane, build_pane};
use crate::ui::now_bar::{NowBar, build_now_bar};
use crate::ui::objects::TrackRow;
use crate::ui::queue_panel::{QueuePanel, build_queue_panel};
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
        // The queue drawer (Phase 4b-ii-b). `queue_current` is the playing
        // position, shared with the panel's row factory for the highlight; the
        // window updates it from the snapshot and rebuilds the drawer.
        pub queue_panel: OnceCell<QueuePanel>,
        pub queue_current: OnceCell<Rc<Cell<Option<i64>>>>,
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

/// One sidebar row: a left-aligned, ellipsized name label.
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
                rt.spawn(conservatory_core::mpris::run(player.clone(), pool.clone()));
            }
        }

        let panes = vec![
            build_pane(conservatory_core::db::FacetField::Genre, "Genre", "genres"),
            build_pane(
                conservatory_core::db::FacetField::AlbumArtist,
                "Album Artist",
                "artists",
            ),
            build_pane(conservatory_core::db::FacetField::Album, "Album", "albums"),
        ];
        let leaf = build_leaf();

        // Facet panes in a row on top; the track table below (a draggable split,
        // the deadbeef-cui layout).
        let facet_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for (i, pane) in panes.iter().enumerate() {
            if i > 0 {
                facet_row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
            }
            facet_row.append(&pane.view);
        }

        let split = gtk::Paned::new(gtk::Orientation::Vertical);
        split.set_start_child(Some(&facet_row));
        split.set_end_child(Some(&leaf.view));
        split.set_resize_start_child(true);
        split.set_resize_end_child(true);
        split.set_position(300);

        let sidebar = self.build_sidebar();
        let body = gtk::Paned::new(gtk::Orientation::Horizontal);
        body.set_start_child(Some(&sidebar));
        body.set_end_child(Some(&split));
        body.set_resize_start_child(false);
        body.set_shrink_start_child(false);
        body.set_position(190);

        // The always-on filter bar (spec §3.4); Ctrl+F focuses it, no search mode.
        let filter = gtk::SearchEntry::builder()
            .placeholder_text("Filter: genre:ambient  rating:>=4  vl:Favourites")
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
        let queue_panel = build_queue_panel(queue_current.clone(), on_reorder);

        // Body + the queue drawer, side by side.
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.append(&body);
        content.append(&queue_panel.revealer);

        let header = adw::HeaderBar::new();
        let queue_btn = gtk::Button::from_icon_name("view-list-symbolic");
        queue_btn.set_tooltip_text(Some("Show / hide the queue (Ctrl+U)"));
        let weak = self.downgrade();
        queue_btn.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.toggle_queue();
            }
        });
        header.pack_end(&queue_btn);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&filter_bar);
        toolbar.set_content(Some(&content));
        toolbar.add_bottom_bar(&now_bar.root);
        self.set_content(Some(&toolbar));

        *imp.panes.borrow_mut() = panes;
        let _ = imp.leaf.set(leaf);
        let _ = imp.filter_entry.set(filter.clone());
        let _ = imp.now_bar.set(now_bar);
        let _ = imp.queue_current.set(queue_current);
        self.install_queue_keys(&queue_panel.list);
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
            leaf.column_view.add_controller(append);
        }

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
        }

        let weak = self.downgrade();
        filter.connect_search_changed(move |_| {
            if let Some(win) = weak.upgrade() {
                if let Some(c) = win.imp().filter_coalescer.get() {
                    c.add(());
                }
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
            }
            glib::Propagation::Proceed
        });

        if imp.pool.get().is_some() {
            self.populate_initial();
            self.refresh_perspectives();
        }
        // Load the saved queue paused at the cursor (Phase 4b-ii-c).
        self.resume_saved_queue();
    }

    /// Double-click / Enter on a track: play the visible leaf list from that row
    /// (spec §3.6). The selection model presents rows in display (sorted) order,
    /// so its index range is the queue order and `pos` is the start.
    fn on_track_activated(&self, pos: u32) {
        let imp = self.imp();
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
            &PlaybackConfig::default(),
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
        let ordered_ids: Vec<i64> = rows.iter().filter_map(|r| r.track_id).collect();
        let mut labels = std::collections::HashMap::new();
        for r in &rows {
            if let Some(id) = r.track_id {
                labels.insert(id, (r.title.clone(), r.artist.clone().unwrap_or_default()));
            }
        }
        let tracks = get_tracks(&conn, &ordered_ids).unwrap_or_default();
        drop(conn);

        let activated = saved
            .as_ref()
            .and_then(|s| s.track_id)
            .and_then(|tid| ordered_ids.iter().position(|&id| id == tid))
            .unwrap_or(0);
        let (items, start) = build_play_queue(
            &ordered_ids,
            activated,
            &tracks,
            root,
            &PlaybackConfig::default(),
        );
        if items.is_empty() {
            return;
        }
        let position = saved.map(|s| s.position).unwrap_or(0.0);
        *imp.now_labels.borrow_mut() = labels;
        imp.last_shown.set(None);
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
            if model.is_selected(i) {
                if let Some(row) = model.item(i).and_then(|o| o.downcast::<TrackRow>().ok()) {
                    let brief = row.brief();
                    ordered_ids.push(brief.id);
                    labels.push((brief.id, (brief.title, brief.artist.unwrap_or_default())));
                }
            }
        }
        if ordered_ids.is_empty() {
            return;
        }

        let Ok(conn) = pool.open() else { return };
        let tracks = get_tracks(&conn, &ordered_ids).unwrap_or_default();
        drop(conn);
        let (items, _start) =
            build_play_queue(&ordered_ids, 0, &tracks, root, &PlaybackConfig::default());
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
            if let Some(panel) = imp.queue_panel.get() {
                if panel.revealer.reveals_child() {
                    self.reload_queue_panel();
                }
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
        self.add_controller(global);

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

    /// Refresh the Now-bar from the player snapshot (the 250 ms poll). Title and
    /// artist re-render only when the track changes; position/seek/icon every tick.
    fn refresh_now_bar(&self) {
        let imp = self.imp();
        let (Some(player), Some(now)) = (imp.player.get(), imp.now_bar.get()) else {
            return;
        };
        let snap = player.snapshot();

        if snap.ended || snap.track_id.is_none() {
            if imp.last_shown.get().is_some() {
                imp.last_shown.set(None);
                now.clear();
            }
            return;
        }

        if imp.last_shown.get() != snap.track_id {
            imp.last_shown.set(snap.track_id);
            if let Some(id) = snap.track_id {
                let labels = imp.now_labels.borrow();
                let (title, artist) = labels
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| ("\u{2014}".to_string(), String::new()));
                now.title.set_text(&title);
                now.artist.set_text(&artist);
            }
        }

        now.play_btn.set_icon_name(if snap.paused {
            "media-playback-start-symbolic"
        } else {
            "media-playback-pause-symbolic"
        });
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

    fn delete_selected_perspective(&self) {
        let imp = self.imp();
        let (Some(rt), Some(worker), Some(list)) =
            (imp.runtime.get(), imp.worker.get(), imp.sidebar_list.get())
        else {
            return;
        };
        let Some(row) = list.selected_row() else {
            return;
        };
        let index = row.index();
        if index <= 0 {
            return; // Default is not deletable
        }
        let id = imp
            .perspectives
            .borrow()
            .get((index - 1) as usize)
            .map(|p| p.id);
        if let Some(id) = id {
            let _ = rt.block_on(worker.delete_perspective(id));
            self.refresh_perspectives();
        }
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
        let (tracks, warnings) = query_leaf(pool, &self.current_filters(), &query, today);
        leaf.set_tracks(&tracks);
        if let Some(entry) = imp.filter_entry.get() {
            if warnings.is_empty() {
                entry.remove_css_class("filter-warn");
            } else {
                entry.add_css_class("filter-warn");
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
