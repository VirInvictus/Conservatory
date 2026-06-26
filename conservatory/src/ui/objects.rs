//! GObject list-model items for the facet panes and the leaf track table. Plain
//! data carriers (Rust getters, no glib properties) — the `ColumnView` factories
//! read them on bind. Phase 3b.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use conservatory_core::db::{MediaKind, QueueDisplayRow, TrackBrief};

// --- Facet row (a pane entry: a value + its track count) ---

mod facet_imp {
    use super::*;

    #[derive(Default)]
    pub struct FacetRow {
        pub value: RefCell<String>,
        pub count: Cell<i64>,    // track count (the Count column)
        pub is_all: Cell<bool>,  // the synthetic `[All]` row
        pub distinct: Cell<i64>, // distinct value count, for the `[All (N)]` label
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FacetRow {
        const NAME: &'static str = "ConservatoryFacetRow";
        type Type = super::FacetRow;
    }

    impl ObjectImpl for FacetRow {}
}

glib::wrapper! {
    pub struct FacetRow(ObjectSubclass<facet_imp::FacetRow>);
}

impl FacetRow {
    /// A normal value row.
    pub fn value_row(value: &str, count: i64) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().value.replace(value.to_string());
        obj.imp().count.set(count);
        obj
    }

    /// The synthetic top row: `distinct` distinct values, `total` tracks.
    pub fn all_row(distinct: i64, total: i64) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().is_all.set(true);
        obj.imp().distinct.set(distinct);
        obj.imp().count.set(total);
        obj
    }

    pub fn value(&self) -> String {
        self.imp().value.borrow().clone()
    }

    pub fn count(&self) -> i64 {
        self.imp().count.get()
    }

    pub fn is_all(&self) -> bool {
        self.imp().is_all.get()
    }

    /// The text for the value column: the value, or `[All (N <plural>)]`.
    pub fn value_text(&self, plural: &str) -> String {
        if self.is_all() {
            format!("[All ({} {plural})]", self.imp().distinct.get())
        } else {
            self.value()
        }
    }
}

// --- Track row (leaf table entry) ---

mod track_imp {
    use super::*;

    #[derive(Default)]
    pub struct TrackRow {
        // The whole brief is carried so the column factories render it and the
        // sorter compares it through core's `cmp_tracks` (one source of truth).
        pub brief: RefCell<Option<TrackBrief>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "ConservatoryTrackRow";
        type Type = super::TrackRow;
    }

    impl ObjectImpl for TrackRow {}
}

glib::wrapper! {
    pub struct TrackRow(ObjectSubclass<track_imp::TrackRow>);
}

impl TrackRow {
    pub fn new(brief: &TrackBrief) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().brief.replace(Some(brief.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&TrackBrief) -> R) -> R {
        f(self.imp().brief.borrow().as_ref().expect("brief set"))
    }

    /// A clone of the underlying brief, for the `CustomSorter` comparison.
    pub fn brief(&self) -> TrackBrief {
        self.with(|b| b.clone())
    }

    pub fn title(&self) -> String {
        self.with(|b| b.title.clone())
    }

    pub fn artist(&self) -> String {
        self.with(|b| b.artist.clone().unwrap_or_default())
    }

    pub fn album(&self) -> String {
        self.with(|b| b.album.clone().unwrap_or_default())
    }

    pub fn genres(&self) -> String {
        self.with(|b| b.genres.clone())
    }

    pub fn rating(&self) -> u8 {
        self.with(|b| b.rating)
    }

    /// `m:ss`, or empty when unknown.
    pub fn duration_text(&self) -> String {
        let secs = self.with(|b| b.duration.unwrap_or(0.0));
        if secs <= 0.0 {
            return String::new();
        }
        let total = secs.round() as i64;
        format!("{}:{:02}", total / 60, total % 60)
    }
}

// --- Queue row (queue drawer entry, Phase 4b-ii-b) ---

mod queue_imp {
    use super::*;

    #[derive(Default)]
    pub struct QueueRow {
        pub row: RefCell<Option<QueueDisplayRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueRow {
        const NAME: &'static str = "ConservatoryQueueRow";
        type Type = super::QueueRow;
    }

    impl ObjectImpl for QueueRow {}
}

glib::wrapper! {
    pub struct QueueRow(ObjectSubclass<queue_imp::QueueRow>);
}

impl QueueRow {
    pub fn new(row: &QueueDisplayRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&QueueDisplayRow) -> R) -> R {
        f(self.imp().row.borrow().as_ref().expect("row set"))
    }

    /// The 0-based queue position (also the engine index, kept in sync).
    pub fn position(&self) -> i64 {
        self.with(|r| r.position)
    }

    pub fn kind(&self) -> MediaKind {
        self.with(|r| r.kind)
    }

    pub fn title(&self) -> String {
        self.with(|r| r.title.clone())
    }

