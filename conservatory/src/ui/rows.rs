//! Shared plain-GTK list rows (the adw::ActionRow replacements, Phase 26): a
//! title with an optional dim subtitle on the left, an optional suffix widget
//! trailing on the right. Ported from the Colophon pilot (ATTRIBUTIONS.md).

use gtk::pango;
use gtk::prelude::*;
use gtk4 as gtk;

/// A non-activatable list row. Long titles and subtitles ellipsize; the
/// subtitle carries itself as a tooltip so nothing is lost to the cut.
pub fn row(title: &str, subtitle: Option<&str>, suffix: Option<&gtk::Widget>) -> gtk::ListBoxRow {
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
            .tooltip_text(subtitle)
            .css_classes(["caption", "dim-label"])
            .build();
        text.append(&subtitle_label);
    }
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
    gtk::ListBoxRow::builder()
        .activatable(false)
        .child(&content)
        .build()
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
pub fn combo_row(title: &str, items: &[&str]) -> (gtk::ListBoxRow, gtk::DropDown) {
    let dropdown = gtk::DropDown::from_strings(items);
    let row = row(title, None, Some(dropdown.upcast_ref()));
    (row, dropdown)
}

/// An adw::PreferencesGroup successor: an optional heading and dim description
/// over a `.boxed-list` of rows.
pub struct Group {
    root: gtk::Box,
    list: gtk::ListBox,
}

pub fn group(title: Option<&str>, description: Option<&str>) -> Group {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        let label = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        root.append(&label);
    }
    if let Some(description) = description.filter(|d| !d.is_empty()) {
        let label = gtk::Label::builder()
            .label(description)
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .build();
        root.append(&label);
    }
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    root.append(&list);
    Group { root, list }
}

impl Group {
    /// The widget to place (a dialog extra child, a preferences page section).
    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
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
