//! Shared plain-GTK list rows (the adw::ActionRow replacements, Phase 26): a
//! title with an optional dim subtitle on the left, an optional suffix widget
//! trailing on the right. Ported from the Colophon pilot (ATTRIBUTIONS.md).

use gtk::pango;
use gtk::prelude::*;
use gtk4 as gtk;

/// The shared row body: title over an optional dim subtitle, an optional
/// trailing suffix. Returns the subtitle label so `action_row` can hand it out
/// for later mutation; it starts hidden when the subtitle is absent or empty.
fn build_row(
    title: &str,
    subtitle: Option<&str>,
    suffix: Option<&gtk::Widget>,
) -> (gtk::ListBoxRow, gtk::Label) {
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    text.append(&title_label);
    let subtitle_label = gtk::Label::builder()
        .label(subtitle.unwrap_or_default())
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();
    subtitle_label.set_tooltip_text(subtitle);
    subtitle_label.set_visible(subtitle.is_some_and(|s| !s.is_empty()));
    text.append(&subtitle_label);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&text);
    if let Some(suffix) = suffix {
        suffix.set_valign(gtk::Align::Center);
        content.append(suffix);
    }
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .child(&content)
        .build();
    (row, subtitle_label)
}

/// A non-activatable list row. Long titles and subtitles ellipsize; the
/// subtitle carries itself as a tooltip so nothing is lost to the cut.
pub fn row(title: &str, subtitle: Option<&str>, suffix: Option<&gtk::Widget>) -> gtk::ListBoxRow {
    build_row(title, subtitle, suffix).0
}

/// An adw::ActionRow successor for rows whose subtitle changes at runtime:
/// the returned label is the subtitle (kept visible so updates always show).
pub fn action_row(
    title: &str,
    subtitle: Option<&str>,
    suffix: Option<&gtk::Widget>,
) -> (gtk::ListBoxRow, gtk::Label) {
    let (row, label) = build_row(title, subtitle, suffix);
    label.set_visible(true);
    (row, label)
}

/// An adw::SwitchRow successor: the returned `gtk::Switch` keeps the exact
/// `set_active` / `is_active` / `connect_active_notify` surface call sites
/// already wire against.
pub fn switch_row(title: &str, subtitle: Option<&str>) -> (gtk::ListBoxRow, gtk::Switch) {
    let switch = gtk::Switch::new();
    let row = row(title, subtitle, Some(switch.upcast_ref()));
    (row, switch)
}

/// An adw::SpinRow successor; the returned `gtk::SpinButton` carries the
/// `set_digits` / `set_value` / `value` / `adjustment` surface.
pub fn spin_row(
    title: &str,
    subtitle: Option<&str>,
    min: f64,
    max: f64,
    step: f64,
) -> (gtk::ListBoxRow, gtk::SpinButton) {
    let spin = gtk::SpinButton::with_range(min, max, step);
    let row = row(title, subtitle, Some(spin.upcast_ref()));
    (row, spin)
}

/// An adw::ComboRow successor; the returned `gtk::DropDown` carries the
/// `set_selected` / `selected` / `connect_selected_notify` surface.
pub fn combo_row(
    title: &str,
    subtitle: Option<&str>,
    items: &[&str],
) -> (gtk::ListBoxRow, gtk::DropDown) {
    let dropdown = gtk::DropDown::from_strings(items);
    let row = row(title, subtitle, Some(dropdown.upcast_ref()));
    (row, dropdown)
}

/// An adw::EntryRow successor; the returned `gtk::Entry` carries the
/// `set_text` / `text` / `connect_changed` surface.
pub fn entry_row(title: &str, text: &str) -> (gtk::ListBoxRow, gtk::Entry) {
    let entry = gtk::Entry::builder().text(text).hexpand(true).build();
    let row = row(title, None, Some(entry.upcast_ref()));
    (row, entry)
}

/// An adw::ExpanderRow (with enable switch) successor: a header row whose
/// switch both gates the feature and reveals the nested settings rows, the
/// way `show_enable_switch` behaved. Callers read `switch.is_active()` where
/// they read `enables_expansion()`.
pub struct Expander {
    pub row: gtk::ListBoxRow,
    pub switch: gtk::Switch,
    body: gtk::ListBox,
}

pub fn expander(title: &str, subtitle: Option<&str>) -> Expander {
    let switch = gtk::Switch::new();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title_label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    text.append(&title_label);
    if let Some(subtitle) = subtitle.filter(|s| !s.is_empty()) {
        let subtitle_label = gtk::Label::builder()
            .label(subtitle)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .css_classes(["caption", "dim-label"])
            .build();
        text.append(&subtitle_label);
    }
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(12)
        .margin_end(12)
        .build();
    switch.set_valign(gtk::Align::Center);
    header.append(&text);
    header.append(&switch);

    let body = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .margin_start(12)
        .margin_bottom(6)
        .build();
    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .child(&body)
        .build();
    {
        let revealer = revealer.clone();
        switch.connect_active_notify(move |s| revealer.set_reveal_child(s.is_active()));
    }
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&header);
    column.append(&revealer);
    let row = gtk::ListBoxRow::builder()
        .activatable(false)
        .child(&column)
        .build();
    Expander { row, switch, body }
}

impl Expander {
    pub fn add_row(&self, row: &gtk::ListBoxRow) {
        self.body.append(row);
    }
}

/// An adw::PreferencesGroup successor: an optional heading and dim description
/// (with room for a trailing header suffix) over a `.boxed-list` of rows.
pub struct Group {
    root: gtk::Box,
    header: gtk::Box,
    list: gtk::ListBox,
}

pub fn group(title: Option<&str>, description: Option<&str>) -> Group {
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        let label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        text.append(&label);
    }
    if let Some(description) = description.filter(|d| !d.is_empty()) {
        let label = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();
        text.append(&label);
    }
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .visible(title.is_some() || description.is_some())
        .build();
    header.append(&text);
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    root.append(&header);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    root.append(&list);
    Group { root, header, list }
}

impl Group {
    /// The widget to place (a dialog extra child, a preferences page section).
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    /// A trailing widget in the header line (the presets group's buttons).
    pub fn set_header_suffix(&self, suffix: &impl IsA<gtk::Widget>) {
        suffix.as_ref().set_valign(gtk::Align::Center);
        self.header.append(suffix);
        self.header.set_visible(true);
    }

    /// Append a row; any non-row widget is wrapped in a non-activatable row,
    /// the way adw::PreferencesGroup::add did.
    pub fn add(&self, child: &impl IsA<gtk::Widget>) {
        if let Some(row) = child.as_ref().downcast_ref::<gtk::ListBoxRow>() {
            self.list.append(row);
        } else {
            let wrapper = gtk::ListBoxRow::builder()
                .activatable(false)
                .child(child)
                .build();
            self.list.append(&wrapper);
        }
    }
}
