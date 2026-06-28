//! The Audiobooks browse tab (Phase 7b-i): a cover-grid **shelf** beside a
//! **detail pane** (a horizontal `gtk::Paned`, the Podcasts-tab layout). The
//! shelf is the first `gtk::GridView` in the app (every other browse is a
//! `ColumnView`) and the first use of the median-cut `accent_rgb` in the GUI.
//!
//! Read-only: a book becomes a `PlayableItem` only at Phase 7c, so there is no
//! play / queue action here (the 6b-ii-a precedent, browse before playback).
//! Reads go through the pool; the worker / runtime / player are threaded in for
//! the later sub-phases (filter, bulk edit, playback) but unused now.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

use conservatory_core::PlayerHandle;
use conservatory_core::db::{
    BookListRow, ReadPool, WorkerHandle, book_chapters, list_book_rows, sort_shelf,
};

use crate::book_query::filter_books;
use crate::query::PoolResolver;
use crate::ui::objects::BookRow;

/// The fixed pixel size of a shelf cover tile's artwork.
const COVER_SIZE: i32 = 132;

/// Build the Audiobooks view (the `build_podcasts_view` signature, so the lazy
/// `::map` wiring in `window.rs` is identical).
pub fn build_audiobooks_view(
    pool: ReadPool,
    _worker: WorkerHandle,
    _rt: tokio::runtime::Handle,
    _player: Option<PlayerHandle>,
    root: Option<PathBuf>,
) -> gtk::Widget {
    let store = gtk::gio::ListStore::new::<BookRow>();
    let selection = gtk::SingleSelection::builder()
        .model(&store)
        .autoselect(false)
        .can_unselect(true)
        .build();

    // The always-on filter bar (spec §3.4), the music-surface idiom: the grammar
    // searches, there is no separate search mode. Ctrl+F focuses it (below).
    let filter = gtk::SearchEntry::builder()
        .placeholder_text("Filter books: author:, narrator:, series:, is:finished …")
        .hexpand(true)
        .build();

    let detail = Detail::new();
    let inner = Rc::new(Inner {
        pool,
        root,
        store,
        detail,
        filter: filter.clone(),
        all_rows: RefCell::new(Vec::new()),
        accent_provider: RefCell::new(None),
    });

    // The shelf grid: a cover tile per book.
    let grid = gtk::GridView::new(Some(selection.clone()), Some(tile_factory(inner.clone())));
    grid.set_max_columns(8);
    grid.set_min_columns(2);
    grid.set_single_click_activate(false);
    grid.add_css_class("navigation-sidebar");

    // Selection drives the detail pane.
    {
        let inner = inner.clone();
        let selection = selection.clone();
        selection.connect_selected_notify(move |sel| {
            let book = sel.selected_item().and_downcast::<BookRow>();
            inner.show_detail(book.as_ref());
        });
    }

    // Re-filter the shelf in memory on every keystroke. The shelf is tens of rows
    // and the grammar evaluates in memory, so there is no debounce (the music
    // surface coalesces because it re-queries SQLite; here a full re-filter is
    // free). A degraded expression tints the bar yellow (`filter-warn`).
    {
        let inner = inner.clone();
        filter.connect_search_changed(move |_| inner.apply_filter());
    }

    let shelf_scroll = gtk::ScrolledWindow::builder()
        .child(&grid)
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();

    // The filter bar sits above the shelf, in a toolbar strip (the Music-page
    // layout); the detail pane is unaffected by it.
    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    filter_bar.add_css_class("toolbar");
    filter_bar.append(&filter);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.append(&filter_bar);
    left.append(&shelf_scroll);

    inner.load();

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&inner.detail.root));
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(false);
    paned.set_shrink_end_child(false);
    paned.set_position(640);

    // Ctrl+F focuses the filter bar. Scope it to this view (Managed) so it fires
    // when the audiobook shelf has focus, without colliding with the window's
    // global music Ctrl+F (which still serves the Music tab).
    install_filter_shortcut(&paned, &filter);
    paned.upcast()
}

/// Ctrl+F focuses the audiobook filter bar (spec §3.4), scoped to `view` so it
/// only fires when focus is within the Audiobooks tab.
fn install_filter_shortcut(view: &gtk::Paned, filter: &gtk::SearchEntry) {
    let target = filter.downgrade();
    let shortcut = gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>f"),
        Some(gtk::CallbackAction::new(move |_, _| {
            if let Some(entry) = target.upgrade() {
                entry.grab_focus();
            }
            gtk::glib::Propagation::Stop
        })),
    );
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Managed);
    controller.add_shortcut(shortcut);
    view.add_controller(controller);
}

