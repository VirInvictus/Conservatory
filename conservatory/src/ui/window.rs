//! The faceted browse window (Phase 3b/3c). An `adw::ApplicationWindow` subclass
//! (programmatic children, no `.ui`) holding the read pool, the single-writer
//! worker, the facet panes, the filter bar, the Perspectives sidebar, and the
//! leaf list. Phase 3c adds the always-on filter bar (spec §3.4: the panes
//! filter, the grammar searches, they intersect on the leaf) and Perspectives:
//! named saved searches in the sidebar, saved through the worker and reloaded by
//! re-parsing their text.

use std::path::PathBuf;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use conservatory_core::db::{
    FacetFilter, Perspective, ReadPool, WorkerHandle, facet_rows, list_perspectives, spawn_worker,
};

use crate::query::query_leaf;
use crate::ui::coalescing::CoalescingQueue;
use crate::ui::facet_pane::{FacetPane, build_pane};
use crate::ui::track_list::{Leaf, build_leaf};

type Coalescer = CoalescingQueue<usize, Box<dyn FnMut(Vec<usize>)>>;
type FilterCoalescer = CoalescingQueue<(), Box<dyn FnMut(Vec<()>)>>;

mod imp {
    use super::*;
    use std::cell::{Cell, OnceCell, RefCell};

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
    pub fn new(app: &adw::Application, db_path: Option<PathBuf>) -> Self {
        let win: Self = glib::Object::builder().property("application", app).build();
        win.set_title(Some("Conservatory"));
        win.set_default_size(1100, 700);
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
                }
            }
            if let Ok(pool) = ReadPool::new(path, 3) {
                let _ = imp.pool.set(pool);
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

        let header = adw::HeaderBar::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&filter_bar);
        toolbar.set_content(Some(&body));
        self.set_content(Some(&toolbar));

        *imp.panes.borrow_mut() = panes;
        let _ = imp.leaf.set(leaf);
        let _ = imp.filter_entry.set(filter.clone());

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

        if imp.pool.get().is_some() {
            self.populate_initial();
            self.refresh_perspectives();
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
