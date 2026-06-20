//! The leaf track list (Phase 3b: minimal, read-only). Phase 3c upgrades it in
//! place to sortable columns, multi-select, and row affordances.

use gtk::gio;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::TrackBrief;

use crate::ui::objects::TrackRow;

/// The leaf list: its model (for repopulation) and the widget to place.
pub struct Leaf {
    pub store: gio::ListStore,
    pub view: gtk::Box,
}

pub fn build_leaf() -> Leaf {
    let store = gio::ListStore::new::<TrackRow>();
    let selection = gtk::NoSelection::new(Some(store.clone()));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = item.item().and_downcast::<TrackRow>().expect("TrackRow");
        let label = item.child().and_downcast::<gtk::Label>().expect("Label");
        let artist = row.artist();
        if artist.is_empty() {
            label.set_text(&row.title());
        } else {
            label.set_text(&format!("{}  —  {}", row.title(), artist));
        }
    });

    let list = gtk::ListView::new(Some(selection), Some(factory));
    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&list)
        .build();

    let header = gtk::Label::builder()
        .label("Tracks")
        .xalign(0.0)
        .css_classes(["heading"])
        .margin_start(8)
        .margin_top(6)
        .margin_bottom(2)
        .build();

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.set_hexpand(true);
    column.append(&header);
    column.append(&scroller);

    Leaf {
        store,
        view: column,
    }
}

impl Leaf {
    pub fn set_tracks(&self, tracks: &[TrackBrief]) {
        self.store.remove_all();
        for t in tracks {
            self.store
                .append(&TrackRow::new(&t.title, t.artist.as_deref()));
        }
    }
}
