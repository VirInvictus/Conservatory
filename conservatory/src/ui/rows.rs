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
