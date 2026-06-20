//! One faceted-browse pane: a scrollable, multi-selectable list of facet values
//! with counts, topped by an `[All (N)]` row. Phase 3b. The selection-change
//! wiring + cascade live in `window` (they need the window's shared state).

use gtk::gio;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{FacetField, FacetRow as CoreFacetRow};

use crate::ui::objects::FacetRow;

/// A built pane: its field, the model + selection (for the cascade), and the
/// widget to place in the window.
pub struct FacetPane {
    pub field: FacetField,
    pub plural: String,
    pub store: gio::ListStore,
    pub selection: gtk::MultiSelection,
    pub view: gtk::Box,
}

/// Build a pane for `field`. `plural` labels the `[All (N <plural>)]` row.
pub fn build_pane(field: FacetField, title: &str, plural: &str) -> FacetPane {
    let store = gio::ListStore::new::<FacetRow>();
    let selection = gtk::MultiSelection::new(Some(store.clone()));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    let plural_owned = plural.to_string();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = item.item().and_downcast::<FacetRow>().expect("FacetRow");
        let label = item.child().and_downcast::<gtk::Label>().expect("Label");
        label.set_text(&row.display(&plural_owned));
    });

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.add_css_class("navigation-sidebar");

    let header = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_start(8)
        .margin_top(6)
        .margin_bottom(2)
        .build();

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_width_request(200);
    column.append(&header);
    column.append(&scroller);

    FacetPane {
        field,
        plural: plural.to_string(),
        store,
        selection,
        view: column,
    }
}

impl FacetPane {
    /// Replace the pane's rows with the `[All (total)]` row plus `rows`, and
    /// select `[All]` (clearing any prior constraint). The caller suppresses the
    /// selection-changed signal around this so it does not re-trigger the cascade.
    pub fn set_rows(&self, rows: &[CoreFacetRow], total: i64) {
        self.store.remove_all();
        self.store.append(&FacetRow::new("", total, true));
        for r in rows {
            self.store.append(&FacetRow::new(&r.value, r.count, false));
        }
        // Select the [All] row (index 0), unselecting the rest.
        self.selection.select_item(0, true);
    }

    /// The pane's effective constraint: the selected non-`[All]` values. `[All]`
    /// selected (or nothing selected) means no constraint (empty).
    pub fn effective_values(&self) -> Vec<String> {
        let n = self.store.n_items();
        let mut values = Vec::new();
        for i in 0..n {
            if !self.selection.is_selected(i) {
                continue;
            }
            let row = self
                .store
                .item(i)
                .and_downcast::<FacetRow>()
                .expect("FacetRow");
            if row.is_all() {
                return Vec::new(); // [All] selected => no constraint
            }
            values.push(row.value());
        }
        values
    }
}
