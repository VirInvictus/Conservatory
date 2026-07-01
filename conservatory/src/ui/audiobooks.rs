//! The Audiobooks browse tab (Phase 7b-i): a cover-grid **shelf** beside a
//! **detail pane** (a horizontal `gtk::Paned`, the Podcasts-tab layout). The
//! shelf is the first `gtk::GridView` in the app (every other browse is a
//! `ColumnView`) and the first use of the median-cut `accent_rgb` in the GUI.
//!
//! Browse + filter (7b-ii) + bulk edit (7b-iii): the shelf is `MultiSelection`,
//! and a pencil button / `Ctrl+E` opens a bulk-edit dialog over the selection;
//! a path-affecting edit re-shelves the books through the journaled mover behind
//! a confirm. A book becomes a `PlayableItem` only at Phase 7c, so there is still
//! no play / queue action here. Reads go through the pool; writes and the move go
//! through the worker (`apply_book_edit` / `apply_book_reorg`); the player is
//! threaded in for 7c but unused now.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use conservatory_audiobooks::edit::{
    book_edit_commons, parse_opt_index, parse_opt_rating, parse_opt_year, split_people, BookEdit,
    SeriesEdit,
};
use conservatory_audiobooks::{apply_book_edit, apply_book_reorg, plan_book_reorg};
use conservatory_core::db::{
    book_chapters, get_book, get_book_playback, list_book_rows, sort_shelf_by, BookListRow,
    BookPlayback, ReadPool, ShelfSort, WorkerHandle,
};
use conservatory_core::mover::MoveMode;
use conservatory_core::PlayerHandle;

use crate::book_query::filter_books;
use crate::query::PoolResolver;
use crate::ui::objects::BookRow;

/// A rejected bulk-edit attempt's state, for the re-present-prefilled loop
/// (16.5g): the per-field `(key, ticked, entered text)` triples plus the
/// Standalone toggle.
type EditAttempt = (Vec<(String, bool, String)>, bool);

/// The fixed pixel size of a shelf cover tile's artwork.
const COVER_SIZE: i32 = 132;

