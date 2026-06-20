//! GObject list-model items for the facet panes and the leaf track list. Plain
//! data carriers (Rust getters, no glib properties) — the factories read them on
//! bind. Phase 3b.

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
        pub count: Cell<i64>,
        pub is_all: Cell<bool>,
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
    pub fn new(value: &str, count: i64, is_all: bool) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().value.replace(value.to_string());
        obj.imp().count.set(count);
        obj.imp().is_all.set(is_all);
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

    /// The text shown in the pane: the `[All (N)]` synthetic row, else
    /// `value (count)`.
    pub fn display(&self, field_plural: &str) -> String {
        if self.is_all() {
            format!("[All ({} {field_plural})]", self.count())
        } else {
            format!("{} ({})", self.value(), self.count())
        }
    }
}

// --- Track row (leaf list entry) ---

mod track_imp {
    use super::*;

    #[derive(Default)]
    pub struct TrackRow {
        pub title: RefCell<String>,
        pub artist: RefCell<String>,
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
    pub fn new(title: &str, artist: Option<&str>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().title.replace(title.to_string());
        obj.imp().artist.replace(artist.unwrap_or("").to_string());
        obj
    }

    pub fn title(&self) -> String {
        self.imp().title.borrow().clone()
    }

    pub fn artist(&self) -> String {
        self.imp().artist.borrow().clone()
    }
}
