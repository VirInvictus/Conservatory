//! Shared plain-GTK list rows. The row builders and `Group` moved into
//! vir-gtk's widget kit (1.4.0); this module re-exports them under the
//! historical path and keeps Conservatory's own `Expander`.

#[allow(unused_imports)]
pub use vir_gtk::widgets::{
    Group, action_row, combo_row, entry_row, group, row, spin_row, switch_row,
};

use gtk::pango;
use gtk::prelude::*;
use gtk4 as gtk;

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