/// Build the Audiobooks view (the `build_podcasts_view` signature, so the lazy
/// `::map` wiring in `window.rs` is identical).
pub fn build_audiobooks_view(
    pool: ReadPool,
    worker: WorkerHandle,
    rt: tokio::runtime::Handle,
    player: Option<PlayerHandle>,
    root: Option<PathBuf>,
) -> gtk::Widget {
    let store = gtk::gio::ListStore::new::<BookRow>();
    // Multi-select for bulk edit (7b-iii); a plain click still selects one book,
    // so the single-book detail browse is unchanged.
    let selection = gtk::MultiSelection::new(Some(store.clone()));

    // The always-on filter bar (spec §3.4), the music-surface idiom: the grammar
    // searches, there is no separate search mode. Ctrl+F focuses it (below).
    let filter = gtk::SearchEntry::builder()
        .placeholder_text("Filter books: author:, narrator:, series:, is:finished …")
        .hexpand(true)
        .build();

    // The empty-shelf state (16.5g): a call-to-action for a fresh library, or
    // a "no matches" page while the filter narrows to nothing.
    let shelf_empty = adw::StatusPage::builder()
        .icon_name("audio-x-generic-symbolic")
        .title("No audiobooks yet")
        .build();
    let shelf_stack = gtk::Stack::new();

    let detail = Detail::new();
    let inner = Rc::new(Inner {
        pool,
        worker,
        rt,
        player,
        root,
        store,
        selection: selection.clone(),
        detail,
        filter: filter.clone(),
        all_rows: RefCell::new(Vec::new()),
        detail_book: RefCell::new(None),
        accent: crate::ui::accent::AccentProvider::new(),
        menu: OnceCell::new(),
        context_pos: Cell::new(None),
        shelf_stack: shelf_stack.clone(),
        shelf_empty: shelf_empty.clone(),
        sort: Cell::new(ShelfSort::InProgress),
    });

    // The shelf grid: a cover tile per book.
    let grid = gtk::GridView::new(Some(selection.clone()), Some(tile_factory(inner.clone())));
    grid.set_max_columns(8);
    grid.set_min_columns(2);
    grid.set_single_click_activate(false);
    grid.add_css_class("navigation-sidebar");

    // The book context menu (Phase 16a): Play / Add to Queue / Edit…, reusing the
    // existing verbs. A `book.` action group on the grid backs a PopoverMenu
    // parented to it (stashed in `inner` so the tile gesture can pop it).
    {
        let menu = gtk::gio::Menu::new();
        let top = gtk::gio::Menu::new();
        top.append(Some("Play"), Some("book.play"));
        top.append(Some("Add to Queue"), Some("book.queue"));
        menu.append_section(None, &top);
        let edit = gtk::gio::Menu::new();
        edit.append(Some("Edit\u{2026}"), Some("book.edit"));
        menu.append_section(None, &edit);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&grid);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        let _ = inner.menu.set(popover);

        let group = gtk::gio::SimpleActionGroup::new();
        let play = gtk::gio::SimpleAction::new("play", None);
        {
            let inner = inner.clone();
            play.connect_activate(move |_, _| {
                if let Some(pos) = inner.context_pos.get() {
                    inner.play_from(pos);
                }
            });
        }
        group.add_action(&play);
        let queue = gtk::gio::SimpleAction::new("queue", None);
        {
            let inner = inner.clone();
            queue.connect_activate(move |_, _| inner.append_selected());
        }
        group.add_action(&queue);
        let edit_action = gtk::gio::SimpleAction::new("edit", None);
        {
            let inner = inner.clone();
            edit_action.connect_activate(move |_, _| {
                let win = inner
                    .menu
                    .get()
                    .and_then(|m| m.root())
                    .and_downcast::<gtk::Window>();
                inner.prompt_bulk_edit(win.as_ref());
            });
        }
        group.add_action(&edit_action);
        grid.insert_action_group("book", Some(&group));
    }

    // Selection drives the detail pane: it follows the first selected book (a
    // plain click selects one, so this is the lone book in the common case).
    {
        let inner = inner.clone();
        selection.connect_selection_changed(move |_, _, _| {
            inner.show_detail(inner.first_selected().as_ref());
        });
    }

    // Double-click / Enter plays the book (Phase 7c-iii): the activated book and
    // the rest of the shelf below it become the queue, the deadbeef idiom.
    {
        let inner = inner.clone();
        grid.connect_activate(move |_, pos| inner.play_from(pos as usize));
    }

    // Activating a chapter row in the detail pane plays that book from the chapter.
    {
        let handler = inner.clone();
        inner.detail.chapters.connect_row_activated(move |_, row| {
            let idx = row.index();
            if idx >= 0 {
                handler.play_detail_chapter(idx as usize);
            }
        });
    }

    // The detail pane's gear opens the per-book playback settings dialog.
    {
        let inner = inner.clone();
        let gear = inner.detail.settings.clone();
        gear.connect_clicked(move |btn| {
            let win = btn.root().and_downcast::<gtk::Window>();
            inner.prompt_book_settings(win.as_ref());
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
    shelf_stack.add_named(&shelf_scroll, Some("grid"));
    shelf_stack.add_named(&shelf_empty, Some("empty"));

    // The filter bar sits above the shelf, in a toolbar strip (the Music-page
    // layout); a sort picker (16.5g) and a pencil button (bulk edit, also
    // Ctrl+E) sit on its right. The detail pane is unaffected by them.
    let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
    edit_btn.set_tooltip_text(Some("Edit selected book(s) (Ctrl+E)"));
    edit_btn.add_css_class("flat");
    {
        let inner = inner.clone();
        edit_btn.connect_clicked(move |btn| {
            inner.prompt_bulk_edit(btn.root().and_downcast::<gtk::Window>().as_ref())
        });
    }
    let sort_dd =
        gtk::DropDown::from_strings(&["In progress first", "Title", "Author", "Recently played"]);
    sort_dd.set_tooltip_text(Some("Shelf order"));
    {
        let inner = inner.clone();
        sort_dd.connect_selected_notify(move |dd| {
            inner.sort.set(match dd.selected() {
                1 => ShelfSort::Title,
                2 => ShelfSort::Author,
                3 => ShelfSort::RecentlyPlayed,
                _ => ShelfSort::InProgress,
            });
            // Re-sort the cached shelf and re-narrow; no DB re-read needed.
            sort_shelf_by(&mut inner.all_rows.borrow_mut(), inner.sort.get());
            inner.apply_filter();
        });
    }
    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    filter_bar.add_css_class("toolbar");
    filter_bar.append(&filter);
    filter_bar.append(&sort_dd);
    filter_bar.append(&edit_btn);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.append(&filter_bar);
    left.append(&shelf_stack);

    inner.load();

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&inner.detail.root));
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(false);
    paned.set_shrink_end_child(false);
    paned.set_position(640);

    // View-scoped shortcuts (Managed scope so they fire only when the Audiobooks
    // tab has focus, never colliding with the window's global music shortcuts):
    // Ctrl+F focuses the filter bar (spec §3.4), Ctrl+E opens the bulk edit.
    let controller = gtk::ShortcutController::new();
    controller.set_scope(gtk::ShortcutScope::Managed);
    let target = filter.downgrade();
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>f"),
        Some(gtk::CallbackAction::new(move |_, _| {
            if let Some(entry) = target.upgrade() {
                entry.grab_focus();
            }
            gtk::glib::Propagation::Stop
        })),
    ));
    let edit_inner = inner.clone();
    let edit_paned = paned.downgrade();
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>e"),
        Some(gtk::CallbackAction::new(move |_, _| {
            let win = edit_paned
                .upgrade()
                .and_then(|p| p.root())
                .and_downcast::<gtk::Window>();
            edit_inner.prompt_bulk_edit(win.as_ref());
            gtk::glib::Propagation::Stop
        })),
    ));
    // Ctrl+Enter appends the selected books to the queue tail (the podcast idiom).
    let append_inner = inner.clone();
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>Return"),
        Some(gtk::CallbackAction::new(move |_, _| {
            append_inner.append_selected();
            gtk::glib::Propagation::Stop
        })),
    ));
    paned.add_controller(controller);
    paned.upcast()
}

