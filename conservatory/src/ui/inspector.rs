//! The track properties inspector (Phase 11a): a right-docked collapsible
//! `gtk::Revealer` (the queue-drawer twin) showing the selected track's large
//! accent-tinted cover over a read-only properties grid (the deadbeef
//! `selproperties` + `coverart` widgets). Selection-driven, distinct from the
//! playback-driven Now Playing drawer (11c).
//!
//! The pure row projection (`inspector_fields`) lives in `ui/fields.rs`; the
//! window resolves the rows from the DB and hands them to `show`, so this module
//! builds no DB reads.

use std::path::Path;

use gtk::prelude::*;
use gtk4 as gtk;

use crate::ui::accent::AccentProvider;

/// The inspector: the revealer the window docks, plus the cover + grid it fills.
pub struct Inspector {
    pub revealer: gtk::Revealer,
    title: gtk::Label,
    cover: gtk::Image,
    cover_frame: gtk::Frame,
    grid: gtk::Grid,
    /// The shared accent helper (Phase 12a) carrying the current cover's ring.
    accent: AccentProvider,
}

/// Build the inspector (revealed off; the window appends `revealer` to the right
/// of the content box and toggles it).
pub fn build_inspector() -> Inspector {
    let title = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-4"])
        .label("Track properties")
        .build();
    // The grid looks editable but is display-only; say so up front rather than
    // let a click land on nothing (16.5b).
    let readonly = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .label("Read-only \u{2022} edit with Ctrl+E")
        .build();

    let cover = gtk::Image::builder()
        .pixel_size(210)
        .icon_name("audio-x-generic-symbolic")
        .build();
    let cover_frame = gtk::Frame::builder()
        .halign(gtk::Align::Center)
        .css_classes(["inspector-cover"])
        .child(&cover)
        .build();

    let grid = gtk::Grid::builder()
        .row_spacing(8)
        .column_spacing(16)
        .margin_top(8)
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&title);
    body.append(&readonly);
    body.append(&cover_frame);
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    body.append(&grid);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .width_request(250)
        .child(&body)
        .build();

    // Closed by default: an empty inspector (no track selected) is dead space on
    // launch. The browse fills the full width instead; `Ctrl+P` slides it in, and
    // closing it returns the space (Phase 12b had it open, but the empty panel read
    // as a wasted gap).
    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .transition_duration(250)
        .reveal_child(false)
        .child(&scroller)
        .build();

    Inspector {
        revealer,
        title,
        cover,
        cover_frame,
        grid,
        accent: AccentProvider::new(),
    }
}

impl Inspector {
    pub fn is_open(&self) -> bool {
        self.revealer.reveals_child()
    }

    pub fn set_open(&self, open: bool) {
        self.revealer.set_reveal_child(open);
    }

    /// Populate the inspector for a selected track: the title heads it, `fields`
    /// fill the grid, `cover_abs` (when present and on disk) loads the large
    /// cover, and `accent` tints its frame.
    pub fn show(
        &self,
        title: &str,
        fields: &[(String, String)],
        cover_abs: Option<&Path>,
        accent: Option<u32>,
    ) {
        self.title.set_text(title);
        self.fill_grid(fields);
        match cover_abs.filter(|p| p.exists()) {
            Some(p) => self.cover.set_from_file(Some(p)),
            None => self.cover.set_icon_name(Some("audio-x-generic-symbolic")),
        }
        self.apply_accent(accent);
    }

    /// The empty state (no track selected, or selection cleared).
    pub fn clear(&self) {
        self.title.set_text("No track selected");
        self.fill_grid(&[]);
        self.cover.set_icon_name(Some("audio-x-generic-symbolic"));
        self.apply_accent(None);
    }

    fn fill_grid(&self, fields: &[(String, String)]) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (row, (label, value)) in fields.iter().enumerate() {
            let key = gtk::Label::builder()
                .label(label)
                .xalign(0.0)
                .yalign(0.0)
                .css_classes(["dim-label", "caption"])
                .build();
            let val = gtk::Label::builder()
                .label(value)
                .xalign(0.0)
                .yalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .hexpand(true)
                .build();
            if crate::ui::fields::is_tech_field(label) {
                val.add_css_class("tech");
            }
            self.grid.attach(&key, 0, row as i32, 1, 1);
            self.grid.attach(&val, 1, row as i32, 1, 1);
        }
    }

    /// Tint the cover frame with the album accent ring (Phase 12a routes this
    /// through the shared `AccentProvider`, the non-deprecated per-item colour
    /// technique, swapping the rule on each call).
    fn apply_accent(&self, accent: Option<u32>) {
        self.accent
            .apply_cover_ring(&self.cover_frame, &["inspector-cover"], accent);
    }
}
