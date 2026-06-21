//! GObject list-model items for the facet panes and the leaf track table. Plain
//! data carriers (Rust getters, no glib properties) — the `ColumnView` factories
//! read them on bind. Phase 3b.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{MediaKind, QueueDisplayRow, TrackBrief};

// --- Facet row (a pane entry: a value + its track count) ---

mod facet_imp {
    use super::*;

    #[derive(Default)]
    pub struct FacetRow {
        pub value: RefCell<String>,
        pub count: Cell<i64>,    // track count (the Count column)
        pub is_all: Cell<bool>,  // the synthetic `[All]` row
        pub distinct: Cell<i64>, // distinct value count, for the `[All (N)]` label
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FacetRow {
        const NAME: &'static str = "ConservatoryFacetRow";
        type Type = super::FacetRow;
    }

    impl ObjectImpl for FacetRow {}
}

glib::wrapper! {
    pub struct FacetRow(ObjectSubclass<facet_imp::FacetRow>);
}

impl FacetRow {
    /// A normal value row.
    pub fn value_row(value: &str, count: i64) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().value.replace(value.to_string());
        obj.imp().count.set(count);
        obj
    }

    /// The synthetic top row: `distinct` distinct values, `total` tracks.
    pub fn all_row(distinct: i64, total: i64) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().is_all.set(true);
        obj.imp().distinct.set(distinct);
        obj.imp().count.set(total);
        obj
    }

    pub fn value(&self) -> String {
        self.imp().value.borrow().clone()
    }

    pub fn count(&self) -> i64 {
        self.imp().count.get()
    }

    pub fn is_all(&self) -> bool {
        self.imp().is_all.get()
    }

    /// The text for the value column: the value, or `[All (N <plural>)]`.
    pub fn value_text(&self, plural: &str) -> String {
        if self.is_all() {
            format!("[All ({} {plural})]", self.imp().distinct.get())
        } else {
            self.value()
        }
    }
}

// --- Track row (leaf table entry) ---

mod track_imp {
    use super::*;

    #[derive(Default)]
    pub struct TrackRow {
        // The whole brief is carried so the column factories render it and the
        // sorter compares it through core's `cmp_tracks` (one source of truth).
        pub brief: RefCell<Option<TrackBrief>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "ConservatoryTrackRow";
        type Type = super::TrackRow;
    }

    impl ObjectImpl for TrackRow {}
}

glib::wrapper! {
    pub struct TrackRow(ObjectSubclass<track_imp::TrackRow>);
}

impl TrackRow {
    pub fn new(brief: &TrackBrief) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().brief.replace(Some(brief.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&TrackBrief) -> R) -> R {
        f(self.imp().brief.borrow().as_ref().expect("brief set"))
    }

    /// A clone of the underlying brief, for the `CustomSorter` comparison.
    pub fn brief(&self) -> TrackBrief {
        self.with(|b| b.clone())
    }

    pub fn title(&self) -> String {
        self.with(|b| b.title.clone())
    }

    pub fn artist(&self) -> String {
        self.with(|b| b.artist.clone().unwrap_or_default())
    }

    pub fn album(&self) -> String {
        self.with(|b| b.album.clone().unwrap_or_default())
    }

    pub fn genres(&self) -> String {
        self.with(|b| b.genres.clone())
    }

    pub fn rating(&self) -> u8 {
        self.with(|b| b.rating)
    }

    /// `m:ss`, or empty when unknown.
    pub fn duration_text(&self) -> String {
        let secs = self.with(|b| b.duration.unwrap_or(0.0));
        if secs <= 0.0 {
            return String::new();
        }
        let total = secs.round() as i64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

// --- Queue row (queue drawer entry, Phase 4b-ii-b) ---

mod queue_imp {
    use super::*;

    #[derive(Default)]
    pub struct QueueRow {
        pub row: RefCell<Option<QueueDisplayRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueRow {
        const NAME: &'static str = "ConservatoryQueueRow";
        type Type = super::QueueRow;
    }

    impl ObjectImpl for QueueRow {}
}

glib::wrapper! {
    pub struct QueueRow(ObjectSubclass<queue_imp::QueueRow>);
}

impl QueueRow {
    pub fn new(row: &QueueDisplayRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&QueueDisplayRow) -> R) -> R {
        f(self.imp().row.borrow().as_ref().expect("row set"))
    }

    /// The 0-based queue position (also the engine index, kept in sync).
    pub fn position(&self) -> i64 {
        self.with(|r| r.position)
    }

    pub fn kind(&self) -> MediaKind {
        self.with(|r| r.kind)
    }

    pub fn title(&self) -> String {
        self.with(|r| r.title.clone())
    }

    pub fn artist(&self) -> String {
        self.with(|r| r.artist.clone().unwrap_or_default())
    }
}
