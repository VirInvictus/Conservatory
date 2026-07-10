//! The Podcasts triage browse (Phase 6b-ii-a/b/c-1).
//!
//! Fills the 6b-i Podcasts page (spec §3.7): a sidebar of triage buckets
//! (Inbox / Queue / Played), subscribed shows, and tags; an episode list
//! showing each episode's played state; and a detail pane with the show notes
//! plus the triage actions. The module compiles only with the `podcasts`
//! feature. Three flows wire together here:
//!
//! - **Browse (6b-ii-a):** reads through the read pool (`episodes_in_bucket` /
//!   `episodes_for_show` / `episodes_for_tag`), rendered into the `ColumnView`.
//! - **Triage (6b-ii-b):** the detail-pane action bar (mark played / unplayed /
//!   archived, star) writes through the single-writer worker
//!   (`set_episode_played` / `set_episode_starred`), then re-loads the current
//!   source so the list glyph and the bucket counts refresh.
//! - **Playback (6b-ii-c-1):** double-click / Enter plays the visible list from
//!   that row, `Ctrl+Enter` appends; episodes flow into the one unified queue
//!   via `build_episode_queue` + the `PlayerHandle` (streamed or local).
//! - **Per-show settings (6b-ii-c-3-c):** when a show is the selected sidebar
//!   source, a gear button in the detail pane opens a settings dialog (speed,
//!   Smart Speed / Voice Boost, skip intro/outro, inbox policy) writing through
//!   `upsert_show_settings`. The CLI analogue is `podcast settings`.
//!
//! Worker writes are dispatched with `rt.block_on(worker.*)` from the GTK main
//! thread, the app-wide GUI-write idiom (the worker runs on a dedicated runtime
//! thread, so this blocks only for a sub-millisecond command round-trip).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use chrono::{DateTime, Utc};
use gtk::gio;
use gtk::glib;

use conservatory_core::PlayerHandle;
use conservatory_core::db::{
    EpisodeListRow, EpisodeSort, InboxPolicy, PlayedState, ReadPool, ShowSettings, TriageBucket,
    WorkerHandle, episodes_for_show, episodes_for_tag, episodes_in_bucket, get_show,
    get_show_settings, list_all_tags, list_shows, podcast_sidebar_counts, show_settings_map,
};
use conservatory_podcasts::{Fetcher, RefreshOutcome, RefreshStatus};

use crate::playqueue::{EpisodeSource, attach_episode_chapters, build_episode_queue};
use crate::ui::objects::EpisodeRow;

/// A context-menu verb: a method on `Inner` taking `&self` (Phase 16a).
type EpisodeVerb = fn(&Inner);

/// What the episode list is currently showing.
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Bucket(TriageBucket),
    Show(i64),
    Tag(i64),
}

/// A triage action button's effect.
#[derive(Clone, Copy)]
enum Action {
    TogglePlayed,
    Archive,
    ToggleStar,
}

/// Shared state for the episode list, detail pane, and triage actions.
struct Inner {
    pool: ReadPool,
    worker: WorkerHandle,
    rt: tokio::runtime::Handle,
    player: Option<PlayerHandle>,
    root: Option<PathBuf>,
    store: gtk::gio::ListStore,
    /// Multi-select (16.5d): the triage verbs act on the whole selection, and
    /// positions resolve through the sort model, never the raw store.
    selection: gtk::MultiSelection,
    current: RefCell<Source>,
    /// The selected show's title, shown in the detail header when no episode is
    /// selected (empty for bucket/tag sources).
    show_title: RefCell<String>,
    title: gtk::Label,
    subtitle: gtk::Label,
    notes: gtk::Label,
    actions: gtk::Box,
    played_btn: gtk::Button,
    star_btn: gtk::Button,
    /// Per-show settings affordance, visible only for a show source.
    settings_btn: gtk::Button,
    /// The episode right-click menu (Phase 16a), parented to the episode list.
    menu: gtk::PopoverMenu,
    // --- Subscription lifecycle (16.5c) ---
    /// Shared HTTP fetcher for subscribe / refresh. `None` when the client
    /// failed to build; the browse stays usable offline.
    fetcher: Option<Fetcher>,
    /// The sidebar list plus its row→source map, shared so subscribe /
    /// unsubscribe can rebuild the sidebar in place.
    sidebar_list: gtk::ListBox,
    sources: RefCell<Vec<Option<Source>>>,
    /// The selected show's `last_fetched`, for the detail header (P18).
    show_last_fetched: RefCell<Option<DateTime<Utc>>>,
    /// Swaps the whole tab for a no-subscriptions status page (P6).
    view_stack: gtk::Stack,
    /// Swaps the episode list for a per-source empty page (P6).
    list_stack: gtk::Stack,
    list_empty: crate::ui::status_page::StatusPage,
    /// The sidebar-footer refresh button; insensitive while a batch runs.
    refresh_btn: gtk::Button,
    refresh_busy: Cell<bool>,
    /// The sidebar rows' badge labels (16.5d), refreshed in place after triage
    /// writes so counts follow the data without rebuilding the list.
    count_labels: RefCell<Vec<(Source, gtk::Label)>>,
    // --- Downloads (16.5e) ---
    /// In-flight downloads: episode id → completed fraction. Written from the
    /// tokio download tasks (hence `Arc<Mutex<…>>`), drained into the rows'
    /// `download-fraction` properties by the 500 ms ticker.
    downloads: Arc<Mutex<HashMap<i64, f64>>>,
    /// Whether the progress ticker is installed (it stops itself when the
    /// downloads map drains).
    download_ticker: Cell<bool>,
}

impl Inner {
    fn load(&self, source: Source) {
        *self.current.borrow_mut() = source;
        // The per-show settings affordance and the detail header are only
        // meaningful for a single show; resolve the show title (and its
        // last-fetched stamp, 16.5c) once here.
        let (show_title, show_fetched) = match source {
            Source::Show(id) => self
                .pool
                .open()
                .ok()
                .and_then(|conn| get_show(&conn, id).ok().flatten())
                .map(|s| (s.title, s.last_fetched))
                .unwrap_or_default(),
            _ => (String::new(), None),
        };
        *self.show_title.borrow_mut() = show_title;
        *self.show_last_fetched.borrow_mut() = show_fetched;
        self.settings_btn
            .set_visible(matches!(source, Source::Show(_)));
        let rows = self.read(source);
        self.store.remove_all();
        for row in &rows {
            self.store.append(&EpisodeRow::new(row));
        }
        // An empty list explains itself per source (16.5c) instead of showing
        // bare column headers.
        if rows.is_empty() {
            let (title, description) = empty_copy(source);
            self.list_empty.set_title(title);
            self.list_empty.set_description(Some(description));
            self.list_stack.set_visible_child_name("empty");
        } else {
            self.list_stack.set_visible_child_name("list");
        }
        self.show_detail(None);
    }

    fn reload(&self) {
        let source = *self.current.borrow();
        self.load(source);
        self.refresh_sidebar_counts();
    }

    fn read(&self, source: Source) -> Vec<EpisodeListRow> {
        let Ok(conn) = self.pool.open() else {
            return Vec::new();
        };
        match source {
            Source::Bucket(b) => episodes_in_bucket(&conn, b).unwrap_or_default(),
            Source::Show(id) => episodes_for_show(&conn, id).unwrap_or_default(),
            Source::Tag(id) => episodes_for_tag(&conn, id).unwrap_or_default(),
        }
    }

    /// The first selected row (drives the detail pane; the anchor for toggles).
    fn selected(&self) -> Option<EpisodeRow> {
        (0..self.selection.n_items())
            .find(|i| self.selection.is_selected(*i))
            .and_then(|i| self.selection.item(i).and_downcast::<EpisodeRow>())
    }

    /// Every selected row, in view order (16.5d: the triage verbs are batches).
    fn selected_rows(&self) -> Vec<EpisodeRow> {
        (0..self.selection.n_items())
            .filter(|i| self.selection.is_selected(*i))
            .filter_map(|i| self.selection.item(i).and_downcast::<EpisodeRow>())
            .collect()
    }

