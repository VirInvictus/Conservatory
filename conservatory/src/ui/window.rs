//! The faceted browse window (Phase 3b). An `adw::ApplicationWindow` subclass
//! (programmatic children, no `.ui`) holding the read pool, the facet panes, and
//! the leaf list, plus the debounced cascade that recomputes downstream panes +
//! the leaf when a pane's selection changes.

use std::path::PathBuf;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use conservatory_core::db::{FacetFilter, ReadPool, facet_rows, facet_tracks};

use crate::ui::coalescing::CoalescingQueue;
use crate::ui::facet_pane::{FacetPane, build_pane};
use crate::ui::track_list::{Leaf, build_leaf};

type Coalescer = CoalescingQueue<usize, Box<dyn FnMut(Vec<usize>)>>;

mod imp {
    use super::*;
    use std::cell::{Cell, OnceCell, RefCell};

    #[derive(Default)]
    pub struct ConservatoryWindow {
        pub pool: OnceCell<ReadPool>,
        pub panes: RefCell<Vec<FacetPane>>,
        pub leaf: OnceCell<Leaf>,
        pub coalescer: OnceCell<Coalescer>,
        pub suppress: Cell<bool>,
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

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for pane in &panes {
            content.append(&pane.view);
            content.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        }
        content.append(&leaf.view);

        let header = adw::HeaderBar::new();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        self.set_content(Some(&toolbar));

        *imp.panes.borrow_mut() = panes;
        let _ = imp.leaf.set(leaf);

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

        for (i, pane) in imp.panes.borrow().iter().enumerate() {
            let weak = self.downgrade();
            pane.selection.connect_selection_changed(move |_, _, _| {
                if let Some(win) = weak.upgrade() {
                    win.on_pane_changed(i);
                }
            });
        }

        if imp.pool.get().is_some() {
            self.populate_initial();
        }
    }

    fn on_pane_changed(&self, pane: usize) {
        if self.imp().suppress.get() {
            return; // programmatic repopulate, not a user action
        }
        if let Some(c) = self.imp().coalescer.get() {
            c.add(pane);
        }
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

    /// Recompute panes after `earliest` and the leaf, from the selections of
    /// panes `0..=earliest` (downstream panes reset to `[All]`).
    fn cascade(&self, earliest: usize) {
        let imp = self.imp();
        let Some(pool) = imp.pool.get() else { return };
        let Ok(conn) = pool.open() else { return };
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

        if let Some(leaf) = imp.leaf.get() {
            let tracks = facet_tracks(&conn, &filters).unwrap_or_default();
            leaf.set_tracks(&tracks);
        }
    }
}
