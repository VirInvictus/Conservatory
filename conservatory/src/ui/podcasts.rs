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

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;

use adw::prelude::*;
use gtk::glib;

use conservatory_core::PlayerHandle;
use conservatory_core::db::{
    EpisodeListRow, InboxPolicy, PlayedState, ReadPool, ShowSettings, TriageBucket, WorkerHandle,
    episodes_for_show, episodes_for_tag, episodes_in_bucket, get_show, get_show_settings,
    list_all_tags, list_shows, show_settings_map,
};

use crate::playqueue::{EpisodeSource, build_episode_queue};
use crate::ui::objects::EpisodeRow;

/// What the episode list is currently showing.
#[derive(Clone, Copy)]
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
    selection: gtk::SingleSelection,
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
}

impl Inner {
    fn load(&self, source: Source) {
        *self.current.borrow_mut() = source;
        // The per-show settings affordance and the detail header are only
        // meaningful for a single show; resolve the show title once here.
        let show_title = match source {
            Source::Show(id) => self
                .pool
                .open()
                .ok()
                .and_then(|conn| get_show(&conn, id).ok().flatten())
                .map(|s| s.title)
                .unwrap_or_default(),
            _ => String::new(),
        };
        *self.show_title.borrow_mut() = show_title;
        self.settings_btn
            .set_visible(matches!(source, Source::Show(_)));
        self.store.remove_all();
        for row in &self.read(source) {
            self.store.append(&EpisodeRow::new(row));
        }
        self.show_detail(None);
    }

    fn reload(&self) {
        let source = *self.current.borrow();
        self.load(source);
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

    fn selected(&self) -> Option<EpisodeRow> {
        self.selection.selected_item().and_downcast::<EpisodeRow>()
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
                self.subtitle.set_text("");
                self.notes.set_text("Select an episode to read its notes.");
                self.actions.set_sensitive(false);
            }
        }
    }

    /// Toggle the selected episode between played-fully and unplayed.
    fn toggle_played(&self) {
        if let Some(row) = self.selected() {
            let next = if row.played() == PlayedState::PlayedFully {
                PlayedState::Unplayed
            } else {
                PlayedState::PlayedFully
            };
            self.write_played(row.id(), next);
        }
    }

    fn archive(&self) {
        if let Some(row) = self.selected() {
            self.write_played(row.id(), PlayedState::ArchivedUnlistened);
        }
    }

    fn toggle_star(&self) {
        if let Some(row) = self.selected() {
            let starred = !row.starred();
            let _ = self
                .rt
                .block_on(self.worker.set_episode_starred(row.id(), starred));
            self.reload();
        }
    }

    fn write_played(&self, episode_id: i64, state: PlayedState) {
        let when = (state == PlayedState::PlayedFully).then(now_secs);
        let _ = self
            .rt
            .block_on(self.worker.set_episode_played(episode_id, state, when));
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
        let Some(row) = self.store.item(activated).and_downcast::<EpisodeRow>() else {
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
        let (items, start) = build_episode_queue(std::slice::from_ref(&source), 0, root, &settings);
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

    /// Append the selected episode to the queue tail (Ctrl+Enter).
    fn append_selected(&self) {
        let (Some(player), Some(root), Some(row)) =
            (self.player.as_ref(), self.root.as_ref(), self.selected())
        else {
            return;
        };
        let _ = self
            .rt
            .block_on(self.worker.enqueue_episodes(vec![row.id()]));
        let source = EpisodeSource {
            id: row.id(),
            show_id: row.show_id(),
            audio_path: row.audio_path(),
            audio_url: row.audio_url(),
        };
        let settings = self.show_settings_for(std::slice::from_ref(&source));
        let (items, _) = build_episode_queue(std::slice::from_ref(&source), 0, root, &settings);
        if !items.is_empty() {
            player.append(items);
        }
    }

    /// Open the per-show settings dialog for the selected show (Phase
    /// 6b-ii-c-3-c): an `adw::PreferencesGroup` pre-populated from the stored
    /// overrides (or the schema defaults), saved through `upsert_show_settings`.
    /// `anchor` is the gear button, used to root the dialog on the window.
    fn open_settings(self: &Rc<Self>, anchor: &gtk::Button) {
        let Source::Show(show_id) = *self.current.borrow() else {
            return;
        };
        let current = self
            .pool
            .open()
            .ok()
            .and_then(|conn| get_show_settings(&conn, show_id).ok().flatten());
        let cur = current.clone().unwrap_or_else(|| default_settings(show_id));

        let group = adw::PreferencesGroup::new();
        group.set_description(Some(
            "Smart Speed and Voice Boost are saved per show; their audio \
             processing arrives in a later update.",
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
            policy.upcast_ref(),
        ] {
            group.add(row);
        }

        let dialog = adw::AlertDialog::new(Some("Show settings"), None);
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
            let settings = settings_from_form(
                current.as_ref(),
                show_id,
                speed.value(),
                smart.is_active(),
                voice.is_active(),
                intro.value() as u32,
                outro.value() as u32,
                inbox_policy_from_index(policy.selected()),
            );
            let _ = inner
                .rt
                .block_on(inner.worker.upsert_show_settings(settings));
        });
        dialog.present(Some(anchor));
    }
}