/// The view's shared state: the read pool, the library root (to resolve covers),
/// the shelf store, the filter bar, the unfiltered shelf rows, the detail
/// widgets, and the rebuildable accent CSS provider.
struct Inner {
    pool: ReadPool,
    worker: WorkerHandle,
    rt: tokio::runtime::Handle,
    player: Option<PlayerHandle>,
    root: Option<PathBuf>,
    store: gtk::gio::ListStore,
    selection: gtk::MultiSelection,
    detail: Detail,
    filter: gtk::SearchEntry,
    /// The whole shelf, sorted in-progress-first; the filter narrows it into
    /// `store` without re-reading the database.
    all_rows: RefCell<Vec<BookListRow>>,
    /// The book currently shown in the detail pane and its chapters, so a chapter
    /// row activation can play that book from the chapter's book-absolute start.
    detail_book: RefCell<Option<(i64, Vec<conservatory_core::db::BookChapter>)>>,
    /// The shared accent provider (Phase 13c). Unlike the single-accent surfaces,
    /// the shelf serves *many* `.book-accent-RRGGBB` rules at once (one per
    /// distinct tile colour), but the provider-swap is the same; only the CSS the
    /// shelf hands it carries N rules instead of one.
    accent: crate::ui::accent::AccentProvider,
    /// The book right-click menu (Phase 16a), parented to the shelf grid (set once
    /// the grid exists), and the tile position last right-clicked (the Play verb
    /// starts the queue from that book).
    menu: OnceCell<gtk::PopoverMenu>,
    context_pos: Cell<Option<usize>>,
    /// Swaps the grid for the empty-shelf StatusPage (16.5g), with per-cause
    /// copy (fresh library vs no filter matches).
    shelf_stack: gtk::Stack,
    shelf_empty: adw::StatusPage,
    /// The active shelf ordering (16.5g); the DropDown drives it.
    sort: Cell<ShelfSort>,
}