    fn show_detail(&self, row: Option<&EpisodeRow>) {
        match row {
            Some(r) => {
                self.title.set_text(&r.title());
                self.subtitle.set_text(&detail_subtitle(r));
                let notes = r.description();
                if notes.trim().is_empty() {
                    self.notes.set_text("No show notes.");
                } else {
                    // Links stay clickable (16.5f): the stored subset renders
                    // as Pango markup; legacy plain-text rows are escaped.
                    self.notes
                        .set_markup(&conservatory_podcasts::notes_to_markup(&notes));
                }
                self.actions.set_sensitive(true);
                let played = r.played() == PlayedState::PlayedFully;
                self.played_btn.set_label(if played {
                    "Mark unplayed"
                } else {
                    "Mark played"
                });
                self.star_btn
                    .set_label(if r.starred() { "Unstar" } else { "Star" });
            }
            None => {
                self.title.set_text(&self.show_title.borrow());
                // For a show header, the subtitle reports feed freshness (P18).
                let sub = if matches!(*self.current.borrow(), Source::Show(_)) {
                    fmt_last_refreshed(Utc::now(), *self.show_last_fetched.borrow())
                } else {
                    String::new()
                };
                self.subtitle.set_text(&sub);
                self.notes.set_text("Select an episode to read its notes.");
                self.actions.set_sensitive(false);
            }
        }
    }

    /// Toggle the selection between played-fully and unplayed (16.5d: a batch;
    /// the first selected row decides the direction for the whole set).
    fn toggle_played(&self) {
        let rows = self.selected_rows();
        let Some(first) = rows.first() else { return };
        let next = if first.played() == PlayedState::PlayedFully {
            PlayedState::Unplayed
        } else {
            PlayedState::PlayedFully
        };
        self.write_played_batch(&rows, next);
    }

    fn archive(&self) {
        let rows = self.selected_rows();
        if !rows.is_empty() {
            self.write_played_batch(&rows, PlayedState::ArchivedUnlistened);
        }
    }

    /// Mark the selection unplayed (16.5d, the `I` key): an unplayed episode
    /// that is not queued lands back in the derived Inbox.
    fn mark_unplayed_selected(&self) {
        let rows = self.selected_rows();
        if !rows.is_empty() {
            self.write_played_batch(&rows, PlayedState::Unplayed);
        }
    }

    fn toggle_star(&self) {
        let rows = self.selected_rows();
        let Some(first) = rows.first() else { return };
        let starred = !first.starred();
        for row in &rows {
            let _ = self
                .rt
                .block_on(self.worker.set_episode_starred(row.id(), starred));
        }
        if rows.len() > 1 {
            let verb = if starred { "Starred" } else { "Unstarred" };
            self.toast(&format!("{verb} {} episodes", rows.len()));
        }
        self.reload();
    }

    fn write_played_batch(&self, rows: &[EpisodeRow], state: PlayedState) {
        let when = (state == PlayedState::PlayedFully).then(now_secs);
        for row in rows {
            let _ = self
                .rt
                .block_on(self.worker.set_episode_played(row.id(), state, when));
        }
        if rows.len() > 1 {
            let verb = match state {
                PlayedState::PlayedFully => "Marked played",
                PlayedState::Unplayed => "Marked unplayed",
                PlayedState::ArchivedUnlistened => "Archived",
                PlayedState::InProgress => "Updated",
            };
            self.toast(&format!("{verb}: {} episodes", rows.len()));
        }
        self.reload();
    }

    /// Play **just the activated episode** (double-click / Enter). Unlike the
    /// music leaf (where playing a row queues the whole visible album/view, the
    /// deadbeef idiom), a podcast feed can be hundreds of episodes, so pressing
    /// play on one must not dump the entire list into the queue. The Queue is
    /// built deliberately, via triage or Ctrl+Enter (`append_selected`).
    fn play_from(&self, activated: u32) {
        let (Some(player), Some(root)) = (self.player.as_ref(), self.root.as_ref()) else {
            return;
        };
        // Positions come from the view, so resolve through the selection model
        // (which sees the sorted order, 16.5d), never the raw store.
        let Some(row) = self.selection.item(activated).and_downcast::<EpisodeRow>() else {
            return;
        };
        let source = EpisodeSource {
            id: row.id(),
            show_id: row.show_id(),
            audio_path: row.audio_path(),
            audio_url: row.audio_url(),
        };
        let settings = self.show_settings_for(std::slice::from_ref(&source));
        let _ = self
            .rt
            .block_on(self.worker.replace_queue_with_episodes(vec![source.id]));
        let (mut items, start) =
            build_episode_queue(std::slice::from_ref(&source), 0, root, &settings);
        attach_episode_chapters(&mut items, &self.pool);
        if !items.is_empty() {
            player.play_queue(items, start);
        }
    }

    /// Batch-read the per-show overrides for the shows in `sources` (Phase
    /// 6b-ii-c-3-a), so the queue builder can resolve each episode's speed.
    fn show_settings_for(
        &self,
        sources: &[EpisodeSource],
    ) -> std::collections::HashMap<i64, ShowSettings> {
        let show_ids: Vec<i64> = sources.iter().map(|e| e.show_id).collect();
        self.pool
            .open()
            .ok()
            .and_then(|conn| show_settings_map(&conn, &show_ids).ok())
            .unwrap_or_default()
    }

    /// Append the selection to the queue tail (Ctrl+Enter / `Q`; a batch since
    /// 16.5d, in view order).
    fn append_selected(&self) {
        let (Some(player), Some(root)) = (self.player.as_ref(), self.root.as_ref()) else {
            return;
        };
        let rows = self.selected_rows();
        if rows.is_empty() {
            return;
        }
        let ids: Vec<i64> = rows.iter().map(|r| r.id()).collect();
        let _ = self.rt.block_on(self.worker.enqueue_episodes(ids));
        let sources: Vec<EpisodeSource> = rows
            .iter()
            .map(|row| EpisodeSource {
                id: row.id(),
                show_id: row.show_id(),
                audio_path: row.audio_path(),
                audio_url: row.audio_url(),
            })
            .collect();
        let settings = self.show_settings_for(&sources);
        let (mut items, _) = build_episode_queue(&sources, 0, root, &settings);
        attach_episode_chapters(&mut items, &self.pool);
        if !items.is_empty() {
            player.append(items);
        }
        if rows.len() > 1 {
            self.toast(&format!("Queued {} episodes", rows.len()));
        }
        self.reload();
    }

    /// Play the currently selected episode (the context-menu Play verb; the row
    /// is selected by `show_context_menu` first).
    fn play_selected(&self) {
        let pos = (0..self.selection.n_items()).find(|i| self.selection.is_selected(*i));
        if let Some(pos) = pos {
            self.play_from(pos);
        }
    }

    /// Pop the episode context menu at the pointer (Phase 16a). Right-clicking
    /// an **unselected** row selects just it first; right-clicking inside the
    /// selection keeps it, so the verbs can act on the whole batch (16.5d).
    fn show_context_menu(&self, pos: u32, x: f64, y: f64, cell: gtk::Widget) {
        if !self.selection.is_selected(pos) {
            self.selection.select_item(pos, true);
        }
        self.show_detail(self.selected().as_ref());
        if let Some(parent) = self.menu.parent() {
            let (cx, cy) = cell
                .compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
                .map(|p| (p.x() as i32, p.y() as i32))
                .unwrap_or((x as i32, y as i32));
            self.menu
                .set_pointing_to(Some(&gtk::gdk::Rectangle::new(cx, cy, 1, 1)));
        }
        self.menu.popup();
    }

