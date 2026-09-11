//! The leaf track table (Phase 3c): a sortable, multi-select `ColumnView` with
//! the deadbeef-cui columns (Artist | Album | Genre | Title | Duration | Rating).
//! Click a header to sort; the comparison delegates to `core::cmp_tracks` so the
//! GTK sort and the headless `sort_tracks` never diverge. Multi-select comes free
//! from `MultiSelection` (Ctrl/Shift). Rating renders as accent-tinted stars.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{TrackBrief, TrackSort, cmp_tracks};

use crate::ui::covers::CoverCache;
use crate::ui::objects::TrackRow;
use crate::ui::status_page::{StatusPage, status_page};

/// Right-click callback for a leaf row (Phase 16a): `(row position, pointer x,
/// pointer y, the clicked cell widget)`. The window uses it to pop the shared
/// track context menu, translating the cell-local pointer into the `ColumnView`.
/// A `ColumnView` exposes no per-row widget, so the gesture lives on each cell.
pub type RowContextFn = Rc<dyn Fn(u32, f64, f64, gtk::Widget)>;

/// Click-to-rate callback (Phase 16b): `(row position, new rating 0..=5)`.
pub type RowRateFn = Rc<dyn Fn(u32, u8)>;

/// Inline cell-edit commit (the 16c follow-on): `(row position, assignment
/// key, entered value)`. The key is a `Field::parse` spelling, so the window
/// routes title / artist / album / genre through one handler.
pub type RowEditFn = Rc<dyn Fn(u32, &'static str, String)>;

/// The full leaf-column catalog as `(id, display title)` in canonical order
/// (Phase 18b), for the Preferences editor. Each `id` is the `[browse].columns`
/// config token `build_column` maps to a column; the default visible set is
/// `conservatory_core::config::default_columns`.
pub const COLUMN_CATALOG: &[(&str, &str)] = &[
    ("cover", "Cover art"),
    ("glyph", "Playing glyph"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("genre", "Genre"),
    ("title", "Title"),
    ("trackno", "Track number"),
    ("year", "Year"),
    ("duration", "Duration"),
    ("bitrate", "Bitrate"),
    ("format", "Format"),
    ("playcount", "Play count"),
    ("added", "Date added"),
    ("lastplayed", "Last played"),
    ("rating", "Rating"),
];

const MAX_STARS: i32 = 5;
/// The browse cover thumbnail edge, in px. A 24px cover keeps album-art-per-row
/// (the deadbeef look) while driving a dense, foobar-tight row height; it is the
/// dominant term in the track row's height, so this is the density knob.
const COVER_PX: i32 = 24;
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
    empty_page: StatusPage,
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
/// sortable by `key`. When `edit_key` is `Some`, a click on the cell of an
/// already-selected row swaps the label for an entry (the Explorer-rename
/// idiom; a first click just selects, and double-click play survives because
/// a double-click's first press lands on a selected row and the second on the
/// editor, which swallows it only when the row was already the target of a
/// deliberate second click).
#[allow(clippy::too_many_arguments)]
fn text_column(
    title: &str,
    expand: bool,
    xalign: f32,
    key: TrackSort,
    field: fn(&TrackRow) -> String,
    on_context: RowContextFn,
    tech: bool,
    edit_key: Option<&'static str>,
    selection: Option<gtk::MultiSelection>,
    on_edit: &RowEditFn,
) -> gtk::ColumnViewColumn {
    let on_edit = on_edit.clone();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        // `hexpand`/`vexpand` let the label fill its cell so the whole row width
        // is right-clickable; `xalign` still governs where the text sits.
        let label = gtk::Label::builder()
            .xalign(xalign)
            .hexpand(true)
            .vexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        // Technical / numeric columns render in the bundled monospace (Phase 13d).
        if tech {
            label.add_css_class("tech");
        }
        item.set_child(Some(&label));

        // Secondary-click opens the row context menu (Phase 16a). `item.position()`
        // is read at click time so it tracks the row even after a re-sort.
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        let on_context = on_context.clone();
        let item_weak = item.downgrade();
        let label_weak = label.downgrade();
        gesture.connect_pressed(move |_, _, x, y| {
            if let (Some(item), Some(label)) = (item_weak.upgrade(), label_weak.upgrade()) {
                on_context(item.position(), x, y, label.upcast::<gtk::Widget>());
            }
        });
        label.add_controller(gesture);

        // Inline editing (the 16c follow-on): second click on a selected cell.
        if let (Some(key), Some(selection)) = (edit_key, selection.as_ref()) {
            let on_edit = on_edit.clone();
            let item_weak = item.downgrade();
            let selection = selection.clone();
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_PRIMARY);
            click.connect_pressed(move |_, _, _, _| {
                let Some(item) = item_weak.upgrade() else {
                    return;
                };
                if !selection.is_selected(item.position()) {
                    return;
                }
                begin_cell_edit(&item, key, on_edit.clone());
            });
            label.add_controller(click);
        }
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

/// Swap a text cell's label for an entry (the inline editor). Enter commits,
/// Escape cancels, focus loss commits (the MusicBee convention). The label is
/// restored in every case; an unchanged value skips the write entirely.
fn begin_cell_edit(item: &gtk::ListItem, key: &'static str, on_edit: RowEditFn) {
    let Some(label) = item.child().and_downcast::<gtk::Label>() else {
        return;
    };
    let old = label.text().to_string();
    let entry = gtk::Entry::new();
    entry.set_text(&old);
    entry.set_hexpand(true);
    entry.set_vexpand(true);
    item.set_child(Some(&entry));
    entry.grab_focus();
    let pos = item.position();
    let finished = Rc::new(std::cell::Cell::new(false));
    let finish = Rc::new({
        let item_weak = item.downgrade();
        let label = label.downgrade();
        let finished = finished.clone();
        let entry = entry.downgrade();
        move |commit: bool| {
            if finished.replace(true) {
                return; // Enter / Escape / focus-loss fire in sequence; first wins.
            }
            if let (Some(item), Some(label)) = (item_weak.upgrade(), label.upgrade()) {
                item.set_child(Some(&label));
            }
            if commit && let Some(entry) = entry.upgrade() {
                let value = entry.text().to_string();
                if value != old {
                    on_edit(pos, key, value);
                }
            }
        }
    });
    {
        let finish = finish.clone();
        entry.connect_activate(move |_| finish(true));
    }
    {
        let finish = finish.clone();
        let controller = gtk::EventControllerKey::new();
        controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                finish(false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        entry.add_controller(controller);
    }
    let controller = gtk::EventControllerFocus::new();
    {
        let finish = finish.clone();
        controller.connect_leave(move |_| finish(true));
    }
    entry.add_controller(controller);
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

/// Map a click at `x` across a stars row of pixel `width` to a new rating, given
/// the row's `current` rating: the star under the pointer (1..=5), or 0 when that
/// star is already the top one (the Apple click-to-clear toggle). Pure, so the
/// geometry is unit-tested without a realized widget.
fn rating_from_click(x: f64, width: f64, current: i32) -> u8 {
    let star_w = width.max(1.0) / f64::from(MAX_STARS);
    let idx = (x.max(0.0) / star_w).floor() as i32;
    let clicked = (idx + 1).clamp(1, MAX_STARS);
    (if current == clicked { 0 } else { clicked }) as u8
}

/// Map a drag-sweep position at `x` across a stars row of pixel `width` to the
/// rating it points at: the star under the pointer, clamped to 1..=5 (releasing
/// before the left edge gives 1, past the right edge gives 5; no zero, a sweep
/// never clears). Pure, like [`rating_from_click`].
fn rating_from_drag(x: f64, width: f64) -> u8 {
    let star_w = width.max(1.0) / f64::from(MAX_STARS);
    let idx = (x.max(0.0) / star_w).floor() as i32;
    (idx + 1).clamp(1, MAX_STARS) as u8
}

/// Fill the star `Box` to `rating` (the shared paint for bind and the live
/// click-to-rate / notify repaint).
fn paint_stars(row: &gtk::Box, rating: i32) {
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
}

/// The Rating column: a fixed row of five symbolic stars, filled to the row's
/// rating. Symbolic icons come from the icon theme (font-independent). Clicking
/// a star sets the rating (Phase 16b), and a press-sweep across the stars sets
/// the value live as the pointer crosses each one (the 0.5.0 drag-sweep). The
/// press previews through the row's `rating` property (paint-only: the property
/// notifies the bound row, the brief and the sorter stay untouched, and the DB
/// write happens once on release through `on_rate`); a plain click keeps the
/// Apple toggle (clicking the current top star clears to 0).
fn rating_column(on_rate: RowRateFn) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("rating-stars");
        for _ in 0..MAX_STARS {
            row.append(&gtk::Image::from_icon_name("non-starred-symbolic"));
        }
        item.set_child(Some(&row));

        // The sweep state: where the press started, whether the pointer ever
        // left the starting star (a click and a sweep that returns are not the
        // same gesture), and the claimed-sequence guarantee that the gesture
        // does not also select / activate / rubber-band the row.
        struct Sweep {
            start_x: f64,
            start_star: u8,
            swept: bool,
        }
        let gesture = gtk::GestureDrag::new();
        gesture.set_button(gtk::gdk::BUTTON_PRIMARY);
        let on_rate = on_rate.clone();
        let item_weak = item.downgrade();
        let row_weak = row.downgrade();
        let sweep_cell = std::rc::Rc::new(std::cell::RefCell::new(None::<Sweep>));
        let begin_item = item_weak.clone();
        let begin_row = row_weak.clone();
        let begin_cell = sweep_cell.clone();
        gesture.connect_drag_begin(move |g, x, _| {
            let (Some(item), Some(row)) = (begin_item.upgrade(), begin_row.upgrade()) else {
                return;
            };
            let Some(track) = item.item().and_downcast::<TrackRow>() else {
                return;
            };
            let width = f64::from(row.width());
            let start_star = rating_from_drag(x, width);
            begin_cell.borrow_mut().replace(Sweep {
                start_x: x,
                start_star,
                swept: false,
            });
            // Claim the sequence so the press is not also activated (double-click
            // play) or turned into a row drag; rating a row is a distinct interaction.
            g.set_state(gtk::EventSequenceState::Claimed);
            // Live preview of the star under the pointer (the Apple clear on a
            // plain click is decided at release, so it does not preview here).
            if start_star != track.rating() {
                track.set_rating(start_star);
            }
        });
        let update_item = item_weak.clone();
        let update_row = row_weak.clone();
        let update_cell = sweep_cell.clone();
        gesture.connect_drag_update(move |_g, dx, _| {
            let mut slot = update_cell.borrow_mut();
            let Some(sweep) = slot.as_mut() else {
                return;
            };
            let (Some(item), Some(row)) = (update_item.upgrade(), update_row.upgrade()) else {
                return;
            };
            let Some(track) = item.item().and_downcast::<TrackRow>() else {
                return;
            };
            let x = sweep.start_x + dx;
            let v = rating_from_drag(x, f64::from(row.width()));
            if v != sweep.start_star {
                sweep.swept = true;
            }
            if v != track.rating() {
                track.set_rating(v);
            }
        });
        let end_item = item_weak.clone();
        let end_row = row_weak.clone();
        let end_cell = sweep_cell.clone();
        gesture.connect_drag_end(move |_, dx, _| {
            let Some(sweep) = end_cell.borrow_mut().take() else {
                return;
            };
            let (Some(item), Some(row)) = (end_item.upgrade(), end_row.upgrade()) else {
                return;
            };
            let Some(track) = item.item().and_downcast::<TrackRow>() else {
                return;
            };
            let x = sweep.start_x + dx;
            let end_star = rating_from_drag(x, f64::from(row.width()));
            // One DB write per gesture, on release. A press that never left its
            // star is a click and keeps the Phase 16b semantics (the current top
            // star clears to 0, the Apple toggle); anything else commits the
            // swept value.
            let commit = if !sweep.swept {
                rating_from_click(x, f64::from(row.width()), i32::from(track.rating()))
            } else {
                end_star
            };
            on_rate(item.position(), commit);
        });
        row.add_controller(gesture);
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let track = track(&item.item().expect("item"));
        let row = item.child().and_downcast::<gtk::Box>().expect("Box");
        paint_stars(&row, i32::from(track.rating()));
        // Repaint just this row's stars when its `rating` property changes (the
        // glyph column's targeted-notify idiom; no full-store rebind).
        let row_weak = row.downgrade();
        let handler = track.connect_rating_notify(move |t| {
            if let Some(row) = row_weak.upgrade() {
                paint_stars(&row, i32::from(t.rating()));
            }
        });
        unsafe {
            row.set_data("rating-handler", handler);
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let Some(row) = item.child().and_downcast::<gtk::Box>()
            && let Some(handler) =
                unsafe { row.steal_data::<glib::SignalHandlerId>("rating-handler") }
            && let Some(obj) = item.item()
        {
            track(&obj).disconnect(handler);
        }
    });
    let col = gtk::ColumnViewColumn::new(Some("Rating"), Some(factory));
    col.set_fixed_width(96);
    col.set_sorter(Some(&column_sorter(TrackSort::Rating)));
    col
}

/// A fixed-width column (the numeric / date columns don't expand).
fn fixed(col: gtk::ColumnViewColumn, width: i32) -> gtk::ColumnViewColumn {
    col.set_fixed_width(width);
    col
}

/// Build the leaf column for a catalog `id` (Phase 18b), or `None` for an unknown
/// id (skipped, the forgiving config idiom). The `text_column` / `cover_column` /
/// `glyph_column` / `rating_column` builders are reused; the numeric / date
/// columns are fixed-width and monospace (`tech`). `id` is the stable config
/// token that `[browse].columns` and the Preferences editor share.
fn build_column(
    id: &str,
    root: &Option<PathBuf>,
    cache: &CoverCache,
    selection: Option<gtk::MultiSelection>,
    on_context: &RowContextFn,
    on_rate: &RowRateFn,
    on_edit: &RowEditFn,
) -> Option<gtk::ColumnViewColumn> {
    let ctx = on_context.clone();
    // The editable text columns (the 16c inline editor): second click on a
    // selected cell edits. Album edits retitle the whole album (the bulk
    // semantics); genre sets the raw multi-value set.
    let col = match id {
        "cover" => cover_column(root.clone(), cache.clone()),
        "glyph" => glyph_column(),
        "artist" => text_column(
            "Artist",
            true,
            0.0,
            TrackSort::Artist,
            TrackRow::artist,
            ctx,
            false,
            Some("artist"),
            selection.clone(),
            on_edit,
        ),
        "album" => text_column(
            "Album",
            true,
            0.0,
            TrackSort::Album,
            TrackRow::album,
            ctx,
            false,
            Some("album"),
            selection.clone(),
            on_edit,
        ),
        "genre" => text_column(
            "Genre",
            true,
            0.0,
            TrackSort::Genre,
            TrackRow::genres,
            ctx,
            false,
            Some("genre"),
            selection.clone(),
            on_edit,
        ),
        "title" => text_column(
            "Title",
            true,
            0.0,
            TrackSort::Title,
            TrackRow::title,
            ctx,
            false,
            Some("title"),
            selection.clone(),
            on_edit,
        ),
        "duration" => fixed(
            text_column(
                "Duration",
                false,
                1.0,
                TrackSort::Duration,
                TrackRow::duration_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            80,
        ),
        "year" => fixed(
            text_column(
                "Year",
                false,
                1.0,
                TrackSort::Year,
                TrackRow::year_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            64,
        ),
        "trackno" => fixed(
            text_column(
                "#",
                false,
                1.0,
                TrackSort::TrackNo,
                TrackRow::track_no_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            48,
        ),
        "format" => fixed(
            text_column(
                "Format",
                false,
                0.0,
                TrackSort::Format,
                TrackRow::format_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            72,
        ),
        "bitrate" => fixed(
            text_column(
                "Bitrate",
                false,
                1.0,
                TrackSort::Bitrate,
                TrackRow::bitrate_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            72,
        ),
        "playcount" => fixed(
            text_column(
                "Plays",
                false,
                1.0,
                TrackSort::PlayCount,
                TrackRow::play_count_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            64,
        ),
        "added" => fixed(
            text_column(
                "Added",
                false,
                0.0,
                TrackSort::Added,
                TrackRow::added_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            108,
        ),
        "lastplayed" => fixed(
            text_column(
                "Last Played",
                false,
                0.0,
                TrackSort::LastPlayed,
                TrackRow::last_played_text,
                ctx,
                true,
                None,
                None,
                on_edit,
            ),
            108,
        ),
        "rating" => rating_column(on_rate.clone()),
        _ => return None,
    };
    Some(col)
}

pub fn build_leaf(
    root: Option<PathBuf>,
    columns: &[String],
    row_style: conservatory_core::config::RowStyle,
    on_context: RowContextFn,
    on_rate: RowRateFn,
    on_edit: RowEditFn,
) -> Leaf {
    let store = gio::ListStore::new::<TrackRow>();
    let cover_cache = CoverCache::new();

    // Wrap the store in a SortListModel so header clicks reorder the rows; its
    // sorter is the ColumnView's own (set once the view exists, just below).
    let sort_model = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
    let selection = gtk::MultiSelection::new(Some(sort_model.clone()));

    let view = gtk::ColumnView::new(Some(selection.clone()));
    // No cell grid (spec §2.4 density pass): the row + column separators boxed
    // every track in; rows read via hover + selection instead (facet_pane matches).
    view.set_show_row_separators(false);
    view.set_show_column_separators(false);
    view.add_css_class("data-table");
    // Scannability option (the post-0.3.0 follow-on): a faint row line or
    // subtle zebra striping, config-driven and off by default, so the default
    // browse renders exactly as before. The rules live in the owned sheet.
    if let Some(class) = row_style.css_class() {
        view.add_css_class(class);
    }

    // Build the configured columns in order (Phase 18b); unknown / duplicate ids
    // are skipped. Fall back to the default set if the config resolves to nothing,
    // so the header is never empty.
    let mut seen = std::collections::HashSet::new();
    let mut added = 0;
    for id in columns {
        if !seen.insert(id.as_str()) {
            continue;
        }
        if let Some(col) = build_column(
            id,
            &root,
            &cover_cache,
            Some(selection.clone()),
            &on_context,
            &on_rate,
            &on_edit,
        ) {
            view.append_column(&col);
            added += 1;
        }
    }
    if added == 0 {
        for id in conservatory_core::config::default_columns() {
            if let Some(col) = build_column(
                &id,
                &root,
                &cover_cache,
                Some(selection.clone()),
                &on_context,
                &on_rate,
                &on_edit,
            ) {
                view.append_column(&col);
            }
        }
    }

    sort_model.set_sorter(view.sorter().as_ref());

    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&view)
        .build();

    // The empty state (Phase 13b; owned composite since Phase 26): a centered
    // status page shown in place of the table when the leaf is empty. Its
    // title / description are set per refresh (no library vs no filter matches).
    let empty_page = status_page(Some(COVER_PLACEHOLDER), "No tracks", None);
    let stack = gtk::Stack::new();
    stack.add_named(&scroller, Some("list"));
    stack.add_named(empty_page.widget(), Some("empty"));

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
                // Both import paths (19b-i drag-drop + the CLI), so the empty
                // state names the interactive one first (16.5b).
                self.empty_page.set_description(Some(
                    "Drop music files or folders here to import them,\n\
                     or from a terminal:\n\
                     conservatory-cli import <library.db> <source folder> <library root>",
                ));
            }
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack.set_visible_child_name("list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{COLUMN_CATALOG, rating_from_click, rating_from_drag};

    #[test]
    fn catalog_has_unique_ids_and_covers_the_defaults() {
        use std::collections::HashSet;
        let ids: HashSet<&str> = COLUMN_CATALOG.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids.len(),
            COLUMN_CATALOG.len(),
            "catalog ids must be unique"
        );
        // Every default column must resolve in the catalog (and thus in build_column).
        for id in conservatory_core::config::default_columns() {
            assert!(
                ids.contains(id.as_str()),
                "default column {id:?} is missing from the catalog"
            );
        }
    }

    // A 100 px stars row: 20 px per star, so [0,20)=1 … [80,100)=5.
    #[test]
    fn click_maps_x_to_the_star_under_it() {
        assert_eq!(rating_from_click(10.0, 100.0, 0), 1);
        assert_eq!(rating_from_click(50.0, 100.0, 0), 3);
        assert_eq!(rating_from_click(90.0, 100.0, 0), 5);
        // The left edge is the first star; past the right edge clamps to five.
        assert_eq!(rating_from_click(0.0, 100.0, 0), 1);
        assert_eq!(rating_from_click(250.0, 100.0, 0), 5);
    }

    // The drag-sweep geometry: the star under the pointer, clamped, with no
    // zero arm (a sweep never clears; the click-to-clear toggle is a click-time
    // decision, not a sweep one).
    #[test]
    fn drag_maps_x_to_the_star_under_it_without_clearing() {
        assert_eq!(rating_from_drag(10.0, 100.0), 1);
        assert_eq!(rating_from_drag(19.9, 100.0), 1);
        assert_eq!(rating_from_drag(20.0, 100.0), 2);
        assert_eq!(rating_from_drag(50.0, 100.0), 3);
        assert_eq!(rating_from_drag(90.0, 100.0), 5);
        assert_eq!(rating_from_drag(500.0, 100.0), 5);
    }

    #[test]
    fn drag_clamps_a_release_before_the_row_to_one_star() {
        assert_eq!(rating_from_drag(-40.0, 100.0), 1);
        assert_eq!(rating_from_drag(0.0, 100.0), 1);
    }

    #[test]
    fn clicking_the_current_top_star_clears_to_zero() {
        // Apple's toggle: re-clicking the filled-to-here star unsets it.
        assert_eq!(rating_from_click(50.0, 100.0, 3), 0);
        assert_eq!(rating_from_click(90.0, 100.0, 5), 0);
        // Clicking a different star sets that rating, not zero.
        assert_eq!(rating_from_click(30.0, 100.0, 3), 2);
        assert_eq!(rating_from_click(70.0, 100.0, 3), 4);
    }
}