impl Inner {
    /// (Re)load the shelf from the database, ordered in-progress first, register
    /// one accent CSS class per distinct cover accent, then apply the active
    /// filter. Accents cover the whole shelf so tints persist while filtering.
    fn load(&self) {
        let rows = {
            let Ok(conn) = self.pool.open() else { return };
            let mut rows = list_book_rows(&conn).unwrap_or_default();
            sort_shelf_by(&mut rows, self.sort.get());
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
        // Empty shelf: say why (16.5g). A fresh library points at the import
        // path (CLI-only until a GUI importer lands); a filtered miss says so.
        if kept.is_empty() {
            let filtered = !query.trim().is_empty();
            if filtered {
                self.shelf_empty
                    .set_icon_name(Some("system-search-symbolic"));
                self.shelf_empty.set_title("No matches");
                self.shelf_empty
                    .set_description(Some("No books match the current filter."));
            } else {
                self.shelf_empty
                    .set_icon_name(Some("audio-x-generic-symbolic"));
                self.shelf_empty.set_title("No audiobooks yet");
                self.shelf_empty.set_description(Some(
                    "Import a book from a terminal to start your shelf:\n\
                     conservatory-cli audiobook import <library.db> <book folder> <library root>",
                ));
            }
            self.shelf_stack.set_visible_child_name("empty");
        } else {
            self.shelf_stack.set_visible_child_name("grid");
        }
        self.detail.clear();
    }

    /// Route feedback through the window's toast overlay (16.5g, the podcasts
    /// idiom): the action walks the widget tree, no window handle needed.
    fn toast(&self, msg: &str) {
        let _ = self
            .filter
            .activate_action("win.toast", Some(&msg.to_variant()));
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
        self.accent.set_css(&css);
    }

    /// Populate the detail pane for the selected book (or clear it).
    fn show_detail(&self, book: Option<&BookRow>) {
        let Some(book) = book else {
            self.detail.clear();
            *self.detail_book.borrow_mut() = None;
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
        *self.detail_book.borrow_mut() = Some((book.id(), chapters));
    }

    /// Play the detailed book from the activated chapter row (the chapter-list
    /// twin of double-clicking a track): the book becomes the queue and playback
    /// jumps to the chapter's book-absolute start, the timeline the Now Playing
    /// chapter list also seeks on.
    fn play_detail_chapter(&self, idx: usize) {
        let (Some(player), Some(root)) = (self.player.as_ref(), self.root.as_ref()) else {
            return;
        };
        let guard = self.detail_book.borrow();
        let Some((book_id, chapters)) = guard.as_ref() else {
            return;
        };
        let book_id = *book_id;
        let _ = self
            .rt
            .block_on(self.worker.replace_queue_with_books(vec![book_id]));
        let (items, _) = crate::playqueue::build_audiobook_queue(&self.pool, &[book_id], 0, root);
        if items.is_empty() {
            return;
        }
        let start = conservatory_core::player::plan_book(chapters)
            .marks
            .get(idx)
            .map(|m| m.start_time);
        player.play_queue(items, 0);
        if let Some(pos) = start {
            player.seek(pos);
        }
    }

    /// Play the shelf from the activated cover (Phase 7c-iii): the activated book
    /// and everything below it on the current shelf become the unified queue, the
    /// deadbeef idiom (music's "play these, start here"). A book is one queue item
    /// spanning its files; `build_audiobook_queue` reads its chapters → segments +
    /// per-book profile.
    fn play_from(&self, activated: usize) {
        let (Some(player), Some(root)) = (self.player.as_ref(), self.root.as_ref()) else {
            return;
        };
        let ids: Vec<i64> = self.shelf_ids();
        if ids.is_empty() {
            return;
        }
        let _ = self
            .rt
            .block_on(self.worker.replace_queue_with_books(ids.clone()));
        let (items, start) =
            crate::playqueue::build_audiobook_queue(&self.pool, &ids, activated, root);
        if !items.is_empty() {
            player.play_queue(items, start);
        }
    }

    /// Append the selected books to the queue tail (Ctrl+Enter, the podcast
    /// `append_selected` precedent): they flow into the one unified queue.
    fn append_selected(&self) {
        let (Some(player), Some(root)) = (self.player.as_ref(), self.root.as_ref()) else {
            return;
        };
        let ids: Vec<i64> = self.selected_books().iter().map(|b| b.id()).collect();
        if ids.is_empty() {
            return;
        }
        let _ = self.rt.block_on(self.worker.enqueue_books(ids.clone()));
        // Build each selected book as a one-item queue, then append in order.
        let (items, _) = crate::playqueue::build_audiobook_queue(&self.pool, &ids, 0, root);
        if !items.is_empty() {
            player.append(items);
        }
    }

    /// Pop the book context menu at the pointer (Phase 16a). Right-clicking a tile
    /// selects it (so Edit / Add to Queue target it) and records its position (so
    /// Play starts the shelf queue from that book).
    fn show_context_menu(&self, pos: u32, x: f64, y: f64, cell: gtk::Widget) {
        self.context_pos.set(Some(pos as usize));
        if !self.selection.is_selected(pos) {
            self.selection.select_item(pos, true);
        }
        let Some(menu) = self.menu.get() else {
            return;
        };
        if let Some(parent) = menu.parent() {
            let (cx, cy) = cell
                .compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
                .map(|p| (p.x() as i32, p.y() as i32))
                .unwrap_or((x as i32, y as i32));
            menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(cx, cy, 1, 1)));
        }
        menu.popup();
    }