    /// Open the per-show settings dialog for `show_id` (Phase 6b-ii-c-3-c;
    /// since 16.5d also reachable from the episode context menu, so the id is
    /// a parameter rather than the sidebar selection): an
    /// `adw::PreferencesGroup` pre-populated from the stored overrides (or the
    /// schema defaults), saved through `upsert_show_settings`.
    fn open_settings_for(self: &Rc<Self>, show_id: i64) {
        let cur = self
            .pool
            .open()
            .ok()
            .and_then(|conn| get_show_settings(&conn, show_id).ok().flatten())
            .unwrap_or_else(|| default_settings(show_id));

        let group = adw::PreferencesGroup::new();
        group.set_description(Some(
            "Smart Speed trims dead air; Voice Boost lifts quiet, uneven speech. \
             Both apply to this show's episodes when you play them.",
        ));

        // Speed bounds mirror player::profile's MIN/MAX_SPEED (the real clamp
        // stays at resolve_episode_profile, so the UI cap is only a guard rail).
        let speed = adw::SpinRow::with_range(MIN_SPEED, MAX_SPEED, 0.05);
        speed.set_title("Playback speed");
        speed.set_digits(2);
        speed.set_value(cur.playback_speed);

        let smart = adw::SwitchRow::new();
        smart.set_title("Smart Speed");
        smart.set_active(cur.smart_speed);

        let voice = adw::SwitchRow::new();
        voice.set_title("Voice Boost");
        voice.set_active(cur.voice_boost);

        let intro = adw::SpinRow::with_range(0.0, 600.0, 1.0);
        intro.set_title("Skip intro (seconds)");
        intro.set_value(cur.skip_intro as f64);

        let outro = adw::SpinRow::with_range(0.0, 600.0, 1.0);
        outro.set_title("Skip outro (seconds)");
        outro.set_value(cur.skip_outro as f64);

        // The quick-seek amounts (16.5f): the schema fields existed since
        // Phase 6, the dialog finally exposes them. 0 inherits the defaults.
        let back = adw::SpinRow::with_range(0.0, 300.0, 5.0);
        back.set_title("Skip back (seconds)");
        back.set_subtitle("0 uses the default (15)");
        back.set_value(cur.skip_back.unwrap_or(0) as f64);

        let fwd = adw::SpinRow::with_range(0.0, 300.0, 5.0);
        fwd.set_title("Skip forward (seconds)");
        fwd.set_subtitle("0 uses the default (30)");
        fwd.set_value(cur.skip_forward.unwrap_or(0) as f64);

        let policy = adw::ComboRow::new();
        policy.set_title("New episodes");
        policy.set_model(Some(&gtk::StringList::new(&[
            "Add to Inbox",
            "Add to Queue",
            "Archive",
        ])));
        policy.set_selected(inbox_policy_index(cur.inbox_policy));

        for row in [
            speed.upcast_ref::<gtk::Widget>(),
            smart.upcast_ref(),
            voice.upcast_ref(),
            intro.upcast_ref(),
            outro.upcast_ref(),
            back.upcast_ref(),
            fwd.upcast_ref(),
            policy.upcast_ref(),
        ] {
            group.add(row);
        }

        let dialog = adw::AlertDialog::new(Some("Show settings"), None);
        dialog.set_extra_child(Some(&group));
        dialog.add_response("cancel", "Cancel");
        // Unsubscribe lives with the rest of the show's management (16.5c);
        // it hands off to its own destructive confirm.
        dialog.add_response("unsubscribe", "Unsubscribe\u{2026}");
        dialog.set_response_appearance("unsubscribe", adw::ResponseAppearance::Destructive);
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let inner = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp == "unsubscribe" {
                inner.confirm_unsubscribe(show_id);
                return;
            }
            if resp != "save" {
                return;
            }
            let settings = settings_from_form(
                show_id,
                speed.value(),
                smart.is_active(),
                voice.is_active(),
                intro.value() as u32,
                outro.value() as u32,
                skip_override(back.value()),
                skip_override(fwd.value()),
                inbox_policy_from_index(policy.selected()),
            );
            let _ = inner
                .rt
                .block_on(inner.worker.upsert_show_settings(settings));
            inner.toast("Show settings saved");
        });
        dialog.present(Some(&self.title));
    }

    // --- Subscription lifecycle (16.5c) ---

    /// Route feedback through the window's toast overlay: the action walks the
    /// widget tree, so this module needs no window handle.
    fn toast(&self, msg: &str) {
        let _ = self
            .title
            .activate_action("win.toast", Some(&msg.to_variant()));
    }

    /// Rebuild the sidebar (triage / shows / tags) from storage, keeping the
    /// shared row→source map in step, and swap in the no-subscriptions page
    /// when there is nothing to browse. Selects `select` when it names a row,
    /// else the Inbox.
    fn rebuild_sidebar(&self, select: Option<Source>) {
        let list = &self.sidebar_list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let mut sources: Vec<Option<Source>> = Vec::new();
        let mut count_labels: Vec<(Source, gtk::Label)> = Vec::new();

        list.append(&section_header("Triage"));
        sources.push(None);
        for (label, icon, bucket) in [
            ("Inbox", "mail-unread-symbolic", TriageBucket::Inbox),
            ("Queue", "view-list-symbolic", TriageBucket::Queue),
            ("Played", "object-select-symbolic", TriageBucket::Played),
        ] {
            let (row, count) = sidebar_entry(label, icon);
            list.append(&row);
            sources.push(Some(Source::Bucket(bucket)));
            count_labels.push((Source::Bucket(bucket), count));
        }

        let conn = self.pool.open().ok();
        let shows = conn
            .as_ref()
            .and_then(|c| list_shows(c).ok())
            .unwrap_or_default();
        if !shows.is_empty() {
            list.append(&section_header("Shows"));
            sources.push(None);
            for show in &shows {
                let (row, count) = sidebar_entry(&show.title, "microphone-symbolic");
                list.append(&row);
                sources.push(Some(Source::Show(show.id)));
                count_labels.push((Source::Show(show.id), count));
            }
        }

        let tags = conn
            .as_ref()
            .and_then(|c| list_all_tags(c).ok())
            .unwrap_or_default();
        if !tags.is_empty() {
            list.append(&section_header("Tags"));
            sources.push(None);
            for tag in &tags {
                let (row, _) = sidebar_entry(&tag.name, "tag-symbolic");
                list.append(&row);
                sources.push(Some(Source::Tag(tag.id)));
            }
        }

        // The wanted row's index, before the map moves into the shared cell.
        let index = select
            .and_then(|want| sources.iter().position(|s| *s == Some(want)))
            .unwrap_or(1); // Inbox, just after the "Triage" header
        *self.sources.borrow_mut() = sources;
        *self.count_labels.borrow_mut() = count_labels;
        self.refresh_sidebar_counts();

        self.view_stack
            .set_visible_child_name(if shows.is_empty() { "empty" } else { "content" });
        if let Some(row) = list.row_at_index(index as i32) {
            list.select_row(Some(&row));
        }
    }

    /// Refresh the sidebar badge counts in place (16.5d): bucket totals and
    /// per-show unplayed counts, without rebuilding the list (rebuilding would
    /// disturb the selection on every triage write).
    fn refresh_sidebar_counts(&self) {
        let Ok(conn) = self.pool.open() else { return };
        let Ok(counts) = podcast_sidebar_counts(&conn) else {
            return;
        };
        for (source, label) in self.count_labels.borrow().iter() {
            let n = match source {
                Source::Bucket(TriageBucket::Inbox) => counts.inbox,
                Source::Bucket(TriageBucket::Queue) => counts.queue,
                Source::Bucket(TriageBucket::Played) => counts.played,
                Source::Show(id) => counts.unplayed_by_show.get(id).copied().unwrap_or(0),
                Source::Tag(_) => 0,
            };
            // Zero renders as empty rather than a wall of "0"s.
            label.set_text(&if n > 0 { n.to_string() } else { String::new() });
        }
    }

    /// Select a triage bucket row by keyboard (16.5d, `Ctrl+1/2/3`): the
    /// bucket rows sit at indices 1..=3, just after the "Triage" header.
    fn select_bucket(&self, index: i32) {
        if let Some(row) = self.sidebar_list.row_at_index(index) {
            self.sidebar_list.select_row(Some(&row));
        }
    }

    /// The subscribe dialog (P1). On a failed fetch it re-presents with the
    /// URL preserved and the error explained, so a typo costs one keystroke.
    fn prompt_subscribe(self: &Rc<Self>, prefill: &str, error: Option<&str>) {
        let entry = gtk::Entry::builder()
            .hexpand(true)
            .placeholder_text("https://example.com/feed.xml")
            .text(prefill)
            .activates_default(true)
            .build();
        let body = match error {
            Some(e) => format!("The feed could not be added: {e}"),
            None => "Paste the podcast's feed URL.".to_string(),
        };
        let dialog = adw::AlertDialog::new(Some("Subscribe to a podcast"), Some(&body));
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("subscribe", "Subscribe");
        dialog.set_response_appearance("subscribe", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("subscribe"));
        dialog.set_close_response("cancel");
        let inner = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "subscribe" {
                return;
            }
            let url = entry.text().trim().to_string();
            if !url.is_empty() {
                inner.subscribe(url);
            }
        });
        dialog.present(Some(&self.title));
    }

    /// Fetch + store a new subscription off the GTK thread: the network work
    /// runs on the tokio runtime, the completion lands back on the GLib main
    /// context (`spawn_future_local` awaiting the `JoinHandle`).
    fn subscribe(self: &Rc<Self>, url: String) {
        let Some(fetcher) = self.fetcher.clone() else {
            self.toast("Network client unavailable");
            return;
        };
        let worker = self.worker.clone();
        let pool = self.pool.clone();
        let task_url = url.clone();
        let handle = self.rt.spawn(async move {
            conservatory_podcasts::add_show(&worker, &pool, &fetcher, &task_url).await
        });
        self.toast("Subscribing\u{2026}");
        let inner = self.clone();
        glib::spawn_future_local(async move {
            match handle.await {
                Ok(Ok((id, new, total))) => {
                    inner.toast(&format!("Subscribed: {new} new of {total} episode(s)"));
                    inner.rebuild_sidebar(Some(Source::Show(id)));
                }
                Ok(Err(e)) => inner.prompt_subscribe(&url, Some(&e.to_string())),
                Err(e) => inner.toast(&format!("Subscribe task failed: {e}")),
            }
        });
    }

    /// Destructive confirm, then drop the subscription (P2). Episodes,
    /// settings, sessions, chapters, and queue rows cascade in the worker;
    /// downloaded files stay on disk (retention owns file deletion). A playing
    /// episode keeps playing (the engine owns its resolved item), so the queue
    /// drawer is told to re-read the DB.
    fn confirm_unsubscribe(self: &Rc<Self>, show_id: i64) {
        // Resolve the name from the DB: since 16.5d the settings dialog is
        // reachable for a show that is not the sidebar selection.
        let name = self
            .pool
            .open()
            .ok()
            .and_then(|conn| get_show(&conn, show_id).ok().flatten())
            .map(|s| s.title)
            .unwrap_or_default();
        let body = format!(
            "Unsubscribe from \u{201c}{name}\u{201d}? Its episodes leave the library and the \
             queue; downloaded files stay on disk. This cannot be undone."
        );
        let dialog = adw::AlertDialog::new(Some("Unsubscribe?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("unsubscribe", "Unsubscribe");
        dialog.set_response_appearance("unsubscribe", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let inner = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "unsubscribe" {
                return;
            }
            // Engine indexes of the show's queued episodes, read before the
            // cascade deletes the rows: the live engine queue drops them too,
            // so it stays in lock-step with the DB queue (the 16a invariant).
            let doomed =
                crate::playqueue::engine_indexes_where(&inner.pool, |r| r.show_id == Some(show_id));
            let _ = inner.rt.block_on(inner.worker.delete_show(show_id));
            if let Some(player) = inner.player.as_ref() {
                crate::playqueue::remove_engine_items(player, &doomed);
            }
            inner.toast(&format!("Unsubscribed from {name}"));
            inner.rebuild_sidebar(None);
            let _ = inner.title.activate_action("win.reload-queue", None);
        });
        dialog.present(Some(&self.title));
    }

    /// Refresh the selected show, or every subscription (P4). One batch at a
    /// time; the footer button goes insensitive while it runs.
    fn refresh_current(self: &Rc<Self>) {
        if self.refresh_busy.get() {
            return;
        }
        let Some(fetcher) = self.fetcher.clone() else {
            self.toast("Network client unavailable");
            return;
        };
        let show_id = match *self.current.borrow() {
            Source::Show(id) => Some(id),
            _ => None,
        };
        let worker = self.worker.clone();
        let pool = self.pool.clone();
        let handle = self.rt.spawn(async move {
            // Best-effort creds (the CLI idiom): no secret service just means
            // private feeds poll anonymously and 401 as a Failed outcome.
            let creds = conservatory_podcasts::CredentialStore::secret_service()
                .await
                .ok();
            match show_id {
                Some(id) => {
                    let show = {
                        let conn = pool.open()?;
                        get_show(&conn, id)?.ok_or_else(|| {
                            conservatory_podcasts::FetchError::Parse(format!("no show {id}"))
                        })?
                    };
                    conservatory_podcasts::refresh_show(
                        &worker,
                        &pool,
                        &fetcher,
                        show,
                        creds.as_ref(),
                    )
                    .await
                    .map(|o| vec![o])
                }
                None => conservatory_podcasts::refresh_all(&worker, &pool, &fetcher, creds).await,
            }
        });
        self.refresh_busy.set(true);
        self.refresh_btn.set_sensitive(false);
        let inner = self.clone();
        glib::spawn_future_local(async move {
            let result = handle.await;
            inner.refresh_busy.set(false);
            inner.refresh_btn.set_sensitive(true);
            match result {
                Ok(Ok(outcomes)) => {
                    inner.toast(&summarize_refresh(&outcomes));
                    inner.reload();
                }
                Ok(Err(e)) => inner.toast(&format!("Refresh failed: {e}")),
                Err(e) => inner.toast(&format!("Refresh task failed: {e}")),
            }
        });
    }

    /// OPML import (P3): file chooser → parse + upsert (network-free) → a
    /// refresh-all so the new shows' episodes arrive without a second step.
    fn prompt_import_opml(self: &Rc<Self>) {
        let Some(win) = self.title.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let filter = gtk::FileFilter::new();
        filter.add_suffix("opml");
        filter.add_suffix("xml");
        filter.set_name(Some("OPML"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Import OPML")
            .filters(&filters)
            .build();
        let inner = self.clone();
        dialog.open(Some(&win), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return }; // cancelled
            let Some(path) = file.path() else { return };
            let body = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    inner.toast(&format!("Could not read {}: {e}", path.display()));
                    return;
                }
            };
            let worker = inner.worker.clone();
            let pool = inner.pool.clone();
            let handle = inner.rt.spawn(async move {
                conservatory_podcasts::import_opml(&worker, &pool, &body).await
            });
            let inner = inner.clone();
            glib::spawn_future_local(async move {
                match handle.await {
                    Ok(Ok(summary)) => {
                        inner.toast(&format!(
                            "Imported {} subscription(s), {} new; refreshing feeds\u{2026}",
                            summary.total, summary.created
                        ));
                        inner.rebuild_sidebar(None);
                        inner.refresh_current();
                    }
                    Ok(Err(e)) => inner.toast(&format!("OPML import failed: {e}")),
                    Err(e) => inner.toast(&format!("OPML task failed: {e}")),
                }
            });
        });
    }

    // --- Downloads (16.5e) ---

    /// Download the selected episodes (the opt-in path; `auto_download` stays
    /// off by default, spec §5.3). Progress flows from the tokio tasks through
    /// the shared map into each row's `download-fraction` property.
    fn download_selected(self: &Rc<Self>) {
        let Some(root) = self.root.clone() else {
            self.toast("No library root configured");
            return;
        };
        let Some(fetcher) = self.fetcher.clone() else {
            self.toast("Network client unavailable");
            return;
        };
        let selected = self.selected_rows();
        if selected.is_empty() {
            return;
        }
        let rows: Vec<EpisodeRow> = selected
            .into_iter()
            .filter(|r| r.audio_path().is_none())
            .collect();
        if rows.is_empty() {
            self.toast("Already downloaded");
            return;
        }
        for row in rows {
            let id = row.id();
            {
                let mut map = self.downloads.lock().expect("downloads lock");
                if map.contains_key(&id) {
                    continue; // already in flight
                }
                map.insert(id, 0.0);
            }
            let episode = self
                .pool
                .open()
                .ok()
                .and_then(|conn| conservatory_core::db::get_episode(&conn, id).ok().flatten());
            let Some(episode) = episode else {
                self.downloads.lock().expect("downloads lock").remove(&id);
                continue;
            };
            let client = fetcher.client();
            let worker = self.worker.clone();
            let map = self.downloads.clone();
            let task_root = root.clone();
            let handle = self.rt.spawn(async move {
                let progress: conservatory_podcasts::ProgressFn = {
                    let map = map.clone();
                    Arc::new(move |written, expected| {
                        // An unknown total still reads "in flight" (>0).
                        let frac = conservatory_podcasts::download_fraction(written, expected)
                            .unwrap_or(0.0)
                            .max(0.01);
                        if let Ok(mut m) = map.lock() {
                            m.insert(id, frac);
                        }
                    })
                };
                conservatory_podcasts::download_episode_with_progress(
                    &client,
                    &worker,
                    &task_root,
                    &episode,
                    None,
                    Some(progress),
                )
                .await
            });
            let title = row.title();
            let inner = self.clone();
            glib::spawn_future_local(async move {
                let result = handle.await;
                inner.downloads.lock().expect("downloads lock").remove(&id);
                match result {
                    Ok(Ok(_)) => {
                        inner.toast(&format!("Downloaded {title}"));
                        inner.reload();
                    }
                    Ok(Err(e)) => inner.toast(&format!("Download failed: {e}")),
                    Err(e) => inner.toast(&format!("Download task failed: {e}")),
                }
            });
        }
        self.ensure_download_ticker();
    }

    /// Push download fractions into the visible rows twice a second while any
    /// download runs; the source stops itself when the map drains.
    fn ensure_download_ticker(self: &Rc<Self>) {
        if self.download_ticker.get() {
            return;
        }
        self.download_ticker.set(true);
        let inner = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            let snapshot: HashMap<i64, f64> = inner
                .downloads
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default();
            for i in 0..inner.store.n_items() {
                let Some(row) = inner.store.item(i).and_downcast::<EpisodeRow>() else {
                    continue;
                };
                let want = snapshot.get(&row.id()).copied().unwrap_or(0.0);
                if row.download_fraction() != want {
                    row.set_download_fraction(want);
                }
            }
            if snapshot.is_empty() {
                inner.download_ticker.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Delete the selected episodes' downloaded files behind a confirm; the
    /// episodes stay in the library (streaming still works). Rides
    /// `retention::apply`, the same remove + clear-`audio_path` path the
    /// keep-N pruning uses, so there is exactly one file-deletion codepath.
    fn delete_download_selected(self: &Rc<Self>) {
        let Some(root) = self.root.clone() else {
            self.toast("No library root configured");
            return;
        };
        let selected = self.selected_rows();
        if selected.is_empty() {
            return;
        }
        let prunes: Vec<conservatory_podcasts::RetentionPrune> = selected
            .iter()
            .filter(|r| r.audio_path().is_some())
            .map(|r| conservatory_podcasts::RetentionPrune {
                episode_id: r.id(),
                show_id: r.show_id(),
                show_title: r.show_title(),
                episode_title: r.title(),
                audio_path: r.audio_path().expect("filtered to Some"),
            })
            .collect();
        if prunes.is_empty() {
            self.toast("Nothing downloaded in the selection");
            return;
        }
        let body = format!(
            "Delete {} downloaded file(s)? The episodes stay in the library and can still stream.",
            prunes.len()
        );
        let dialog = adw::AlertDialog::new(Some("Delete downloads?"), Some(&body));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let inner = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "delete" {
                return;
            }
            let worker = inner.worker.clone();
            let root = root.clone();
            let prunes = prunes.clone();
            let handle = inner.rt.spawn(async move {
                conservatory_podcasts::retention::apply(&worker, &root, &prunes).await
            });
            let inner = inner.clone();
            glib::spawn_future_local(async move {
                match handle.await {
                    Ok(Ok(n)) => {
                        inner.toast(&format!("Deleted {n} download(s)"));
                        inner.reload();
                    }
                    Ok(Err(e)) => inner.toast(&format!("Delete failed: {e}")),
                    Err(e) => inner.toast(&format!("Delete task failed: {e}")),
                }
            });
        });
        dialog.present(Some(&self.title));
    }

    /// OPML export (P3): every subscription with its tags, to a file.
    fn prompt_export_opml(self: &Rc<Self>) {
        let Some(win) = self.title.root().and_downcast::<gtk::Window>() else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title("Export OPML")
            .initial_name("conservatory-podcasts.opml")
            .build();
        let inner = self.clone();
        dialog.save(Some(&win), gio::Cancellable::NONE, move |res| {
            let Ok(file) = res else { return }; // cancelled
            let Some(path) = file.path() else { return };
            let pool = inner.pool.clone();
            let handle = inner
                .rt
                .spawn(async move { conservatory_podcasts::export_opml(&pool).await });
            let inner = inner.clone();
            glib::spawn_future_local(async move {
                match handle.await {
                    Ok(Ok(xml)) => match std::fs::write(&path, xml) {
                        Ok(()) => {
                            inner.toast(&format!("Exported subscriptions to {}", path.display()));
                        }
                        Err(e) => inner.toast(&format!("Could not write {}: {e}", path.display())),
                    },
                    Ok(Err(e)) => inner.toast(&format!("OPML export failed: {e}")),
                    Err(e) => inner.toast(&format!("OPML task failed: {e}")),
                }
            });
        });
    }
}