/// The view's shared state: the read pool, the library root (to resolve covers),
/// the shelf store, the filter bar, the unfiltered shelf rows, the detail
/// widgets, and the rebuildable accent CSS provider.
struct Inner {
    pool: ReadPool,
    root: Option<PathBuf>,
    store: gtk::gio::ListStore,
    detail: Detail,
    filter: gtk::SearchEntry,
    /// The whole shelf, sorted in-progress-first; the filter narrows it into
    /// `store` without re-reading the database.
    all_rows: RefCell<Vec<BookListRow>>,
    accent_provider: RefCell<Option<gtk::CssProvider>>,
}

impl Inner {
    /// (Re)load the shelf from the database, ordered in-progress first, register
    /// one accent CSS class per distinct cover accent, then apply the active
    /// filter. Accents cover the whole shelf so tints persist while filtering.
    fn load(&self) {
        let rows = {
            let Ok(conn) = self.pool.open() else { return };
            let mut rows = list_book_rows(&conn).unwrap_or_default();
            sort_shelf(&mut rows);
            rows
        };
        self.rebuild_accent_css(&rows);
        *self.all_rows.borrow_mut() = rows;
        self.apply_filter();
    }

    /// Narrow the cached shelf by the filter-bar grammar into `store`, preserving
    /// the in-progress-first order, and tint the bar when the input degraded.
    fn apply_filter(&self) {
        let query = self.filter.text().to_string();
        let today = chrono::Local::now().date_naive();
        let (kept, warnings) = {
            let rows = self.all_rows.borrow();
            filter_books(&rows, &query, &PoolResolver(&self.pool), today)
        };
        if warnings.is_empty() {
            self.filter.remove_css_class("filter-warn");
        } else {
            self.filter.add_css_class("filter-warn");
        }
        self.store.remove_all();
        for row in &kept {
            self.store.append(&BookRow::new(row));
        }
        self.detail.clear();
    }

    /// Resolve a root-relative cover path to an absolute file path.
    fn cover_abs(&self, rel: Option<String>) -> Option<PathBuf> {
        match (&self.root, rel) {
            (Some(root), Some(rel)) => Some(root.join(rel)),
            _ => None,
        }
    }

    /// Build a display-wide CSS provider carrying one `.book-accent-RRGGBB` rule
    /// per distinct accent (the per-book median-cut tint). A display provider is
    /// the non-deprecated route to dynamic per-tile colour: each tile just adds
    /// its accent class on bind. Rebuilt (old provider removed) on every load.
    fn rebuild_accent_css(&self, rows: &[conservatory_core::db::BookListRow]) {
        let accents: BTreeSet<u32> = rows.iter().filter_map(|r| r.accent_rgb).collect();
        let mut css = String::new();
        for rgb in &accents {
            let hex = rgb & 0x00ff_ffff;
            css.push_str(&format!(
                ".book-accent-{hex:06x} {{ box-shadow: inset 0 -4px 0 #{hex:06x}; }}\n"
            ));
        }
        let Some(display) = gtk::gdk::Display::default() else {
            return;
        };
        if let Some(old) = self.accent_provider.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        if css.is_empty() {
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        *self.accent_provider.borrow_mut() = Some(provider);
    }

    /// Populate the detail pane for the selected book (or clear it).
    fn show_detail(&self, book: Option<&BookRow>) {
        let Some(book) = book else {
            self.detail.clear();
            return;
        };
        let cover = self.cover_abs(book.cover_path());
        let chapters = {
            match self.pool.open() {
                Ok(conn) => book_chapters(&conn, book.id()).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        };
        self.detail.show(book, cover.as_deref(), &chapters);
    }
}

/// The shelf cover-tile factory: a framed cover above the title and author. The
/// accent class is (re)set on bind so recycled tiles always match their book.
fn tile_factory(inner: Rc<Inner>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let cover = gtk::Image::new();
        cover.set_pixel_size(COVER_SIZE);
        cover.set_size_request(COVER_SIZE, COVER_SIZE);
        cover.add_css_class("book-cover");

        let title = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(16)
            .width_chars(16)
            .justify(gtk::Justification::Center)
            .css_classes(["caption-heading"])
            .build();
        let author = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(16)
            .justify(gtk::Justification::Center)
            .css_classes(["caption", "dim-label"])
            .build();

        let tile = gtk::Box::new(gtk::Orientation::Vertical, 4);
        tile.set_halign(gtk::Align::Center);
        tile.append(&cover);
        tile.append(&title);
        tile.append(&author);
        item.set_child(Some(&tile));
    });

    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let (Some(book), Some(tile)) = (
            item.item().and_downcast::<BookRow>(),
            item.child().and_downcast::<gtk::Box>(),
        ) else {
            return;
        };
        let mut child = tile.first_child();
        // Cover.
        if let Some(cover) = child.as_ref().and_then(|c| c.downcast_ref::<gtk::Image>()) {
            match inner.cover_abs(book.cover_path()).filter(|p| p.exists()) {
                Some(path) => cover.set_from_file(Some(&path)),
                None => cover.set_icon_name(Some("audio-x-generic-symbolic")),
            }
        }
        // Title + author labels.
        child = child.and_then(|c| c.next_sibling());
        if let Some(label) = child.as_ref().and_then(|c| c.downcast_ref::<gtk::Label>()) {
            label.set_text(&book.title());
            label.set_tooltip_text(Some(&book.title()));
        }
        child = child.and_then(|c| c.next_sibling());
        if let Some(label) = child.as_ref().and_then(|c| c.downcast_ref::<gtk::Label>()) {
            label.set_text(&book.author_display());
        }