    /// Open the per-book playback settings dialog for the selected book (Phase
    /// 7c-iii, the podcast `open_settings` precedent): speed, Smart Speed, Voice
    /// Boost. On save these per-book overrides are written through
    /// `upsert_book_playback`, preserving the resume position / finished state.
    fn prompt_book_settings(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        let Some(book) = self.first_selected() else {
            return;
        };
        let book_id = book.id();
        let cur = self
            .pool
            .open()
            .ok()
            .and_then(|conn| get_book_playback(&conn, book_id).ok().flatten());

        let group = adw::PreferencesGroup::new();
        group.set_description(Some(
            "Smart Speed trims dead air; Voice Boost lifts quiet, uneven narration. \
             These apply to this book when you play it.",
        ));

        // Speed bounds mirror player::profile's MIN/MAX_SPEED; the authoritative
        // clamp stays at resolve_book_profile, so this cap is only a guard rail.
        let speed = adw::SpinRow::with_range(MIN_SPEED, MAX_SPEED, 0.05);
        speed.set_title("Playback speed");
        speed.set_digits(2);
        speed.set_value(cur.as_ref().and_then(|p| p.speed).unwrap_or(1.0));

        let smart = adw::SwitchRow::new();
        smart.set_title("Smart Speed");
        smart.set_active(cur.as_ref().and_then(|p| p.smart_speed).unwrap_or(false));

        let voice = adw::SwitchRow::new();
        voice.set_title("Voice Boost");
        voice.set_active(cur.as_ref().and_then(|p| p.voice_boost).unwrap_or(false));

        for row in [
            speed.upcast_ref::<gtk::Widget>(),
            smart.upcast_ref(),
            voice.upcast_ref(),
        ] {
            group.add(row);
        }

        let dialog = adw::AlertDialog::new(Some("Playback settings"), None);
        dialog.set_extra_child(Some(&group));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let inner = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "save" {
                return;
            }
            // Preserve the resume position / finished / last_played; only the
            // override columns change.
            let playback = BookPlayback {
                book_id,
                position: cur.as_ref().map(|p| p.position).unwrap_or(0.0),
                finished: cur.as_ref().map(|p| p.finished).unwrap_or(false),
                last_played: cur.as_ref().and_then(|p| p.last_played),
                speed: Some(speed.value()),
                smart_speed: Some(smart.is_active()),
                voice_boost: Some(voice.is_active()),
            };
            let _ = inner
                .rt
                .block_on(inner.worker.upsert_book_playback(playback));
            inner.toast("Playback settings saved");
        });
        dialog.present(parent);
    }

    /// Every book on the current shelf, in display order (the play-from queue).
    fn shelf_ids(&self) -> Vec<i64> {
        let n = self.store.n_items();
        (0..n)
            .filter_map(|i| self.store.item(i).and_downcast::<BookRow>())
            .map(|b| b.id())
            .collect()
    }

    /// Every selected book on the shelf (positions line up: the selection wraps
    /// the displayed store directly).
    fn selected_books(&self) -> Vec<BookRow> {
        let n = self.store.n_items();
        (0..n)
            .filter(|&i| self.selection.is_selected(i))
            .filter_map(|i| self.store.item(i).and_downcast::<BookRow>())
            .collect()
    }

    /// The first selected book (drives the detail pane under multi-select).
    fn first_selected(&self) -> Option<BookRow> {
        let n = self.store.n_items();
        (0..n)
            .find(|&i| self.selection.is_selected(i))
            .and_then(|i| self.store.item(i).and_downcast::<BookRow>())
    }

    /// Open the bulk-edit dialog over the current selection (Phase 7b-iii;
    /// 16.5g brings the Phase 16c music treatment): a checkbox, label, and
    /// entry per field, pre-filled with the selection's shared value or a
    /// "multiple values" hint, plus the "Standalone" toggle (the explicit
    /// series-clear). Only ticked fields write; a bad value rejects the whole
    /// set and re-presents the dialog with everything intact (the 16.5a idiom).
    fn prompt_bulk_edit(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        self.prompt_bulk_edit_prefilled(parent, None);
    }

    fn prompt_bulk_edit_prefilled(
        self: &Rc<Self>,
        parent: Option<&gtk::Window>,
        prefill: Option<EditAttempt>,
    ) {
        let books = self.selected_books();
        if books.is_empty() {
            return;
        }
        // The shared-value prefill: rows carry most fields; the shelf genre is
        // resolved per book (BookListRow does not carry it).
        let commons = {
            let rows: Vec<BookListRow> = books.iter().map(|b| b.row()).collect();
            let shelf_genres: Vec<String> = self
                .pool
                .open()
                .ok()
                .map(|conn| {
                    rows.iter()
                        .map(|r| {
                            get_book(&conn, r.id)
                                .ok()
                                .flatten()
                                .and_then(|b| b.shelf_genre)
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            book_edit_commons(&rows, shelf_genres)
        };
        let (prefill_fields, prefill_standalone) = match prefill {
            Some((fields, standalone)) => {
                let map: std::collections::HashMap<String, (bool, String)> = fields
                    .into_iter()
                    .map(|(k, ticked, v)| (k, (ticked, v)))
                    .collect();
                (Some(map), standalone)
            }
            None => (None, false),
        };

        let fields: [(&str, &str); 8] = [
            ("author", "Author(s) (; separated)"),
            ("narrator", "Narrator(s) (; separated)"),
            ("series", "Series"),
            ("series_index", "Series index"),
            ("title", "Title"),
            ("year", "Year"),
            ("shelfgenre", "Shelf genre"),
            ("rating", "Rating (0-5)"),
        ];
        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        let mut entries: Vec<(String, gtk::CheckButton, gtk::Entry)> = Vec::new();
        for (r, (key, label)) in fields.iter().enumerate() {
            let check = gtk::CheckButton::builder()
                .tooltip_text(
                    "Write this field to every selected book (overwrites differing values)",
                )
                .valign(gtk::Align::Center)
                .build();
            let lbl = gtk::Label::builder().label(*label).xalign(1.0).build();
            let entry = gtk::Entry::builder().hexpand(true).build();
            match commons.get(*key).cloned().flatten() {
                Some(v) if !v.is_empty() => {
                    entry.set_text(&v);
                    entry.set_placeholder_text(Some("unchanged"));
                }
                Some(_) => entry.set_placeholder_text(Some("unchanged")),
                None => entry.set_placeholder_text(Some("multiple values")),
            }
            let prefilled = prefill_fields.as_ref().and_then(|p| p.get(*key));
            if let Some((_, value)) = prefilled {
                entry.set_text(value);
            }
            // Editing ticks; connected after the pre-fill so it does not.
            let check_edit = check.clone();
            entry.connect_changed(move |_| check_edit.set_active(true));
            if let Some((ticked, _)) = prefilled {
                check.set_active(*ticked);
            }
            grid.attach(&check, 0, r as i32, 1, 1);
            grid.attach(&lbl, 1, r as i32, 1, 1);
            grid.attach(&entry, 2, r as i32, 1, 1);
            entries.push(((*key).to_string(), check, entry));
        }
        let standalone = gtk::CheckButton::with_label("Standalone (no series)");
        standalone.set_active(prefill_standalone);
        grid.attach(&standalone, 2, fields.len() as i32, 1, 1);

        let dialog = adw::AlertDialog::new(
            Some("Edit book(s)"),
            Some(&format!(
                "Apply to {} selected book(s). Tick a field to write it; shared values are \
                 shown, differing ones read \u{201c}multiple values\u{201d}.",
                books.len()
            )),
        );
        dialog.set_extra_child(Some(&grid));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("apply", "Apply");
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");

        let this = self.clone();
        let parent_weak = parent.map(|w| w.downgrade());
        dialog.connect_response(None, move |_, resp| {
            if resp != "apply" {
                return;
            }
            let entered: Vec<(String, bool, String)> = entries
                .iter()
                .map(|(key, check, entry)| {
                    (key.clone(), check.is_active(), entry.text().to_string())
                })
                .collect();
            let (edit, errors) = build_book_edit(&entered, standalone.is_active());
            let parent = parent_weak.as_ref().and_then(|w| w.upgrade());
            if !errors.is_empty() {
                // Reject the whole set; the error dialog re-presents this one
                // pre-filled so the fix loses nothing (16.5a / A16).
                this.present_book_edit_errors(
                    errors,
                    entered,
                    standalone.is_active(),
                    parent.as_ref(),
                );
                return;
            }
            this.apply_bulk_edit(&books, edit, parent.as_ref());
        });
        dialog.present(parent);
    }

    /// List the parse failures that rejected a book edit, then reopen the edit
    /// dialog pre-filled with the attempt (16.5g; failures went to stderr).
    fn present_book_edit_errors(
        self: &Rc<Self>,
        errors: Vec<String>,
        entered: Vec<(String, bool, String)>,
        standalone: bool,
        parent: Option<&gtk::Window>,
    ) {
        let dialog = adw::AlertDialog::new(Some("Edit not applied"), Some(&errors.join("\n")));
        dialog.add_response("ok", "Fix Values");
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("ok");
        let this = self.clone();
        let parent_weak = parent.map(|w| w.downgrade());
        dialog.connect_response(None, move |_, _| {
            let parent = parent_weak.as_ref().and_then(|w| w.upgrade());
            this.prompt_bulk_edit_prefilled(parent.as_ref(), Some((entered.clone(), standalone)));
        });
        dialog.present(parent);
    }

    /// Write the edit's metadata to every selected book, then re-shelve (behind a
    /// confirm) if a path-affecting field changed, else just reload the shelf.
    fn apply_bulk_edit(
        self: &Rc<Self>,
        books: &[BookRow],
        edit: BookEdit,
        parent: Option<&gtk::Window>,
    ) {
        if edit.is_empty() {
            self.toast("No fields ticked; nothing written");
            return;
        }
        let ids: Vec<i64> = books.iter().map(|b| b.id()).collect();
        let mut failed = 0usize;
        for &id in &ids {
            if let Err(e) = self.rt.block_on(apply_book_edit(&self.worker, id, &edit)) {
                failed += 1;
                eprintln!("conservatory: edit failed for book {id}: {e}");
            }
        }
        // Edits get the same feedback music edits do (16.5g / A22).
        if failed > 0 {
            self.toast(&format!("Edit failed for {failed} book(s); see the log"));
        } else {
            self.toast(&format!("Updated {} book(s)", ids.len()));
        }
        if edit.is_path_affecting() {
            match self.root.clone() {
                Some(root) => {
                    self.confirm_and_reshelve(ids, root, parent);
                    return;
                }
                None => self.toast("No library root; cannot re-shelve the edited book(s)"),
            }
        }
        self.load();
    }

    /// Preview the aggregate move across the edited books, then re-shelve each
    /// through the journaled mover on confirm (the music `confirm_and_move`
    /// precedent). Always reloads the shelf when the dialog closes.
    fn confirm_and_reshelve(
        self: &Rc<Self>,
        ids: Vec<i64>,
        root: PathBuf,
        parent: Option<&gtk::Window>,
    ) {
        let (mut total, mut conflicts) = (0usize, 0usize);
        for &id in &ids {
            if let Ok(plan) = plan_book_reorg(&self.pool, id, &root) {
                total += plan.ops.len();
                conflicts += plan.conflicts.len();
            }
        }
        if total == 0 && conflicts == 0 {
            self.load(); // nothing to move (e.g. an in-place edit)
            return;
        }
        let body = if conflicts == 0 {
            format!("{total} file(s) will move to match the edit.")
        } else {
            format!(
                "{total} file(s) will move; {conflicts} conflict(s) will be skipped (those books stay put)."
            )
        };
        let dialog = adw::AlertDialog::new(Some("Move files?"), Some(&body));
        dialog.add_response("cancel", "Keep in place");
        dialog.add_response("move", "Move");
        dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("move"));
        dialog.set_close_response("cancel");

        let this = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp == "move" {
                let mut failed = 0usize;
                for &id in &ids {
                    if let Err(e) = this.rt.block_on(apply_book_reorg(
                        &this.worker,
                        &this.pool,
                        id,
                        &root,
                        MoveMode::Move,
                    )) {
                        failed += 1;
                        eprintln!("conservatory: re-shelve failed for book {id}: {e}");
                    }
                }
                if failed > 0 {
                    this.toast(&format!(
                        "Re-shelve failed for {failed} book(s); see the log"
                    ));
                } else {
                    this.toast(&format!("Re-shelved {} book(s)", ids.len()));
                }
            }
            this.load();
        });
        dialog.present(parent);
    }
}

