//! The leaf track table (Phase 3c): a sortable, multi-select `ColumnView` with
//! the deadbeef-cui columns (Artist | Album | Genre | Title | Duration | Rating).
//! Click a header to sort; the comparison delegates to `core::cmp_tracks` so the
//! GTK sort and the headless `sort_tracks` never diverge. Multi-select comes free
//! from `MultiSelection` (Ctrl/Shift). Rating renders as accent-tinted stars.

use std::path::PathBuf;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use conservatory_core::db::{TrackBrief, TrackSort, cmp_tracks};

use crate::ui::covers::CoverCache;
use crate::ui::objects::TrackRow;

const MAX_STARS: i32 = 5;
/// The browse cover thumbnail edge, in px. A 40px cover gives album-art-per-row
/// (the deadbeef look) while keeping the table reasonably dense.
const COVER_PX: i32 = 40;
const COVER_PLACEHOLDER: &str = "audio-x-generic-symbolic";

/// The leaf table: its backing store (for repopulation), the selection (for
/// multi-select + reading the visible order), the `ColumnView` (for row
/// activation), the scroller, and a `Stack` that swaps the table for an empty
/// state (Phase 13b). `stack` is the placeable widget.
pub struct Leaf {
    pub store: gio::ListStore,
    pub selection: gtk::MultiSelection,
    pub column_view: gtk::ColumnView,
    pub view: gtk::ScrolledWindow,
    pub stack: gtk::Stack,
    empty_page: adw::StatusPage,
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

/// Set the play-status glyph on `img` from a `TrackRow::playing` state; `None`
/// (an inactive row) renders no icon, keeping the fixed column width stable.
fn set_glyph(img: &gtk::Image, state: u8) {
    img.set_icon_name(crate::statusbar::play_glyph(state));
}

/// The play-status glyph column (Phase 11b, the deadbeef leftmost ♫): a symbolic
/// play/pause icon on the row that is the currently playing *track*. Bound to the
/// row's `playing` glib property via `notify`, so when playback moves the window
/// flips just the affected rows' property and only those repaint (no full-store
/// rebind on a 50k-track library). The owed item from Phase 3c.
fn glyph_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let img = gtk::Image::new();
        img.add_css_class("play-glyph");
        item.set_child(Some(&img));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = track(&item.item().expect("item"));
        let img = item.child().and_downcast::<gtk::Image>().expect("Image");
        set_glyph(&img, row.playing());
        // Repaint this row's glyph when its `playing` property changes; the
        // handler is stashed so unbind can disconnect it on recycle.
        let img_weak = img.downgrade();
        let handler = row.connect_playing_notify(move |row| {
            if let Some(img) = img_weak.upgrade() {
                set_glyph(&img, row.playing());
            }
        });
        unsafe {
            img.set_data("playing-handler", handler);
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let Some(img) = item.child().and_downcast::<gtk::Image>()
            && let Some(handler) =
                unsafe { img.steal_data::<glib::SignalHandlerId>("playing-handler") }
            && let Some(obj) = item.item()
        {
            track(&obj).disconnect(handler);
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    col.set_fixed_width(28);
    col
}

/// The album-cover thumbnail column (Phase 12b, the deadbeef album-art-per-row):
/// a small rounded cover loaded from the album's `cover_path`, resolved against
/// the library `root` and decoded once through the shared `CoverCache`. Falls
/// back to the generic-audio icon when the album has no cover on disk. Lazy-bound
/// like the other factory columns, so only visible rows decode.
fn cover_column(root: Option<PathBuf>, cache: CoverCache) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let img = gtk::Image::builder()
            .pixel_size(COVER_PX)
            .icon_name(COVER_PLACEHOLDER)
            .css_classes(["cover-thumb"])
            .overflow(gtk::Overflow::Hidden) // clip the texture to the rounded corners
            .build();
        item.set_child(Some(&img));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = track(&item.item().expect("item"));
        let img = item.child().and_downcast::<gtk::Image>().expect("Image");
        // Decode at 2x the display size for crispness on HiDPI.
        let tex = root
            .as_ref()
            .zip(row.cover_path())
            .and_then(|(r, cp)| cache.texture(&r.join(cp), COVER_PX * 2));
        match tex {
            Some(t) => img.set_paintable(Some(&t)),
            None => img.set_icon_name(Some(COVER_PLACEHOLDER)),
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    col.set_fixed_width(COVER_PX + 8);
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

pub fn build_leaf(root: Option<PathBuf>) -> Leaf {
    let store = gio::ListStore::new::<TrackRow>();
    let cover_cache = CoverCache::new();

    // Wrap the store in a SortListModel so header clicks reorder the rows; its
    // sorter is the ColumnView's own (set once the view exists, just below).
    let sort_model = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
    let selection = gtk::MultiSelection::new(Some(sort_model.clone()));

    let view = gtk::ColumnView::new(Some(selection.clone()));
    view.set_show_row_separators(true);
    view.set_show_column_separators(true);
    view.add_css_class("data-table");

    view.append_column(&cover_column(root, cover_cache));
    view.append_column(&glyph_column());
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

    // The empty state (Phase 13b): a centered StatusPage shown in place of the
    // table when the leaf is empty. Its title / description are set per refresh
    // (no library vs no filter matches).
    let empty_page = adw::StatusPage::builder()
        .icon_name(COVER_PLACEHOLDER)
        .title("No tracks")
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&scroller, Some("list"));
    stack.add_named(&empty_page, Some("empty"));

    Leaf {
        store,
        selection,
        column_view: view,
        view: scroller,
        stack,
        empty_page,
    }
}

impl Leaf {
    /// Repopulate the table and swap to the empty state when there are no rows.
    /// `filtered` distinguishes "the library is empty" from "nothing matches the
    /// current filter" so the empty state can say the useful thing.
    pub fn set_tracks(&self, tracks: &[TrackBrief], filtered: bool) {
        self.store.remove_all();
        for t in tracks {
            self.store.append(&TrackRow::new(t));
        }
        if tracks.is_empty() {
            if filtered {
                self.empty_page
                    .set_icon_name(Some("system-search-symbolic"));
                self.empty_page.set_title("No matches");
                self.empty_page
                    .set_description(Some("No tracks match the current filter."));
            } else {
                self.empty_page.set_icon_name(Some(COVER_PLACEHOLDER));
                self.empty_page.set_title("No tracks yet");
                self.empty_page
                    .set_description(Some("Import music to start your library."));
            }
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack.set_visible_child_name("list");
        }
    }
}
