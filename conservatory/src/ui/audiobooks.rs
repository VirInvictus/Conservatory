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

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use conservatory_audiobooks::edit::{
    BookEdit, SeriesEdit, parse_opt_index, parse_opt_rating, parse_opt_year, split_people,
};
use conservatory_audiobooks::{apply_book_edit, apply_book_reorg, plan_book_reorg};
use conservatory_core::PlayerHandle;
use conservatory_core::db::{
    BookListRow, BookPlayback, ReadPool, WorkerHandle, book_chapters, get_book_playback,
    list_book_rows, sort_shelf,
};
use conservatory_core::mover::MoveMode;

use crate::book_query::filter_books;
use crate::query::PoolResolver;
use crate::ui::objects::BookRow;

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
        accent_provider: RefCell::new(None),
    });

    // The shelf grid: a cover tile per book.
    let grid = gtk::GridView::new(Some(selection.clone()), Some(tile_factory(inner.clone())));
    grid.set_max_columns(8);
    grid.set_min_columns(2);
    grid.set_single_click_activate(false);
    grid.add_css_class("navigation-sidebar");

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

    // The filter bar sits above the shelf, in a toolbar strip (the Music-page
    // layout); a pencil button on the right opens the bulk-edit dialog over the
    // current selection (also Ctrl+E). The detail pane is unaffected by it.
    let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
    edit_btn.set_tooltip_text(Some("Edit selected book(s) (Ctrl+E)"));
    edit_btn.add_css_class("flat");
    {
        let inner = inner.clone();
        edit_btn.connect_clicked(move |btn| {
            inner.prompt_bulk_edit(btn.root().and_downcast::<gtk::Window>().as_ref())
        });
    }
    let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    filter_bar.add_css_class("toolbar");
    filter_bar.append(&filter);
    filter_bar.append(&edit_btn);
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

    /// Open the bulk-edit dialog over the current selection (Phase 7b-iii): a
    /// labelled-entry grid plus a "Standalone" toggle (the explicit series-clear).
    /// Blank fields are left unchanged; a bad value rejects the whole set.
    fn prompt_bulk_edit(self: &Rc<Self>, parent: Option<&gtk::Window>) {
        let books = self.selected_books();
        if books.is_empty() {
            return;
        }

        let grid = gtk::Grid::builder()
            .row_spacing(6)
            .column_spacing(12)
            .build();
        let mut row = 0;
        let mut add = |label: &str| -> gtk::Entry {
            let lbl = gtk::Label::builder().label(label).xalign(1.0).build();
            let entry = gtk::Entry::builder()
                .placeholder_text("unchanged")
                .hexpand(true)
                .build();
            grid.attach(&lbl, 0, row, 1, 1);
            grid.attach(&entry, 1, row, 1, 1);
            row += 1;
            entry
        };
        let author = add("Author(s) (; separated)");
        let narrator = add("Narrator(s) (; separated)");
        let series = add("Series");
        let series_index = add("Series index");
        let title = add("Title");
        let year = add("Year");
        let shelf_genre = add("Shelf genre");
        let rating = add("Rating (0-5)");
        let standalone = gtk::CheckButton::with_label("Standalone (no series)");
        grid.attach(&standalone, 1, row, 1, 1);

        let dialog = adw::AlertDialog::new(
            Some("Edit book(s)"),
            Some(&format!(
                "Apply to {} selected book(s). Blank fields are left unchanged.",
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
            let nonblank = |e: &gtk::Entry| {
                let t = e.text().to_string();
                (!t.trim().is_empty()).then_some(t)
            };
            let build = || -> Result<BookEdit, String> {
                let series_edit = if standalone.is_active() {
                    Some(SeriesEdit::Clear)
                } else {
                    nonblank(&series).map(SeriesEdit::Set)
                };
                Ok(BookEdit {
                    title: nonblank(&title),
                    year: parse_opt_year(&year.text())?,
                    series: series_edit,
                    series_index: parse_opt_index(&series_index.text())?,
                    authors: nonblank(&author).map(|s| split_people(&s)),
                    narrators: nonblank(&narrator).map(|s| split_people(&s)),
                    shelf_genre: nonblank(&shelf_genre),
                    rating: parse_opt_rating(&rating.text())?,
                    starred: None,
                })
            };
            match build() {
                Ok(edit) => {
                    let parent = parent_weak.as_ref().and_then(|w| w.upgrade());
                    this.apply_bulk_edit(&books, edit, parent.as_ref());
                }
                // Reject the whole set rather than apply a partly-valid edit.
                Err(e) => eprintln!("conservatory: edit not applied: {e}"),
            }
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
            return;
        }
        let ids: Vec<i64> = books.iter().map(|b| b.id()).collect();
        for &id in &ids {
            if let Err(e) = self.rt.block_on(apply_book_edit(&self.worker, id, &edit)) {
                eprintln!("conservatory: edit failed for book {id}: {e}");
            }
        }
        if edit.is_path_affecting() {
            match self.root.clone() {
                Some(root) => {
                    self.confirm_and_reshelve(ids, root, parent);
                    return;
                }
                None => {
                    eprintln!("conservatory: no library root; cannot re-shelve the edited book(s)")
                }
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
                for &id in &ids {
                    if let Err(e) = this.rt.block_on(apply_book_reorg(
                        &this.worker,
                        &this.pool,
                        id,
                        &root,
                        MoveMode::Move,
                    )) {
                        eprintln!("conservatory: re-shelve failed for book {id}: {e}");
                    }
                }
            }
            this.load();
        });
        dialog.present(parent);
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
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

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