/// Build a `BookEdit` from the dialog's per-field state (16.5g). Only ticked
/// fields contribute; a ticked-but-blank text field is left unchanged (the
/// music-editor semantics; the parse helpers treat blank as `None` too); every
/// parse failure is reported so the caller rejects the whole set. Pure.
fn build_book_edit(
    entered: &[(String, bool, String)],
    standalone: bool,
) -> (BookEdit, Vec<String>) {
    let mut errors = Vec::new();
    let get = |key: &str| -> Option<String> {
        entered
            .iter()
            .find(|(k, ticked, _)| k == key && *ticked)
            .map(|(_, _, v)| v.clone())
    };
    let nonblank = |key: &str| get(key).filter(|v| !v.trim().is_empty());

    let year = match get("year") {
        Some(v) => parse_opt_year(&v).unwrap_or_else(|e| {
            errors.push(e);
            None
        }),
        None => None,
    };
    let series_index = match get("series_index") {
        Some(v) => parse_opt_index(&v).unwrap_or_else(|e| {
            errors.push(e);
            None
        }),
        None => None,
    };
    let rating = match get("rating") {
        Some(v) => parse_opt_rating(&v).unwrap_or_else(|e| {
            errors.push(e);
            None
        }),
        None => None,
    };
    let series = if standalone {
        Some(SeriesEdit::Clear)
    } else {
        nonblank("series").map(SeriesEdit::Set)
    };
    let edit = BookEdit {
        title: nonblank("title"),
        year,
        series,
        series_index,
        authors: nonblank("author").map(|s| split_people(&s)),
        narrators: nonblank("narrator").map(|s| split_people(&s)),
        shelf_genre: nonblank("shelfgenre"),
        rating,
        starred: None,
    };
    (edit, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, ticked: bool, value: &str) -> (String, bool, String) {
        (key.to_string(), ticked, value.to_string())
    }

    #[test]
    fn build_book_edit_honours_ticks_and_collects_errors() {
        // Unticked fields never write, even with text in them.
        let (edit, errors) = build_book_edit(
            &[
                field("title", false, "ignored"),
                field("year", true, "2010"),
            ],
            false,
        );
        assert!(errors.is_empty());
        assert_eq!(edit.title, None);
        assert_eq!(edit.year, Some(2010));

        // Bad numerics report; the valid field still parses (the caller
        // rejects the whole set on any error).
        let (edit, errors) = build_book_edit(
            &[
                field("year", true, "abc"),
                field("rating", true, "9"),
                field("title", true, "kept"),
            ],
            false,
        );
        assert_eq!(errors.len(), 2);
        assert_eq!(edit.title.as_deref(), Some("kept"));

        // Standalone clears the series regardless of the entry.
        let (edit, _) = build_book_edit(&[field("series", true, "Stormlight")], true);
        assert_eq!(edit.series, Some(SeriesEdit::Clear));
    }
}