    pub fn artist(&self) -> String {
        self.with(|r| r.artist.clone().unwrap_or_default())
    }
}

// --- Episode row (podcast triage list, Phase 6b-ii-a) ---

#[cfg(feature = "podcasts")]
mod episode_imp {
    use super::*;
    use conservatory_core::db::EpisodeListRow;

    #[derive(Default)]
    pub struct EpisodeRow {
        pub row: RefCell<Option<EpisodeListRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EpisodeRow {
        const NAME: &'static str = "ConservatoryEpisodeRow";
        type Type = super::EpisodeRow;
    }

    impl ObjectImpl for EpisodeRow {}
}

#[cfg(feature = "podcasts")]
glib::wrapper! {
    pub struct EpisodeRow(ObjectSubclass<episode_imp::EpisodeRow>);
}

#[cfg(feature = "podcasts")]
impl EpisodeRow {
    pub fn new(row: &conservatory_core::db::EpisodeListRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&conservatory_core::db::EpisodeListRow) -> R) -> R {
        f(self.imp().row.borrow().as_ref().expect("row set"))
    }

    pub fn id(&self) -> i64 {
        self.with(|r| r.id)
    }

    pub fn show_id(&self) -> i64 {
        self.with(|r| r.show_id)
    }

    pub fn title(&self) -> String {
        self.with(|r| r.title.clone())
    }

    pub fn show_title(&self) -> String {
        self.with(|r| r.show_title.clone())
    }

    /// Show notes (cleaned to plain text at ingest by `sanitize_notes`, 6c-iii-c).
    pub fn description(&self) -> String {
        self.with(|r| r.description.clone().unwrap_or_default())
    }

    /// `YYYY-MM-DD`, or empty when the feed gave no date.
    pub fn date_text(&self) -> String {
        self.with(|r| {
            r.pub_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        })
    }

    /// `h:mm:ss` / `m:ss`, or empty when unknown.
    pub fn duration_text(&self) -> String {
        let secs = self.with(|r| r.duration.unwrap_or(0)) as i64;
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

    /// A symbolic icon name for the played state (bundle-safe Adwaita names).
    pub fn state_icon(&self) -> &'static str {
        use conservatory_core::db::PlayedState;
        match self.with(|r| r.played) {
            PlayedState::Unplayed => "media-record-symbolic",
            PlayedState::InProgress => "media-playback-start-symbolic",
            PlayedState::PlayedFully | PlayedState::ArchivedUnlistened => "object-select-symbolic",
        }
    }

    /// A human label for the state (tooltip / accessibility).
    pub fn state_label(&self) -> &'static str {
        use conservatory_core::db::PlayedState;
        match self.with(|r| r.played) {
            PlayedState::Unplayed => "Unplayed",
            PlayedState::InProgress => "In progress",
            PlayedState::PlayedFully => "Played",
            PlayedState::ArchivedUnlistened => "Archived",
        }
    }

    pub fn played(&self) -> conservatory_core::db::PlayedState {
        self.with(|r| r.played)
    }

    pub fn starred(&self) -> bool {
        self.with(|r| r.starred)
    }

    pub fn in_queue(&self) -> bool {
        self.with(|r| r.in_queue)
    }

    /// The downloaded local file (relative to the library root), if any.
    pub fn audio_path(&self) -> Option<String> {
        self.with(|r| r.audio_path.clone())
    }

    /// The remote enclosure URL (streamed when not downloaded), if any.
    pub fn audio_url(&self) -> Option<String> {
        self.with(|r| r.audio_url.clone())
    }
}

#[cfg(all(test, feature = "podcasts"))]
mod episode_tests {
    use super::EpisodeRow;
    use conservatory_core::db::{EpisodeListRow, PlayedState};

    fn row(duration: Option<u32>, played: PlayedState) -> EpisodeListRow {
        EpisodeListRow {
            id: 1,
            show_id: 1,
            show_title: "Show".to_string(),
            title: "Episode".to_string(),
            description: None,
            pub_date: None,
            duration,
            played,
            position: 0.0,
            starred: false,
            in_queue: false,
            audio_path: None,
            audio_url: None,
        }
    }

    #[test]
    fn duration_and_state_formatting() {
        let long = EpisodeRow::new(&row(Some(3725), PlayedState::PlayedFully));
        assert_eq!(long.duration_text(), "1:02:05");
        assert_eq!(long.state_icon(), "object-select-symbolic");

        let short = EpisodeRow::new(&row(Some(95), PlayedState::Unplayed));
        assert_eq!(short.duration_text(), "1:35");
        assert_eq!(short.state_icon(), "media-record-symbolic");

        let none = EpisodeRow::new(&row(None, PlayedState::InProgress));
        assert_eq!(none.duration_text(), "");
        assert_eq!(none.state_label(), "In progress");
    }
}
