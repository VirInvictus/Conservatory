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
    use gtk::prelude::*;

    #[derive(glib::Properties, Default)]
    #[properties(wrapper_type = super::TrackRow)]
    pub struct TrackRow {
        // The whole brief is carried so the column factories render it and the
        // sorter compares it through core's `cmp_tracks` (one source of truth).
        pub brief: RefCell<Option<TrackBrief>>,
        // The play-status glyph state (Phase 11b): 0 none / 1 playing / 2 paused.
        // A glib property so the leaf glyph column can bind `notify::playing` and
        // repaint only the affected rows when playback moves, without rebinding
        // the whole (50k-row) store.
        #[property(get, set)]
        pub playing: Cell<u8>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "ConservatoryTrackRow";
        type Type = super::TrackRow;
    }

    #[glib::derived_properties]
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

// --- Book row (audiobook shelf, Phase 7b-i) ---

#[cfg(feature = "audiobooks")]
mod book_imp {
    use super::*;
    use conservatory_core::db::BookListRow;

    #[derive(Default)]
    pub struct BookRow {
        pub row: RefCell<Option<BookListRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BookRow {
        const NAME: &'static str = "ConservatoryBookRow";
        type Type = super::BookRow;
    }

    impl ObjectImpl for BookRow {}
}

#[cfg(feature = "audiobooks")]
glib::wrapper! {
    pub struct BookRow(ObjectSubclass<book_imp::BookRow>);
}

#[cfg(feature = "audiobooks")]
impl BookRow {
    pub fn new(row: &conservatory_core::db::BookListRow) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().row.replace(Some(row.clone()));
        obj
    }

    fn with<R>(&self, f: impl FnOnce(&conservatory_core::db::BookListRow) -> R) -> R {
        f(self.imp().row.borrow().as_ref().expect("row set"))
    }

    pub fn id(&self) -> i64 {
        self.with(|r| r.id)
    }

    pub fn title(&self) -> String {
        self.with(|r| r.title.clone())
    }

    pub fn subtitle(&self) -> Option<String> {
        self.with(|r| r.subtitle.clone())
    }

    /// The denormalized author credit ("Patrick Rothfuss"), or empty.
    pub fn author_display(&self) -> String {
        self.with(|r| r.author_display.clone().unwrap_or_default())
    }

    pub fn narrator_display(&self) -> String {
        self.with(|r| r.narrator_display.clone().unwrap_or_default())
    }

    /// "Series #1.5" / "Series" / empty.
    pub fn series_text(&self) -> String {
        self.with(|r| match (&r.series_name, r.series_sequence) {
            (Some(s), Some(n)) => format!("{s} #{}", trim_seq(n)),
            (Some(s), None) => s.clone(),
            _ => String::new(),
        })
    }

    pub fn year(&self) -> Option<i32> {
        self.with(|r| r.year)
    }

    /// The cover file, relative to the library root, if any.
    pub fn cover_path(&self) -> Option<String> {
        self.with(|r| r.cover_path.clone())
    }

    /// The packed median-cut accent (`0x00RRGGBB`), if a cover gave one.
    pub fn accent_rgb(&self) -> Option<u32> {
        self.with(|r| r.accent_rgb)
    }

    pub fn starred(&self) -> bool {
        self.with(|r| r.starred)
    }

    pub fn rating(&self) -> u8 {
        self.with(|r| r.rating)
    }

    /// Progress through the book as a 0.0–1.0 fraction (finished is full).
    pub fn progress_fraction(&self) -> f64 {
        self.with(|r| {
            if r.finished {
                1.0
            } else if r.total_duration > 0.0 {
                (r.position / r.total_duration).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
    }

    /// A human label for the derived state (New / In progress / Finished).
    pub fn state_label(&self) -> &'static str {
        use conservatory_core::db::BookState;
        match self.with(|r| r.state()) {
            BookState::New => "New",
            BookState::InProgress => "In progress",
            BookState::Finished => "Finished",
        }
    }

    /// A one-line meta string for the detail/grid subtitle: author, then narrator
    /// ("Read by …"), then series, then year, the present parts joined by " · ".
    pub fn meta_line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let author = self.author_display();
        if !author.is_empty() {
            parts.push(author);
        }
        let narrator = self.narrator_display();
        if !narrator.is_empty() {
            parts.push(format!("Read by {narrator}"));
        }
        let series = self.series_text();
        if !series.is_empty() {
            parts.push(series);
        }
        if let Some(y) = self.year() {
            parts.push(y.to_string());
        }
        parts.join(" · ")
    }
}

/// Render a decimal series sequence minimally: `1.0` -> `1`, `1.5` -> `1.5`.
#[cfg(feature = "audiobooks")]
fn trim_seq(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

#[cfg(all(test, feature = "audiobooks"))]
mod book_tests {
    use super::BookRow;
    use conservatory_core::db::BookListRow;

    fn row() -> BookListRow {
        BookListRow {
            id: 1,
            title: "The Name of the Wind".to_string(),
            subtitle: None,
            author_display: Some("Patrick Rothfuss".to_string()),
            narrator_display: Some("Nick Podehl".to_string()),
            series_name: Some("The Kingkiller Chronicle".to_string()),
            series_sequence: Some(1.0),
            year: Some(2009),
            cover_path: None,
            accent_rgb: Some(0x3366cc),
            rating: 5,
            starred: false,
            position: 0.0,
            finished: false,
            last_played: None,
            total_duration: 0.0,
        }
    }

    #[test]
    fn progress_state_and_meta_formatting() {
        let mut r = row();
        // No playback, no duration: New, zero progress.
        let n = BookRow::new(&r);
        assert_eq!(n.state_label(), "New");
        assert!((n.progress_fraction() - 0.0).abs() < 1e-9);
        assert_eq!(n.series_text(), "The Kingkiller Chronicle #1");
        assert_eq!(
            n.meta_line(),
            "Patrick Rothfuss · Read by Nick Podehl · The Kingkiller Chronicle #1 · 2009"
        );

        // Mid-book: in progress at 25%.
        r.position = 300.0;
        r.total_duration = 1200.0;
        let p = BookRow::new(&r);
        assert_eq!(p.state_label(), "In progress");
        assert!((p.progress_fraction() - 0.25).abs() < 1e-9);

        // Finished is full regardless of position.
        r.finished = true;
        r.position = 0.0;
        let f = BookRow::new(&r);
        assert_eq!(f.state_label(), "Finished");
        assert!((f.progress_fraction() - 1.0).abs() < 1e-9);

        // A decimal sequence keeps its fraction.
        r.series_sequence = Some(1.5);
        assert_eq!(
            BookRow::new(&r).series_text(),
            "The Kingkiller Chronicle #1.5"
        );
    }
}