/// The schema defaults for a show with no stored `show_settings` row (mirrors
/// the CLI `run_podcast_settings` skeleton and the migration `0006` defaults).
pub(crate) fn default_settings(show_id: i64) -> ShowSettings {
    ShowSettings {
        show_id,
        playback_speed: 1.0,
        smart_speed: true,
        voice_boost: false,
        skip_intro: 0,
        skip_outro: 0,
        skip_forward: None,
        skip_back: None,
        inbox_policy: InboxPolicy::Inbox,
    }
}

/// The `ComboRow` index for an inbox policy (0 = Inbox, 1 = Queue, 2 = Archive).
fn inbox_policy_index(policy: InboxPolicy) -> u32 {
    match policy {
        InboxPolicy::Inbox => 0,
        InboxPolicy::AlwaysQueue => 1,
        InboxPolicy::AlwaysArchive => 2,
    }
}

/// The inbox policy for a `ComboRow` index; an out-of-range index degrades to
/// Inbox (the schema default) rather than panicking.
fn inbox_policy_from_index(index: u32) -> InboxPolicy {
    match index {
        1 => InboxPolicy::AlwaysQueue,
        2 => InboxPolicy::AlwaysArchive,
        _ => InboxPolicy::Inbox,
    }
}

/// A skip SpinRow's value as the stored override: 0 means "inherit the
/// default", stored as `None` (16.5f). Pure.
pub(crate) fn skip_override(value: f64) -> Option<u32> {
    let secs = value as u32;
    (secs > 0).then_some(secs)
}