/// The shelf cover-tile factory: a framed cover above the title and author. The
/// accent class is (re)set on bind so recycled tiles always match their book.
fn tile_factory(inner: Rc<Inner>) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    let setup_inner = inner.clone();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let cover = gtk::Image::new();
        cover.set_pixel_size(COVER_SIZE);
        cover.set_size_request(COVER_SIZE, COVER_SIZE);
        cover.add_css_class("book-cover");

        // The cover sits in an overlay so a finished book can badge its corner
        // (16.5g); the badge is hidden until bind says otherwise.
        let badge = gtk::Image::from_icon_name("object-select-symbolic");
        badge.add_css_class("success");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::Start);
        badge.set_margin_top(4);
        badge.set_margin_end(4);
        badge.set_visible(false);
        badge.set_tooltip_text(Some("Finished"));
        let cover_overlay = gtk::Overlay::new();
        cover_overlay.set_child(Some(&cover));
        cover_overlay.add_overlay(&badge);

        // Listening progress under the cover (16.5g): the shelf answers "how
        // far am I?" without a trip to the detail pane.
        let progress = gtk::ProgressBar::new();
        progress.set_visible(false);
        progress.add_css_class("osd"); // the thin variant

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
        tile.append(&cover_overlay);
        tile.append(&progress);
        tile.append(&title);
        tile.append(&author);
        item.set_child(Some(&tile));

        // Secondary-click opens the book context menu (Phase 16a).
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        let inner = setup_inner.clone();
        let item_weak = item.downgrade();
        let tile_weak = tile.downgrade();
        gesture.connect_pressed(move |_, _, x, y| {
            if let (Some(item), Some(tile)) = (item_weak.upgrade(), tile_weak.upgrade()) {
                inner.show_context_menu(item.position(), x, y, tile.upcast::<gtk::Widget>());
            }
        });
        tile.add_controller(gesture);
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
        // Cover overlay: the artwork plus the finished badge (16.5g).
        if let Some(overlay) = child
            .as_ref()
            .and_then(|c| c.downcast_ref::<gtk::Overlay>())
        {
            if let Some(cover) = overlay.child().and_downcast::<gtk::Image>() {
                match inner.cover_abs(book.cover_path()).filter(|p| p.exists()) {
                    Some(path) => cover.set_from_file(Some(&path)),
                    None => cover.set_icon_name(Some("audio-x-generic-symbolic")),
                }
            }
            // The badge is the overlay child after the main cover.
            if let Some(badge) = overlay
                .child()
                .and_then(|c| c.next_sibling())
                .and_downcast::<gtk::Image>()
            {
                badge.set_visible(book.is_finished());
            }
        }
        // Listening progress (16.5g): shown only mid-book; a finished book
        // shows the badge instead of a pinned-full bar.
        child = child.and_then(|c| c.next_sibling());
        if let Some(bar) = child
            .as_ref()
            .and_then(|c| c.downcast_ref::<gtk::ProgressBar>())
        {
            let in_progress = book.is_in_progress();
            bar.set_visible(in_progress);
            if in_progress {
                bar.set_fraction(book.progress_fraction());
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
    /// Per-book playback settings (Phase 7c-iii): shown only with a book selected.
    settings: gtk::Button,
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

        let settings = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Playback settings (speed, Smart Speed, Voice Boost)")
            .halign(gtk::Align::Start)
            .css_classes(["flat"])
            .visible(false)
            .build();

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
        root.append(&settings);
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
            settings,
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
        self.settings.set_visible(false);
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
        self.settings.set_visible(true);

        clear_list(&self.chapters);
        for ch in chapters {
            self.chapters.append(&chapter_row(ch));
        }
        self.chapters.set_visible(!chapters.is_empty());
    }
}

/// Per-book speed bounds for the settings SpinRow, mirroring `player::profile`'s
/// `MIN_SPEED` / `MAX_SPEED` (the authoritative clamp is `resolve_book_profile`).
/// Crate-visible since 16.5f: the window's Now-bar book-settings dialog shares
/// them.
pub(crate) const MIN_SPEED: f64 = 0.25;
pub(crate) const MAX_SPEED: f64 = 4.0;

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