/// The schema defaults for a show with no stored `show_settings` row (mirrors
/// the CLI `run_podcast_settings` skeleton and the migration `0006` defaults).
fn default_settings(show_id: i64) -> ShowSettings {
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

/// Build the `ShowSettings` to persist from the dialog's field values,
/// preserving `skip_forward` / `skip_back` from `current` (the panel does not
/// expose those global-inherit fields, so a save must not clobber them).
#[allow(clippy::too_many_arguments)]
fn settings_from_form(
    current: Option<&ShowSettings>,
    show_id: i64,
    speed: f64,
    smart_speed: bool,
    voice_boost: bool,
    skip_intro: u32,
    skip_outro: u32,
    inbox_policy: InboxPolicy,
) -> ShowSettings {
    ShowSettings {
        show_id,
        playback_speed: speed,
        smart_speed,
        voice_boost,
        skip_intro,
        skip_outro,
        skip_forward: current.and_then(|c| c.skip_forward),
        skip_back: current.and_then(|c| c.skip_back),
        inbox_policy,
    }
}

/// Variable-speed bounds for the speed spin row, mirroring `player::profile`
/// (`MIN_SPEED` / `MAX_SPEED`). The authoritative clamp is at playback
/// resolution; this only bounds the input widget.
const MIN_SPEED: f64 = 0.25;
const MAX_SPEED: f64 = 4.0;

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
    let selection = gtk::SingleSelection::builder()
        .model(&store)
        .autoselect(false)
        .can_unselect(true)
        .build();

    let column_view = gtk::ColumnView::new(Some(selection.clone()));
    column_view.add_css_class("data-table");
    column_view.append_column(&state_column());
    column_view.append_column(&download_column());
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
    });
    inner.show_detail(None);

    // The detail-pane gear opens the per-show settings dialog (shown only for a
    // show source; visibility toggled in `load`).
    {
        let inner = inner.clone();
        settings_btn.connect_clicked(move |btn| inner.open_settings(btn));
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

    // Episode selection drives the detail pane (and the action labels).
    {
        let inner = inner.clone();
        selection.connect_selected_item_notify(move |sel| {
            let row = sel.selected_item().and_downcast::<EpisodeRow>();
            inner.show_detail(row.as_ref());
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

    // Sidebar: triage buckets, subscribed shows, then tags. `sources` maps a row
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

    let conn = pool.open().ok();
    let shows = conn
        .as_ref()
        .and_then(|c| list_shows(c).ok())
        .unwrap_or_default();
    if !shows.is_empty() {
        sidebar_list.append(&section_header("Shows"));
        sources.push(None);
        for show in &shows {
            sidebar_list.append(&sidebar_entry(&show.title, "microphone-symbolic"));
            sources.push(Some(Source::Show(show.id)));
        }
    }

    let tags = conn
        .as_ref()
        .and_then(|c| list_all_tags(c).ok())
        .unwrap_or_default();
    if !tags.is_empty() {
        sidebar_list.append(&section_header("Tags"));
        sources.push(None);
        for tag in &tags {
            sidebar_list.append(&sidebar_entry(&tag.name, "tag-symbolic"));
            sources.push(Some(Source::Tag(tag.id)));
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

/// A glyph column for the episode's media availability (v0.0.38): a downloaded
/// episode (a local `audio_path`) vs a stream-only one. ("Downloading" is not a
/// state yet — there is no GUI-triggered download.)
fn download_column() -> gtk::ColumnViewColumn {
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
            let downloaded = row.audio_path().is_some();
            img.set_icon_name(Some(if downloaded {
                "folder-download-symbolic"
            } else {
                "network-wireless-symbolic"
            }));
            img.set_tooltip_text(Some(if downloaded {
                "Downloaded"
            } else {
                "Stream only"
            }));
            img.add_css_class("dim-label");
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
// formatting that backs the list is unit-tested in `objects.rs`. The settings
// dialog's pure form mapping is unit-tested below (it constructs no widgets).
#[cfg(test)]
mod tests {
    use super::*;

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
    fn settings_from_form_applies_edits_and_preserves_skip_fields() {
        // A stored row carries custom global-inherit skip overrides the panel
        // does not expose; a save must keep them.
        let current = ShowSettings {
            show_id: 7,
            playback_speed: 1.0,
            smart_speed: true,
            voice_boost: false,
            skip_intro: 0,
            skip_outro: 0,
            skip_forward: Some(45),
            skip_back: Some(15),
            inbox_policy: InboxPolicy::Inbox,
        };
        let out = settings_from_form(
            Some(&current),
            7,
            1.5,
            false,
            true,
            30,
            20,
            InboxPolicy::AlwaysQueue,
        );
        assert_eq!(out.playback_speed, 1.5);
        assert!(!out.smart_speed);
        assert!(out.voice_boost);
        assert_eq!((out.skip_intro, out.skip_outro), (30, 20));
        assert_eq!(out.inbox_policy, InboxPolicy::AlwaysQueue);
        // Untouched, inherited from `current`.
        assert_eq!(out.skip_forward, Some(45));
        assert_eq!(out.skip_back, Some(15));
    }

    #[test]
    fn settings_from_form_without_current_leaves_skip_fields_unset() {
        let out = settings_from_form(None, 3, 1.0, true, false, 0, 0, InboxPolicy::AlwaysArchive);
        assert_eq!(out.skip_forward, None);
        assert_eq!(out.skip_back, None);
        assert_eq!(out.inbox_policy, InboxPolicy::AlwaysArchive);
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