/// Build the `ShowSettings` to persist from the dialog's field values. Since
/// 16.5f every stored field is on the form (the skip overrides included), so
/// nothing needs preserving from a prior row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn settings_from_form(
    show_id: i64,
    speed: f64,
    smart_speed: bool,
    voice_boost: bool,
    skip_intro: u32,
    skip_outro: u32,
    skip_back: Option<u32>,
    skip_forward: Option<u32>,
    inbox_policy: InboxPolicy,
) -> ShowSettings {
    ShowSettings {
        show_id,
        playback_speed: speed,
        smart_speed,
        voice_boost,
        skip_intro,
        skip_outro,
        skip_forward,
        skip_back,
        inbox_policy,
    }
}

/// Variable-speed bounds for the speed spin row, mirroring `player::profile`
/// (`MIN_SPEED` / `MAX_SPEED`). The authoritative clamp is at playback
/// resolution; this only bounds the input widget.
pub(crate) const MIN_SPEED: f64 = 0.25;
pub(crate) const MAX_SPEED: f64 = 4.0;

/// One toast line for a refresh batch (16.5c). Pure, so unit-tested.
pub(crate) fn summarize_refresh(outcomes: &[RefreshOutcome]) -> String {
    if outcomes.is_empty() {
        return "No subscriptions to refresh".to_string();
    }
    let mut new_episodes = 0usize;
    let mut failed = 0usize;
    for o in outcomes {
        match &o.status {
            RefreshStatus::Updated { new, .. } => new_episodes += new,
            RefreshStatus::NotModified => {}
            RefreshStatus::Failed(_) => failed += 1,
        }
    }
    let mut out = if outcomes.len() == 1 {
        format!("Refreshed {}", outcomes[0].show_title)
    } else {
        format!("Refreshed {} shows", outcomes.len())
    };
    match new_episodes {
        0 => out.push_str(": no new episodes"),
        1 => out.push_str(": 1 new episode"),
        n => out.push_str(&format!(": {n} new episodes")),
    }
    if failed > 0 {
        out.push_str(&format!(" \u{2022} {failed} failed"));
    }
    out
}

