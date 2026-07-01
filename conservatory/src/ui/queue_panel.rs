//! The slide-in queue drawer (Phase 4b-ii-b, spec §3.6). A `gtk::ListView` of the
//! unified queue inside a right-docked `gtk::Revealer`: each row is a kind icon
//! over title/artist, the playing row is accent-highlighted, and rows are
//! drag-and-drop reorderable (the Atrium DragSource/DropTarget idiom). Rendering
//! reads core's `QueueDisplayRow`; reorder is delegated to the window via the
//! `on_reorder` callback (which writes the DB queue and the live engine queue).

use std::cell::Cell;
use std::rc::Rc;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::MediaKind;

use crate::playqueue::{drop_target_position, DropBias};
use crate::ui::objects::QueueRow;

/// The queue drawer: its backing store (for repopulation), the selection (for
/// keyboard ops), the list, and the revealer to place.
pub struct QueuePanel {
    pub store: gio::ListStore,
    pub selection: gtk::SingleSelection,
    pub list: gtk::ListView,
    pub revealer: gtk::Revealer,
}

fn queue_row(obj: &glib::Object) -> QueueRow {
    obj.clone().downcast::<QueueRow>().expect("QueueRow")
}

/// A symbolic icon for the entry's kind (font-independent, icon-theme glyphs).
fn kind_icon(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Track => "audio-x-generic-symbolic",
        MediaKind::Episode => "audio-speakers-symbolic",
        MediaKind::Audiobook => "book-open-variant-symbolic",
    }
}

/// Build the drawer. `current` is the playing position (shared with the window,
/// which updates it and rebuilds the store so the highlight follows playback);
/// `on_reorder` applies a finished drag as `(from, to)`.
pub fn build_queue_panel(
    current: Rc<Cell<Option<i64>>>,
    on_reorder: Rc<dyn Fn(usize, usize)>,
) -> QueuePanel {
    let store = gio::ListStore::new::<QueueRow>();
    let selection = gtk::SingleSelection::new(Some(store.clone()));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("queue-row");
        let icon = gtk::Image::new();
        icon.add_css_class("dim-label");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let artist = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label", "caption"])
            .build();
        text.append(&title);
        text.append(&artist);
        row.append(&icon);
        row.append(&text);
        item.set_child(Some(&row));
    });

    let store_for_bind = store.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let obj = queue_row(&item.item().expect("item"));
        let row = item.child().and_downcast::<gtk::Box>().expect("Box");
        let icon = row
            .first_child()
            .and_downcast::<gtk::Image>()
            .expect("Image");
        let text = icon.next_sibling().and_downcast::<gtk::Box>().expect("Box");
        let title = text
            .first_child()
            .and_downcast::<gtk::Label>()
            .expect("Label");
        let artist = title
            .next_sibling()
            .and_downcast::<gtk::Label>()
            .expect("Label");

        icon.set_icon_name(Some(kind_icon(obj.kind())));
        title.set_text(&obj.title());
        artist.set_text(&obj.artist());

        // Highlight the playing row (the window updates `current` + rebuilds).
        if current.get() == Some(obj.position()) {
            row.add_css_class("playing");
        } else {
            row.remove_css_class("playing");
        }

        // Drag-and-drop reorder (the Atrium idiom): the source carries this
        // row's position; the target reads it, computes Above/Below from the
        // cursor Y, and hands the window a final (from, to).
        let from = obj.position();
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        drag.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&from.to_value()))
        });
        row.add_controller(drag.clone());

        let drop = gtk::DropTarget::new(i64::static_type(), gdk::DragAction::MOVE);
        let dest = obj.position() as usize;
        let on_reorder = on_reorder.clone();
        let store = store_for_bind.clone();
        drop.connect_drop(move |target, value, _x, y| {
            if let Ok(src) = value.get::<i64>() {
                let src = src as usize;
                if src != dest {
                    let height = target.widget().map(|w| w.height()).unwrap_or(0).max(1);
                    let bias = if y < f64::from(height) / 2.0 {
                        DropBias::Above
                    } else {
                        DropBias::Below
                    };
                    let to = drop_target_position(src, dest, bias, store.n_items() as usize);
                    on_reorder(src, to);
                }
                return true;
            }
            false
        });
        row.add_controller(drop.clone());

        // Stash both controllers so unbind can detach them (no leak / double-fire
        // when the row is recycled).
        unsafe {
            row.set_data("queue-drag", drag);
            row.set_data("queue-drop", drop);
        }
    });

    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let Some(row) = item.child().and_downcast::<gtk::Box>() {
            if let Some(drag) = unsafe { row.steal_data::<gtk::DragSource>("queue-drag") } {
                row.remove_controller(&drag);
            }
            if let Some(drop) = unsafe { row.steal_data::<gtk::DropTarget>("queue-drop") } {
                row.remove_controller(&drop);
            }
        }
    });

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.add_css_class("queue-list");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .width_request(230)
        .child(&list)
        .build();

    let header = gtk::Label::builder()
        .label("Queue")
        .xalign(0.0)
        .margin_top(8)
        .margin_bottom(4)
        .margin_start(12)
        .margin_end(12)
        .css_classes(["heading"])
        .build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.add_css_class("background");
    column.append(&header);
    column.append(&scroller);

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .transition_duration(250)
        .reveal_child(false)
        .child(&column)
        .build();

    QueuePanel {
        store,
        selection,
        list,
        revealer,
    }
}

impl QueuePanel {
    /// Replace the rows from a fresh `load_queue_display` read.
    pub fn set_rows(&self, rows: &[conservatory_core::db::QueueDisplayRow]) {
        self.store.remove_all();
        for row in rows {
            self.store.append(&QueueRow::new(row));
        }
    }

    /// Toggle the drawer's visibility.
    pub fn toggle(&self) {
        self.revealer
            .set_reveal_child(!self.revealer.reveals_child());
    }
}
