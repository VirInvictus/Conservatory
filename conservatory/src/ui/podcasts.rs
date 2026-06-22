//! The Podcasts triage browse (Phase 6b-ii-a, read-only).
//!
//! Fills the 6b-i Podcasts page (spec §3.7): a sidebar of triage buckets
//! (Inbox / Queue / Played) and subscribed shows, an episode list showing each
//! episode's played state, and a detail pane with the show notes. Read-only;
//! the triage *actions* (mark played/starred, enqueue) and episode playback are
//! Phase 6b-ii-b. The whole module is compiled only with the `podcasts` feature.
//!
//! Built over the read pool with the core triage reads (`episodes_in_bucket`,
//! `episodes_for_show`); no widget logic reaches into the database directly
//! beyond those calls.

use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;

use conservatory_core::db::{
    EpisodeListRow, ReadPool, TriageBucket, episodes_for_show, episodes_in_bucket, list_shows,
};

use crate::ui::objects::EpisodeRow;

/// What the episode list is currently showing.
#[derive(Clone, Copy)]
enum Source {
    Bucket(TriageBucket),
    Show(i64),
}

/// Shared state for the episode list + detail pane.
struct Inner {
    pool: ReadPool,
    store: gtk::gio::ListStore,
    title: gtk::Label,
    subtitle: gtk::Label,
    notes: gtk::Label,
}

impl Inner {
    fn load(&self, source: Source) {
        self.store.remove_all();
        for row in &self.read(source) {
            self.store.append(&EpisodeRow::new(row));
        }
        self.show_detail(None);
    }

    fn read(&self, source: Source) -> Vec<EpisodeListRow> {
        let Ok(conn) = self.pool.open() else {
            return Vec::new();
        };
        match source {
            Source::Bucket(b) => episodes_in_bucket(&conn, b).unwrap_or_default(),
            Source::Show(id) => episodes_for_show(&conn, id).unwrap_or_default(),
        }
    }

    fn show_detail(&self, row: Option<&EpisodeRow>) {
        match row {
            Some(r) => {
                self.title.set_text(&r.title());
                self.subtitle.set_text(&detail_subtitle(r));
                let notes = r.description();
                self.notes.set_text(if notes.trim().is_empty() {
                    "No show notes."
                } else {
                    &notes
                });
            }
            None => {
                self.title.set_text("");
                self.subtitle.set_text("");
                self.notes.set_text("Select an episode to read its notes.");
            }
        }
    }
}

fn detail_subtitle(r: &EpisodeRow) -> String {
    let mut parts = vec![r.show_title()];
    for piece in [r.date_text(), r.duration_text()] {
        if !piece.is_empty() {
            parts.push(piece);
        }
    }
    parts.join("  \u{2022}  ")
}