/// The show header's feed-freshness line (16.5c). Pure, so unit-tested.
pub(crate) fn fmt_last_refreshed(now: DateTime<Utc>, last: Option<DateTime<Utc>>) -> String {
    let Some(last) = last else {
        return "Never refreshed".to_string();
    };
    match (now - last).num_minutes() {
        m if m < 1 => "Last refreshed just now".to_string(),
        m if m < 60 => format!("Last refreshed {m} min ago"),
        m if m < 60 * 24 => format!("Last refreshed {} h ago", m / 60),
        m => format!("Last refreshed {} day(s) ago", m / (60 * 24)),
    }
}

/// The per-source empty-list copy (16.5c): what an empty episode list means
/// depends on what it is showing.
fn empty_copy(source: Source) -> (&'static str, &'static str) {
    match source {
        Source::Bucket(TriageBucket::Inbox) => (
            "Inbox is empty",
            "New episodes land here when feeds refresh.",
        ),
        Source::Bucket(TriageBucket::Queue) => (
            "Queue is empty",
            "Add episodes from the Inbox or a show (Ctrl+Enter appends).",
        ),
        Source::Bucket(TriageBucket::Played) => (
            "Nothing played yet",
            "Episodes you finish or archive land here.",
        ),
        Source::Show(_) => ("No episodes", "Refresh to fetch this show's episodes."),
        Source::Tag(_) => ("No episodes", "No episodes carry this tag."),
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Build the Podcasts triage view: reads over `pool`, triage writes over
/// `worker` (dispatched on `rt`, the GUI write idiom).
pub fn build_podcasts_view(
    pool: ReadPool,
    worker: WorkerHandle,
    rt: tokio::runtime::Handle,
    player: Option<PlayerHandle>,
    root: Option<PathBuf>,
) -> gtk::Widget {
    let store = gtk::gio::ListStore::new::<EpisodeRow>();

    // The episode context menu resolves its `Inner` lazily: the columns (with the
    // right-click gesture) are built before `Inner` exists (Phase 16a).
    let ctx: Rc<OnceCell<Rc<Inner>>> = Rc::new(OnceCell::new());

    // Sortable headers (16.5d): the columns carry `CustomSorter`s over the
    // headless `cmp_episodes`; the model chain is store → sort → multi-select
    // (the facet-pane shape), so the default order stays the source's (queue
    // position / newest-first) until a header is clicked.
    let column_view = gtk::ColumnView::new(None::<gtk::SelectionModel>);
    column_view.add_css_class("data-table");
    column_view.append_column(&state_column());
    column_view.append_column(&download_column());
    column_view.append_column(&text_column(
        "Episode",
        true,
        ctx.clone(),
        EpisodeRow::title,
        Some(EpisodeSort::Title),
    ));
    column_view.append_column(&text_column(
        "Date",
        false,
        ctx.clone(),
        EpisodeRow::date_text,
        Some(EpisodeSort::Date),
    ));
    column_view.append_column(&text_column(
        "Length",
        false,
        ctx.clone(),
        EpisodeRow::length_text,
        Some(EpisodeSort::Length),
    ));

    let sort_model = gtk::SortListModel::new(Some(store.clone()), column_view.sorter());
    let selection = gtk::MultiSelection::new(Some(sort_model));
    column_view.set_model(Some(&selection));

    // The context menu's PopoverMenu, parented to the episode list. Its actions
    // (an `episode.` group) are wired after `Inner` exists.
    let episode_menu = {
        let menu = gio::Menu::new();
        let top = gio::Menu::new();
        top.append(Some("Play"), Some("episode.play"));
        top.append(Some("Add to Queue"), Some("episode.queue"));
        menu.append_section(None, &top);
        // Media verbs (16.5e): the opt-in download and its undo.
        let media = gio::Menu::new();
        media.append(Some("Download"), Some("episode.download"));
        media.append(
            Some("Delete Download\u{2026}"),
            Some("episode.delete-download"),
        );
        menu.append_section(None, &media);
        let triage = gio::Menu::new();
        triage.append(Some("Mark Played / Unplayed"), Some("episode.played"));
        triage.append(Some("Star / Unstar"), Some("episode.star"));
        triage.append(Some("Archive"), Some("episode.archive"));
        menu.append_section(None, &triage);
        // The show-level verb (16.5d): settings for the episode's show, so the
        // dialog is reachable without hunting the sidebar (P16).
        let show = gio::Menu::new();
        show.append(Some("Show Settings\u{2026}"), Some("episode.settings"));
        menu.append_section(None, &show);
        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(&column_view);
        popover.set_has_arrow(false);
        popover.set_halign(gtk::Align::Start);
        popover
    };
    let list_scroll = gtk::ScrolledWindow::builder()
        .child(&column_view)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Detail pane.
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .css_classes(["title-3"])
        .build();
    // Per-show settings gear, shown only for a show source (wired below).
    let settings_btn = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Show settings")
        .valign(gtk::Align::Start)
        .visible(false)
        .css_classes(["flat"])
        .build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&title);
    header.append(&settings_btn);
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();

    // Triage action bar (insensitive until an episode is selected).
    let played_btn = gtk::Button::with_label("Mark played");
    let archive_btn = gtk::Button::with_label("Archive");
    let star_btn = gtk::Button::with_label("Star");
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    actions.append(&played_btn);
    actions.append(&archive_btn);
    actions.append(&star_btn);
    actions.set_sensitive(false);

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
    detail.append(&header);
    detail.append(&subtitle);
    detail.append(&actions);
    detail.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    detail.append(&notes_scroll);

    // The episode list's per-source empty page (16.5c), swapped in by `load`.
    let list_empty =
        crate::ui::status_page::status_page(Some("microphone-symbolic"), "No episodes", None);
    let list_stack = gtk::Stack::new();
    list_stack.add_named(&list_scroll, Some("list"));
    list_stack.add_named(list_empty.widget(), Some("empty"));

    // The sidebar list is populated by `rebuild_sidebar` (16.5c), so subscribe
    // and unsubscribe can rebuild it in place.
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");

    // Sidebar footer: the subscription-lifecycle toolbar (16.5c).
    let subscribe_btn = gtk::Button::from_icon_name("list-add-symbolic");
    subscribe_btn.set_tooltip_text(Some("Subscribe to a podcast feed"));
    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh the selected show, or all shows (R)"));
    let opml_menu = gio::Menu::new();
    opml_menu.append(Some("Import OPML\u{2026}"), Some("podcast.import-opml"));
    opml_menu.append(Some("Export OPML\u{2026}"), Some("podcast.export-opml"));
    let opml_btn = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text("Import or export subscriptions (OPML)")
        .menu_model(&opml_menu)
        .build();

    // Swaps the whole tab for a no-subscriptions call-to-action (16.5c);
    // children are added below, once the panes are assembled.
    let view_stack = gtk::Stack::new();

    let inner = Rc::new(Inner {
        pool: pool.clone(),
        worker,
        rt,
        player,
        root,
        store,
        selection: selection.clone(),
        current: RefCell::new(Source::Bucket(TriageBucket::Inbox)),
        show_title: RefCell::new(String::new()),
        title,
        subtitle,
        notes,
        actions,
        played_btn: played_btn.clone(),
        star_btn: star_btn.clone(),
        settings_btn: settings_btn.clone(),
        menu: episode_menu,
        fetcher: Fetcher::new().ok(),
        sidebar_list: sidebar_list.clone(),
        sources: RefCell::new(Vec::new()),
        show_last_fetched: RefCell::new(None),
        view_stack: view_stack.clone(),
        list_stack: list_stack.clone(),
        list_empty: list_empty.clone(),
        refresh_btn: refresh_btn.clone(),
        refresh_busy: Cell::new(false),
        count_labels: RefCell::new(Vec::new()),
        downloads: Arc::new(Mutex::new(HashMap::new())),
        download_ticker: Cell::new(false),
    });
    inner.show_detail(None);
    let _ = ctx.set(inner.clone());

    // The episode context-menu actions (Phase 16a): an `episode.` group on the
    // list, reusing the triage/playback verbs (which act on the selection).
    {
        let group = gio::SimpleActionGroup::new();
        let verbs: [(&str, EpisodeVerb); 5] = [
            ("play", Inner::play_selected),
            ("queue", Inner::append_selected),
            ("played", Inner::toggle_played),
            ("star", Inner::toggle_star),
            ("archive", Inner::archive),
        ];
        for (name, verb) in verbs {
            let action = gio::SimpleAction::new(name, None);
            let inner = inner.clone();
            action.connect_activate(move |_, _| verb(&inner));
            group.add_action(&action);
        }
        // Show Settings… needs the Rc (the dialog holds a clone), so it is
        // wired directly rather than through the fn-pointer verb table.
        let settings = gio::SimpleAction::new("settings", None);
        {
            let inner = inner.clone();
            settings.connect_activate(move |_, _| {
                if let Some(row) = inner.selected() {
                    inner.open_settings_for(row.show_id());
                }
            });
        }
        group.add_action(&settings);
        // The download verbs (16.5e) also hold Rc clones (async completions).
        let download = gio::SimpleAction::new("download", None);
        {
            let inner = inner.clone();
            download.connect_activate(move |_, _| inner.download_selected());
        }
        group.add_action(&download);
        let delete_dl = gio::SimpleAction::new("delete-download", None);
        {
            let inner = inner.clone();
            delete_dl.connect_activate(move |_, _| inner.delete_download_selected());
        }
        group.add_action(&delete_dl);
        column_view.insert_action_group("episode", Some(&group));
    }

    // The detail-pane gear opens the per-show settings dialog (shown only for a
    // show source; visibility toggled in `load`).
    {
        let inner = inner.clone();
        settings_btn.connect_clicked(move |_| {
            if let Source::Show(id) = *inner.current.borrow() {
                inner.open_settings_for(id);
            }
        });
    }

    // Double-click / Enter plays the visible list from that row; Ctrl+Enter
    // appends the selection (the music leaf idiom, spec §3.6).
    {
        let inner = inner.clone();
        column_view.connect_activate(move |_, pos| inner.play_from(pos));
    }
    {
        let inner = inner.clone();
        let append = gtk::ShortcutController::new();
        append.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control>Return"),
            Some(gtk::CallbackAction::new(move |_, _| {
                inner.append_selected();
                glib::Propagation::Stop
            })),
        ));
        column_view.add_controller(append);
    }

    // Episode selection drives the detail pane (and the action labels); with
    // multi-select (16.5d) the first selected row is the detail anchor.
    {
        let inner = inner.clone();
        selection.connect_selection_changed(move |_, _, _| {
            inner.show_detail(inner.selected().as_ref());
        });
    }

    // Triage action buttons.
    for (btn, action) in [
        (&played_btn, Action::TogglePlayed),
        (&archive_btn, Action::Archive),
        (&star_btn, Action::ToggleStar),
    ] {
        let inner = inner.clone();
        btn.connect_clicked(move |_| match action {
            Action::TogglePlayed => inner.toggle_played(),
            Action::Archive => inner.archive(),
            Action::ToggleStar => inner.toggle_star(),
        });
    }

    // Row selection reads the shared row→source map (16.5c: the sidebar can be
    // rebuilt, so the map lives on `Inner`, not in this closure).
    {
        let inner = inner.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row
                && let Some(Some(source)) = usize::try_from(row.index())
                    .ok()
                    .and_then(|i| inner.sources.borrow().get(i).copied())
            {
                inner.load(source);
            }
        });
    }

    // The subscription-lifecycle toolbar + its actions (16.5c).
    {
        let inner = inner.clone();
        subscribe_btn.connect_clicked(move |_| inner.prompt_subscribe("", None));
    }
    {
        let inner = inner.clone();
        refresh_btn.connect_clicked(move |_| inner.refresh_current());
    }
    {
        let group = gio::SimpleActionGroup::new();
        let import = gio::SimpleAction::new("import-opml", None);
        {
            let inner = inner.clone();
            import.connect_activate(move |_, _| inner.prompt_import_opml());
        }
        group.add_action(&import);
        let export = gio::SimpleAction::new("export-opml", None);
        {
            let inner = inner.clone();
            export.connect_activate(move |_, _| inner.prompt_export_opml());
        }
        group.add_action(&export);
        view_stack.insert_action_group("podcast", Some(&group));
    }

    // View-scoped keys (16.5c, keymap.md): `R` refreshes, `Ctrl+Shift+O`
    // imports OPML. Scoped to this widget subtree, so dialogs are unaffected
    // (and the view has no text entry for a bare key to collide with).
    {
        let keys = gtk::ShortcutController::new();
        keys.set_scope(gtk::ShortcutScope::Managed);
        let inner_r = inner.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("r"),
            Some(gtk::CallbackAction::new(move |_, _| {
                inner_r.refresh_current();
                glib::Propagation::Stop
            })),
        ));
        let inner_o = inner.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control><Shift>o"),
            Some(gtk::CallbackAction::new(move |_, _| {
                inner_o.prompt_import_opml();
                glib::Propagation::Stop
            })),
        ));
        // Triage keys (16.5d, keymap.md): `Q` queues the selection, `I` marks
        // it unplayed (back to the derived Inbox once unqueued), `Ctrl+1/2/3`
        // jump between the triage buckets.
        let inner_q = inner.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("q"),
            Some(gtk::CallbackAction::new(move |_, _| {
                inner_q.append_selected();
                glib::Propagation::Stop
            })),
        ));
        let inner_i = inner.clone();
        keys.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("i"),
            Some(gtk::CallbackAction::new(move |_, _| {
                inner_i.mark_unplayed_selected();
                glib::Propagation::Stop
            })),
        ));
        for (trigger, index) in [("<Control>1", 1), ("<Control>2", 2), ("<Control>3", 3)] {
            let inner_b = inner.clone();
            keys.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::CallbackAction::new(move |_, _| {
                    inner_b.select_bucket(index);
                    glib::Propagation::Stop
                })),
            ));
        }
        view_stack.add_controller(keys);
    }

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer.add_css_class("toolbar");
    footer.append(&subscribe_btn);
    footer.append(&refresh_btn);
    footer.append(&opml_btn);

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar_list)
        .vexpand(true)
        .width_request(200)
        .build();
    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.append(&sidebar_scroll);
    sidebar_box.append(&footer);

    // Layout: sidebar | (episode list | detail). Nested `gtk::Paned`, matching
    // the music browse body; an adaptive AdwNavigationSplitView is a later
    // refinement.
    let content = gtk::Paned::new(gtk::Orientation::Horizontal);
    content.set_start_child(Some(&list_stack));
    content.set_end_child(Some(&detail));
    content.set_resize_start_child(true);
    content.set_resize_end_child(true);
    content.set_position(520);

    let root = gtk::Paned::new(gtk::Orientation::Horizontal);
    root.set_start_child(Some(&sidebar_box));
    root.set_end_child(Some(&content));
    root.set_resize_start_child(false);
    root.set_shrink_start_child(false);
    root.set_position(200);

    // The no-subscriptions call-to-action (16.5c): the whole tab swaps for a
    // status page until a first feed exists.
    let cta_subscribe = gtk::Button::with_label("Subscribe\u{2026}");
    cta_subscribe.add_css_class("suggested-action");
    cta_subscribe.add_css_class("pill");
    {
        let inner = inner.clone();
        cta_subscribe.connect_clicked(move |_| inner.prompt_subscribe("", None));
    }
    let cta_import = gtk::Button::with_label("Import OPML\u{2026}");
    cta_import.add_css_class("pill");
    {
        let inner = inner.clone();
        cta_import.connect_clicked(move |_| inner.prompt_import_opml());
    }
    let cta_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    cta_box.set_halign(gtk::Align::Center);
    cta_box.append(&cta_subscribe);
    cta_box.append(&cta_import);
    let empty_view = crate::ui::status_page::status_page(
        Some("microphone-symbolic"),
        "No podcast subscriptions",
        Some("Subscribe to a feed to start your podcast library."),
    );
    empty_view.set_child(&cta_box);

    view_stack.add_named(&root, Some("content"));
    view_stack.add_named(empty_view.widget(), Some("empty"));

    // First population: fills the sidebar, selects the Inbox (which loads the
    // episode list), and picks the content-vs-empty page.
    inner.rebuild_sidebar(None);

    view_stack.upcast()
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

