//! The leaf track table (Phase 3b): a `ColumnView` with Artist / Album / Title /
//! Duration columns, grid lines, dense rows (the deadbeef-cui track list look).
//! Phase 3c adds sorting, multi-select, and richer columns (rating, bitrate, ...).

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::TrackBrief;

use crate::ui::objects::TrackRow;

/// The leaf table: its model (for repopulation) and the widget to place.
pub struct Leaf {
    pub store: gio::ListStore,
    pub view: gtk::ScrolledWindow,
}

fn track(obj: &glib::Object) -> TrackRow {
    obj.clone().downcast::<TrackRow>().expect("TrackRow")
}

/// A text column reading `field(row)` into a (optionally right-aligned) label.
fn text_column(
    title: &str,
    expand: bool,
    xalign: f32,
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
    col
}

pub fn build_leaf() -> Leaf {
    let store = gio::ListStore::new::<TrackRow>();
    let selection = gtk::MultiSelection::new(Some(store.clone()));

    let view = gtk::ColumnView::new(Some(selection));
    view.set_show_row_separators(true);
    view.set_show_column_separators(true);
    view.add_css_class("data-table");

    view.append_column(&text_column("Artist", true, 0.0, TrackRow::artist));
    view.append_column(&text_column("Album", true, 0.0, TrackRow::album));
    view.append_column(&text_column("Title", true, 0.0, TrackRow::title));
    let duration = text_column("Duration", false, 1.0, TrackRow::duration_text);
    duration.set_fixed_width(80);
    view.append_column(&duration);

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&view)
        .build();

    Leaf {
        store,
        view: scroller,
    }
}

impl Leaf {
    pub fn set_tracks(&self, tracks: &[TrackBrief]) {
        self.store.remove_all();
        for t in tracks {
            self.store.append(&TrackRow::new(
                &t.title,
                t.artist.as_deref(),
                t.album.as_deref(),
                t.duration,
            ));
        }
    }
}
