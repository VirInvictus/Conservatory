//! One faceted-browse pane: a `ColumnView` with a value column and a right-
//! aligned `Count` column (the deadbeef-cui Columns UI look), topped by an
//! `[All (N)]` row. Sortable headers; grid lines; dense rows. Phase 3b. The
//! selection-change wiring + cascade live in `window`.

use std::cmp::Ordering;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{FacetField, FacetRow as CoreFacetRow};

use crate::ui::objects::FacetRow;

/// A built pane: its field, the model + selection (for the cascade), and the
/// widget to place in the window.
pub struct FacetPane {
    pub field: FacetField,
    pub store: gio::ListStore,
    pub selection: gtk::MultiSelection,
    /// The inner `ColumnView`; the window connects `activate` to it for
    /// double-click / Enter to play the facet's filtered set (Phase 13e-i).
    pub column_view: gtk::ColumnView,
    pub view: gtk::ScrolledWindow,
}

fn to_gtk(o: Ordering) -> gtk::Ordering {
    match o {
        Ordering::Less => gtk::Ordering::Smaller,
        Ordering::Equal => gtk::Ordering::Equal,
        Ordering::Greater => gtk::Ordering::Larger,
    }
}

fn facet(obj: &glib::Object) -> FacetRow {
    obj.clone().downcast::<FacetRow>().expect("FacetRow")
}

/// Build a pane for `field`. The column header (`field.title()`) and the
/// `[All (N <plural>)]` noun (`field.plural()`) come from the field descriptor
/// (Phase 10c), so the window builds N panes from a config list.
pub fn build_pane(field: FacetField) -> FacetPane {
    let title = field.title();
    let plural = field.plural();
    let store = gio::ListStore::new::<FacetRow>();

    let view = gtk::ColumnView::new(None::<gtk::SelectionModel>);
    view.set_show_row_separators(true);
    view.set_show_column_separators(true);
    view.add_css_class("data-table");

    // Value column (expands, ellipsizes).
    let value_factory = gtk::SignalListItemFactory::new();
    value_factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    let plural_owned = plural.to_string();
    value_factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = facet(&item.item().expect("item"));
        let label = item.child().and_downcast::<gtk::Label>().expect("Label");
        label.set_text(&row.value_text(&plural_owned));
    });
    let value_col = gtk::ColumnViewColumn::new(Some(title), Some(value_factory));
    value_col.set_expand(true);
    value_col.set_resizable(true);
    let value_sorter = gtk::CustomSorter::new(|a, b| {
        let (a, b) = (facet(a), facet(b));
        let ord = match (a.is_all(), b.is_all()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.value().to_lowercase().cmp(&b.value().to_lowercase()),
        };
        to_gtk(ord)
    });
    value_col.set_sorter(Some(&value_sorter));

    // Count column (right-aligned numbers).
    let count_factory = gtk::SignalListItemFactory::new();
    count_factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder().xalign(1.0).build();
        label.add_css_class("numeric");
        item.set_child(Some(&label));
    });
    count_factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = facet(&item.item().expect("item"));
        let label = item.child().and_downcast::<gtk::Label>().expect("Label");
        label.set_text(&row.count().to_string());
    });
    let count_col = gtk::ColumnViewColumn::new(Some("Count"), Some(count_factory));
    count_col.set_fixed_width(64);
    count_col.set_resizable(true);
    let count_sorter = gtk::CustomSorter::new(|a, b| {
        let (a, b) = (facet(a), facet(b));
        let ord = match (a.is_all(), b.is_all()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.count().cmp(&b.count()),
        };
        to_gtk(ord)
    });
    count_col.set_sorter(Some(&count_sorter));

    view.append_column(&value_col);
    view.append_column(&count_col);

    // Sort through the column headers; default by value ascending.
    let sort_model = gtk::SortListModel::new(Some(store.clone()), view.sorter());
    let selection = gtk::MultiSelection::new(Some(sort_model));
    view.set_model(Some(&selection));
    view.sort_by_column(Some(&value_col), gtk::SortType::Ascending);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .width_request(160)
        .child(&view)
        .build();

    FacetPane {
        field,
        store,
        selection,
        column_view: view,
        view: scroller,
    }
}

impl FacetPane {
    /// Replace the rows with the `[All]` row (`distinct` values, `total` tracks)
    /// plus `rows`, and select `[All]`. The caller suppresses the selection-
    /// changed signal around this so it does not re-trigger the cascade.
    pub fn set_rows(&self, rows: &[CoreFacetRow], total: i64) {
        self.store.remove_all();
        self.store
            .append(&FacetRow::all_row(rows.len() as i64, total));
        for r in rows {
            self.store.append(&FacetRow::value_row(&r.value, r.count));
        }
        // Select the [All] row wherever it sorts (it is pinned to the top).
        let n = self.selection.n_items();
        for i in 0..n {
            if facet(&self.selection.item(i).expect("item")).is_all() {
                self.selection.select_item(i, true);
                break;
            }
        }
    }

    /// The pane's effective constraint: the selected non-`[All]` values. `[All]`
    /// selected (or nothing selected) means no constraint (empty).
    pub fn effective_values(&self) -> Vec<String> {
        let n = self.selection.n_items();
        let mut values = Vec::new();
        for i in 0..n {
            if !self.selection.is_selected(i) {
                continue;
            }
            let row = facet(&self.selection.item(i).expect("item"));
            if row.is_all() {
                return Vec::new();
            }
            values.push(row.value());
        }
        values
    }
}