/// A sidebar row plus its right-aligned badge label (16.5d): bucket totals /
/// per-show unplayed counts, filled by `refresh_sidebar_counts` (empty at 0).
fn sidebar_entry(text: &str, icon: &str) -> (gtk::ListBoxRow, gtk::Label) {
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
    let count = gtk::Label::builder()
        .xalign(1.0)
        .css_classes(["dim-label", "caption", "numeric"])
        .build();
    b.append(&count);
    row.set_child(Some(&b));
    row.set_tooltip_text(Some(text));
    (row, count)
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

/// A glyph column for the episode's media availability (v0.0.38; 16.5e adds
/// the in-flight state): downloaded / downloading (with a live percent
/// tooltip, repainted via `notify::download-fraction`, the play-glyph
/// targeted-repaint idiom) / stream-only.
fn download_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        item.set_child(Some(&gtk::Image::new()));
    });
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let (Some(row), Some(img)) = (
            item.item().and_downcast::<EpisodeRow>(),
            item.child().and_downcast::<gtk::Image>(),
        ) else {
            return;
        };
        paint_download(&img, &row);
        // Repaint this row when its fraction ticks; the handler is stashed so
        // unbind can disconnect it on recycle (the glyph-column idiom).
        let img_weak = img.downgrade();
        let handler = row.connect_download_fraction_notify(move |row| {
            if let Some(img) = img_weak.upgrade() {
                paint_download(&img, row);
            }
        });
        unsafe {
            img.set_data("download-handler", handler);
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        if let Some(img) = item.child().and_downcast::<gtk::Image>()
            && let Some(handler) =
                unsafe { img.steal_data::<glib::SignalHandlerId>("download-handler") }
            && let Some(row) = item.item().and_downcast::<EpisodeRow>()
        {
            row.disconnect(handler);
        }
    });
    let col = gtk::ColumnViewColumn::new(Some(""), Some(factory));
    col.set_fixed_width(36);
    col
}

