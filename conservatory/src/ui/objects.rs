//! GObject list-model items for the facet panes and the leaf track table. Plain
//! data carriers (Rust getters, no glib properties) — the `ColumnView` factories
//! read them on bind. Phase 3b.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

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
        pub title: RefCell<String>,
        pub artist: RefCell<String>,
        pub album: RefCell<String>,
        pub duration: Cell<f64>,
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
    pub fn new(
        title: &str,
        artist: Option<&str>,
        album: Option<&str>,
        duration: Option<f64>,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().title.replace(title.to_string());
        obj.imp().artist.replace(artist.unwrap_or("").to_string());
        obj.imp().album.replace(album.unwrap_or("").to_string());
        obj.imp().duration.set(duration.unwrap_or(0.0));
        obj
    }

    pub fn title(&self) -> String {
        self.imp().title.borrow().clone()
    }

    pub fn artist(&self) -> String {
        self.imp().artist.borrow().clone()
    }

    pub fn album(&self) -> String {
        self.imp().album.borrow().clone()
    }

    /// `m:ss`, or empty when unknown.
    pub fn duration_text(&self) -> String {
        let secs = self.imp().duration.get();
        if secs <= 0.0 {
            return String::new();
        }
        let total = secs.round() as i64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}