        // Accent tint: reset to the base class set, then add this book's accent
        // class (registered by `rebuild_accent_css`).
        match book.accent_rgb() {
            Some(rgb) => {
                let hex = rgb & 0x00ff_ffff;
                tile.set_css_classes(&["book-tile", &format!("book-accent-{hex:06x}")]);
            }
            None => tile.set_css_classes(&["book-tile"]),
        }
    });

    factory
}

/// The detail pane widgets, updated on selection.
struct Detail {
    root: gtk::Box,
    cover: gtk::Image,
    title: gtk::Label,
    meta: gtk::Label,
    progress: gtk::ProgressBar,
    state: gtk::Label,
    chapters: gtk::ListBox,
    placeholder: gtk::Label,
}

impl Detail {
    fn new() -> Self {
        let cover = gtk::Image::new();
        cover.set_pixel_size(192);
        cover.set_size_request(192, 192);
        cover.add_css_class("book-cover");
        cover.set_halign(gtk::Align::Center);

        let title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-3"])
            .build();
        let meta = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label"])
            .build();
        let state = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["caption"])
            .build();
        let progress = gtk::ProgressBar::new();
        progress.set_show_text(false);

        let chapters = gtk::ListBox::new();
        chapters.set_selection_mode(gtk::SelectionMode::None);
        chapters.add_css_class("boxed-list");
        let chapters_scroll = gtk::ScrolledWindow::builder()
            .child(&chapters)
            .vexpand(true)
            .build();

        let placeholder = gtk::Label::builder()
            .label("Select a book to see its details.")
            .css_classes(["dim-label"])
            .vexpand(true)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .width_request(300)
            .build();
        root.append(&cover);
        root.append(&title);
        root.append(&meta);
        root.append(&state);
        root.append(&progress);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&chapters_scroll);
        root.append(&placeholder);

        let detail = Detail {
            root,
            cover,
            title,
            meta,
            progress,
            state,
            chapters,
            placeholder,
        };
        detail.clear();
        detail
    }

    /// Empty state: hide the metadata widgets, show the placeholder.
    fn clear(&self) {
        self.cover.set_visible(false);
        self.title.set_visible(false);
        self.meta.set_visible(false);
        self.state.set_visible(false);
        self.progress.set_visible(false);
        clear_list(&self.chapters);
        self.chapters.set_visible(false);
        self.placeholder.set_visible(true);
    }

    /// Populate for `book`, with its resolved `cover` path and chapter rows.
    fn show(
        &self,
        book: &BookRow,
        cover: Option<&std::path::Path>,
        chapters: &[conservatory_core::db::BookChapter],
    ) {
        self.placeholder.set_visible(false);
        match cover.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some("audio-x-generic-symbolic")),
        }
        self.cover.set_visible(true);

        self.title.set_text(&book.title());
        self.title.set_visible(true);
        self.meta.set_text(&book.meta_line());
        self.meta.set_visible(!book.meta_line().is_empty());

        let frac = book.progress_fraction();
        self.progress.set_fraction(frac);
        self.progress.set_visible(true);
        self.state.set_text(&format!(
            "{} · {}%",
            book.state_label(),
            (frac * 100.0) as u32
        ));
        self.state.set_visible(true);

        clear_list(&self.chapters);
        for ch in chapters {
            self.chapters.append(&chapter_row(ch));
        }
        self.chapters.set_visible(!chapters.is_empty());
    }
}

/// One chapter list row: "NN. Title          m:ss".
fn chapter_row(ch: &conservatory_core::db::BookChapter) -> gtk::Widget {
    let title = ch
        .title
        .clone()
        .unwrap_or_else(|| format!("Chapter {}", ch.idx + 1));
    let label = gtk::Label::builder()
        .label(format!("{:>2}.  {title}", ch.idx + 1))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();
    let dur = gtk::Label::builder()
        .label(chapter_duration(ch.duration))
        .css_classes(["dim-label", "numeric"])
        .build();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&label);
    row.append(&dur);
    row.add_css_class("chapter-row");
    row.upcast()
}

/// `h:mm:ss` / `m:ss` of a chapter duration, or empty when unknown.
fn chapter_duration(duration: Option<f64>) -> String {
    let secs = duration.unwrap_or(0.0) as i64;
    if secs <= 0 {
        return String::new();
    }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Remove every row from a `ListBox` (no `remove_all` on `ListBox`).
fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}