/// Build the read-only Podcasts triage view over the read pool.
pub fn build_podcasts_view(pool: ReadPool) -> gtk::Widget {
    let store = gtk::gio::ListStore::new::<EpisodeRow>();
    let selection = gtk::SingleSelection::builder()
        .model(&store)
        .autoselect(false)
        .can_unselect(true)
        .build();

    let column_view = gtk::ColumnView::new(Some(selection.clone()));
    column_view.add_css_class("data-table");
    column_view.append_column(&state_column());
    column_view.append_column(&text_column("Episode", true, EpisodeRow::title));
    column_view.append_column(&text_column("Date", false, EpisodeRow::date_text));
    column_view.append_column(&text_column("Length", false, EpisodeRow::duration_text));
    let list_scroll = gtk::ScrolledWindow::builder()
        .child(&column_view)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Detail pane.
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-3"])
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    let notes = gtk::Label::builder()
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    let notes_scroll = gtk::ScrolledWindow::builder()
        .child(&notes)
        .vexpand(true)
        .build();
    let detail = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(280)
        .build();
    detail.append(&title);
    detail.append(&subtitle);
    detail.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    detail.append(&notes_scroll);

    let inner = Rc::new(Inner {
        pool: pool.clone(),
        store,
        title,
        subtitle,
        notes,
    });
    inner.show_detail(None);

    // Episode selection drives the detail pane.
    {
        let inner = inner.clone();
        selection.connect_selected_item_notify(move |sel| {
            let row = sel.selected_item().and_downcast::<EpisodeRow>();
            inner.show_detail(row.as_ref());
        });
    }

    // Sidebar: triage buckets, then subscribed shows. `sources` maps a row
    // index to what it loads (header rows are `None`).
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    let mut sources: Vec<Option<Source>> = Vec::new();

    sidebar_list.append(&section_header("Triage"));
    sources.push(None);
    for (label, icon, bucket) in [
        ("Inbox", "mail-unread-symbolic", TriageBucket::Inbox),
        ("Queue", "view-list-symbolic", TriageBucket::Queue),
        ("Played", "object-select-symbolic", TriageBucket::Played),
    ] {
        sidebar_list.append(&sidebar_entry(label, icon));
        sources.push(Some(Source::Bucket(bucket)));
    }

    let shows = pool
        .open()
        .ok()
        .and_then(|c| list_shows(&c).ok())
        .unwrap_or_default();
    if !shows.is_empty() {
        sidebar_list.append(&section_header("Shows"));
        sources.push(None);
        for show in &shows {
            sidebar_list.append(&sidebar_entry(&show.title, "microphone-symbolic"));
            sources.push(Some(Source::Show(show.id)));
        }
    }

    {
        let inner = inner.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row
                && let Some(Some(source)) = usize::try_from(row.index())
                    .ok()
                    .and_then(|i| sources.get(i))
            {
                inner.load(*source);
            }
        });
    }
    // Open on Inbox (row index 1, just after the "Triage" header).
    if let Some(first) = sidebar_list.row_at_index(1) {
        sidebar_list.select_row(Some(&first));
    }
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar_list)
        .width_request(200)
        .build();

    // Layout: sidebar | (episode list | detail). Nested `gtk::Paned`, matching
    // the music browse body; an adaptive AdwNavigationSplitView is a later
    // refinement.
    let content = gtk::Paned::new(gtk::Orientation::Horizontal);
    content.set_start_child(Some(&list_scroll));
    content.set_end_child(Some(&detail));
    content.set_resize_start_child(true);
    content.set_resize_end_child(true);
    content.set_position(520);

    let root = gtk::Paned::new(gtk::Orientation::Horizontal);
    root.set_start_child(Some(&sidebar_scroll));
    root.set_end_child(Some(&content));
    root.set_resize_start_child(false);
    root.set_shrink_start_child(false);
    root.set_position(200);
    root.upcast()
}

fn section_header(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading", "dim-label"])
        .margin_top(8)
        .margin_start(6)
        .margin_bottom(2)
        .build();
    row.set_child(Some(&label));
    row
}

fn sidebar_entry(text: &str, icon: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.set_margin_start(6);
    b.set_margin_end(6);
    b.set_margin_top(4);
    b.set_margin_bottom(4);
    b.append(&gtk::Image::from_icon_name(icon));
    b.append(
        &gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .hexpand(true)
            .build(),
    );
    row.set_child(Some(&b));
    row
}

/// The played-state glyph column.
fn state_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        item.set_child(Some(&gtk::Image::new()));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let (Some(row), Some(img)) = (
            item.item().and_downcast::<EpisodeRow>(),
            item.child().and_downcast::<gtk::Image>(),
        ) {
            img.set_icon_name(Some(row.state_icon()));
            img.set_tooltip_text(Some(row.state_label()));
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    col.set_fixed_width(36);
    col
}

/// A text column rendering `getter(row)` into an ellipsized label.
fn text_column(
    title: &str,
    expand: bool,
    getter: impl Fn(&EpisodeRow) -> String + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));
    });
    let getter = Rc::new(getter);
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let (Some(row), Some(label)) = (
            item.item().and_downcast::<EpisodeRow>(),
            item.child().and_downcast::<gtk::Label>(),
        ) {
            label.set_text(&getter(&row));
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_expand(expand);
    col
}

// The widget tree itself is verified by build + manual launch (the 3b/3c/4b/6b-i
// precedent): a `gtk::init()`-based construction test hangs under cargo's
// multi-threaded runner because GTK must run on the main thread. The row
// formatting that backs the list is unit-tested in `objects.rs`.