/// The download glyph's shared paint, for bind and the live notify repaint.
fn paint_download(img: &gtk::Image, row: &EpisodeRow) {
    let frac = row.download_fraction();
    if frac > 0.0 {
        img.set_icon_name(Some("emblem-synchronizing-symbolic"));
        img.set_tooltip_text(Some(&format!("Downloading\u{2026} {:.0}%", frac * 100.0)));
    } else if row.audio_path().is_some() {
        img.set_icon_name(Some("folder-download-symbolic"));
        img.set_tooltip_text(Some("Downloaded"));
    } else {
        img.set_icon_name(Some("network-wireless-symbolic"));
        img.set_tooltip_text(Some("Stream only"));
    }
    img.add_css_class("dim-label");
}

/// A text column rendering `getter(row)` into an ellipsized label.
fn text_column(
    title: &str,
    expand: bool,
    ctx: Rc<OnceCell<Rc<Inner>>>,
    getter: impl Fn(&EpisodeRow) -> String + 'static,
    sort: Option<EpisodeSort>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("ListItem");
        let label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .vexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        item.set_child(Some(&label));

        // Secondary-click opens the episode context menu (Phase 16a); the shared
        // `Inner` is resolved lazily (the columns are built before it exists).
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
        let ctx = ctx.clone();
        let item_weak = item.downgrade();
        let label_weak = label.downgrade();
        gesture.connect_pressed(move |_, _, x, y| {
            if let (Some(inner), Some(item), Some(label)) =
                (ctx.get(), item_weak.upgrade(), label_weak.upgrade())
            {
                inner.show_context_menu(item.position(), x, y, label.upcast::<gtk::Widget>());
            }
        });
        label.add_controller(gesture);
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
    // Click-to-sort (16.5d): the header drives the headless comparator; the
    // ColumnView owns the ascending/descending toggle.
    if let Some(key) = sort {
        let sorter = gtk::CustomSorter::new(move |a, b| {
            let (Some(a), Some(b)) = (
                a.downcast_ref::<EpisodeRow>(),
                b.downcast_ref::<EpisodeRow>(),
            ) else {
                return gtk::Ordering::Equal;
            };
            match a.cmp_by(b, key) {
                std::cmp::Ordering::Less => gtk::Ordering::Smaller,
                std::cmp::Ordering::Equal => gtk::Ordering::Equal,
                std::cmp::Ordering::Greater => gtk::Ordering::Larger,
            }
        });
        col.set_sorter(Some(&sorter));
    }
    col
}

// The widget tree itself is verified by build + manual launch (the 3b/3c/4b/6b-i
// precedent): a `gtk::init()`-based construction test hangs under cargo's
// multi-threaded runner because GTK must run on the main thread. The row
// formatting that backs the list is unit-tested in `objects.rs`. The settings
// dialog's pure form mapping is unit-tested below (it constructs no widgets).
#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(title: &str, status: RefreshStatus) -> RefreshOutcome {
        RefreshOutcome {
            show_id: 1,
            show_title: title.to_string(),
            status,
        }
    }

    #[test]
    fn summarize_refresh_covers_empty_single_and_batch() {
        assert_eq!(summarize_refresh(&[]), "No subscriptions to refresh");
        assert_eq!(
            summarize_refresh(&[outcome("ATP", RefreshStatus::NotModified)]),
            "Refreshed ATP: no new episodes"
        );
        let batch = [
            outcome("ATP", RefreshStatus::Updated { new: 2, total: 10 }),
            outcome("Upgrade", RefreshStatus::Updated { new: 1, total: 8 }),
            outcome("Dead Feed", RefreshStatus::Failed("410 Gone".into())),
        ];
        assert_eq!(
            summarize_refresh(&batch),
            "Refreshed 3 shows: 3 new episodes \u{2022} 1 failed"
        );
    }

    #[test]
    fn fmt_last_refreshed_buckets_by_age() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let at = |s: &str| Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
        assert_eq!(fmt_last_refreshed(now, None), "Never refreshed");
        assert_eq!(
            fmt_last_refreshed(now, at("2026-07-01T11:59:40Z")),
            "Last refreshed just now"
        );
        assert_eq!(
            fmt_last_refreshed(now, at("2026-07-01T11:15:00Z")),
            "Last refreshed 45 min ago"
        );
        assert_eq!(
            fmt_last_refreshed(now, at("2026-07-01T06:00:00Z")),
            "Last refreshed 6 h ago"
        );
        assert_eq!(
            fmt_last_refreshed(now, at("2026-06-28T12:00:00Z")),
            "Last refreshed 3 day(s) ago"
        );
    }

    #[test]
    fn inbox_policy_index_round_trips() {
        for policy in [
            InboxPolicy::Inbox,
            InboxPolicy::AlwaysQueue,
            InboxPolicy::AlwaysArchive,
        ] {
            assert_eq!(inbox_policy_from_index(inbox_policy_index(policy)), policy);
        }
        // The fixed ComboRow order.
        assert_eq!(inbox_policy_index(InboxPolicy::Inbox), 0);
        assert_eq!(inbox_policy_index(InboxPolicy::AlwaysQueue), 1);
        assert_eq!(inbox_policy_index(InboxPolicy::AlwaysArchive), 2);
    }

    #[test]
    fn inbox_policy_from_out_of_range_index_degrades_to_inbox() {
        assert_eq!(inbox_policy_from_index(99), InboxPolicy::Inbox);
    }

    #[test]
    fn settings_from_form_passes_every_field_through() {
        // Since 16.5f the skip overrides are on the form; a save writes
        // exactly what the dialog shows.
        let out = settings_from_form(
            7,
            1.5,
            false,
            true,
            30,
            20,
            Some(15),
            Some(45),
            InboxPolicy::AlwaysQueue,
        );
        assert_eq!(out.playback_speed, 1.5);
        assert!(!out.smart_speed);
        assert!(out.voice_boost);
        assert_eq!((out.skip_intro, out.skip_outro), (30, 20));
        assert_eq!((out.skip_back, out.skip_forward), (Some(15), Some(45)));
        assert_eq!(out.inbox_policy, InboxPolicy::AlwaysQueue);
    }

    #[test]
    fn skip_override_maps_zero_to_inherit() {
        assert_eq!(skip_override(0.0), None);
        assert_eq!(skip_override(20.0), Some(20));
    }

    #[test]
    fn default_settings_matches_schema_defaults() {
        let d = default_settings(1);
        assert_eq!(d.playback_speed, 1.0);
        assert!(d.smart_speed);
        assert!(!d.voice_boost);
        assert_eq!(d.inbox_policy, InboxPolicy::Inbox);
    }
}
