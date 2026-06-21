//! The leaf track table (Phase 3c): a sortable, multi-select `ColumnView` with
//! the deadbeef-cui columns (Artist | Album | Genre | Title | Duration | Rating).
//! Click a header to sort; the comparison delegates to `core::cmp_tracks` so the
//! GTK sort and the headless `sort_tracks` never diverge. Multi-select comes free
//! from `MultiSelection` (Ctrl/Shift). Rating renders as accent-tinted stars.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{TrackBrief, TrackSort, cmp_tracks};

use crate::ui::objects::TrackRow;

const MAX_STARS: i32 = 5;

/// The leaf table: its backing store (for repopulation), the selection (for
/// multi-select + reading the visible order), the `ColumnView` (for row
/// activation), and the scroller to place.
pub struct Leaf {
    pub store: gio::ListStore,
    pub selection: gtk::MultiSelection,
    pub column_view: gtk::ColumnView,
    pub view: gtk::ScrolledWindow,
}

fn track(obj: &glib::Object) -> TrackRow {
    obj.clone().downcast::<TrackRow>().expect("TrackRow")
}

/// A `CustomSorter` that orders two rows by `key`, ascending; the `ColumnView`
/// header reverses it for descending. Both rows route through `cmp_tracks`.
fn column_sorter(key: TrackSort) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let a = track(a).brief();
        let b = track(b).brief();
        cmp_tracks(&a, &b, key, false).into()
    })
}

/// A text column reading `field(row)` into a (optionally right-aligned) label,
/// sortable by `key`.
fn text_column(
    title: &str,
    expand: bool,
    xalign: f32,
    key: TrackSort,
    field: fn(&TrackRow) -> String,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(xalign)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = track(&item.item().expect("item"));
        let label = item.child().and_downcast::<gtk::Label>().expect("Label");
        label.set_text(&field(&row));
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col.set_resizable(true);
    col.set_sorter(Some(&column_sorter(key)));
    col
}

/// The Rating column: a fixed row of five symbolic stars, filled to the row's
/// rating. Symbolic icons come from the icon theme (font-independent).
fn rating_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("rating-stars");
        for _ in 0..MAX_STARS {
            row.append(&gtk::Image::from_icon_name("non-starred-symbolic"));
        }
        item.set_child(Some(&row));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let rating = i32::from(track(&item.item().expect("item")).rating());
        let row = item.child().and_downcast::<gtk::Box>().expect("Box");
        let mut star = row.first_child();
        let mut i = 0;
        while let Some(child) = star {
            let img = child.downcast_ref::<gtk::Image>().expect("Image");
            img.set_icon_name(Some(if i < rating {
                "starred-symbolic"
            } else {
                "non-starred-symbolic"
            }));
            star = child.next_sibling();
            i += 1;
        }
    });
    let col = gtk::ColumnViewColumn::new(Some("Rating"), Some(factory));
    col.set_fixed_width(96);
    col.set_sorter(Some(&column_sorter(TrackSort::Rating)));
    col
}

pub fn build_leaf() -> Leaf {
    let store = gio::ListStore::new::<TrackRow>();

    // Wrap the store in a SortListModel so header clicks reorder the rows; its
    // sorter is the ColumnView's own (set once the view exists, just below).
    let sort_model = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
    let selection = gtk::MultiSelection::new(Some(sort_model.clone()));

    let view = gtk::ColumnView::new(Some(selection.clone()));
    view.set_show_row_separators(true);
    view.set_show_column_separators(true);
    view.add_css_class("data-table");

    view.append_column(&text_column(
        "Artist",
        true,
        0.0,
        TrackSort::Artist,
        TrackRow::artist,
    ));
    view.append_column(&text_column(
        "Album",
        true,
        0.0,
        TrackSort::Album,
        TrackRow::album,
    ));
    view.append_column(&text_column(
        "Genre",
        true,
        0.0,
        TrackSort::Genre,
        TrackRow::genres,
    ));
    view.append_column(&text_column(
        "Title",
        true,
        0.0,
        TrackSort::Title,
        TrackRow::title,
    ));
    let duration = text_column(
        "Duration",
        false,
        1.0,
        TrackSort::Duration,
        TrackRow::duration_text,
    );
    duration.set_fixed_width(80);
    view.append_column(&duration);
    view.append_column(&rating_column());

    sort_model.set_sorter(view.sorter().as_ref());

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&view)
        .build();

    Leaf {
        store,
        selection,
        column_view: view,
        view: scroller,
    }
}

impl Leaf {
    pub fn set_tracks(&self, tracks: &[TrackBrief]) {
        self.store.remove_all();
        for t in tracks {
            self.store.append(&TrackRow::new(t));
        }
    }
}
