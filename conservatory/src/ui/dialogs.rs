//! Owned modal alert (the adw::AlertDialog replacement, Phase 26). The API
//! mirrors the adw surface (heading/body, extra child, named responses with
//! appearance, default and close responses) so call sites convert mechanically.
//! Stock `gtk::AlertDialog` was evaluated and rejected: no extra child, no
//! per-response styling, and index-addressed buttons (spec §2.4).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk4 as gtk;

#[derive(Clone, Copy, PartialEq)]
pub enum Appearance {
    Suggested,
    Destructive,
}

fn appearance_class(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Suggested => "suggested-action",
        Appearance::Destructive => "destructive-action",
    }
}

type ResponseHandler = Box<dyn Fn(&str)>;

struct State {
    responses: RefCell<Vec<(String, gtk::Button)>>,
    handler: RefCell<Option<ResponseHandler>>,
    default_response: RefCell<Option<String>>,
    close_response: RefCell<String>,
    /// Exactly-once dispatch: a button click emits its id and closes; the
    /// window's close path (Escape, the close button, `close()`) emits the
    /// close response only if nothing was emitted yet.
    responded: Cell<bool>,
}

impl State {
    fn emit(&self, id: &str) {
        if self.responded.replace(true) {
            return;
        }
        if let Some(handler) = self.handler.borrow().as_ref() {
            handler(id);
        }
    }
}

pub struct Alert {
    win: gtk::Window,
    extra_slot: gtk::Box,
    button_row: gtk::Box,
    state: Rc<State>,
}

impl Alert {
    pub fn new(heading: Option<&str>, body: Option<&str>) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .build();
        if let Some(heading) = heading.filter(|h| !h.is_empty()) {
            let label = gtk::Label::builder()
                .label(heading)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes(["heading"])
                .build();
            content.append(&label);
        }
        if let Some(body) = body.filter(|b| !b.is_empty()) {
            let label = gtk::Label::builder()
                .label(body)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .build();
            content.append(&label);
        }
        let extra_slot = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&extra_slot);
        let button_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(8)
            .build();
        content.append(&button_row);

        let win = gtk::Window::builder()
            .title(heading.unwrap_or_default())
            .modal(true)
            .resizable(false)
            .default_width(360)
            .child(&content)
            .build();
        super::close_on_escape(&win);

        let state = Rc::new(State {
            responses: RefCell::new(Vec::new()),
            handler: RefCell::new(None),
            default_response: RefCell::new(None),
            close_response: RefCell::new("close".to_string()),
            responded: Cell::new(false),
        });
        // Any close path that skipped the buttons (Escape, the WM close button)
        // still answers, with the close response.
        let close_state = state.clone();
        win.connect_close_request(move |_| {
            let id = close_state.close_response.borrow().clone();
            close_state.emit(&id);
            gtk::glib::Propagation::Proceed
        });

        Self {
            win,
            extra_slot,
            button_row,
            state,
        }
    }

    pub fn set_extra_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        while let Some(old) = self.extra_slot.first_child() {
            self.extra_slot.remove(&old);
        }
        if let Some(child) = child {
            self.extra_slot.append(child);
        }
    }

    pub fn add_response(&self, id: &str, label: &str) {
        let button = gtk::Button::with_label(label);
        let state = self.state.clone();
        let weak = self.win.downgrade();
        let response = id.to_string();
        button.connect_clicked(move |_| {
            state.emit(&response);
            if let Some(win) = weak.upgrade() {
                win.close();
            }
        });
        self.button_row.append(&button);
        self.state
            .responses
            .borrow_mut()
            .push((id.to_string(), button));
    }

    pub fn set_response_appearance(&self, id: &str, appearance: Appearance) {
        if let Some((_, button)) = self
            .state
            .responses
            .borrow()
            .iter()
            .find(|(rid, _)| rid == id)
        {
            button.add_css_class(appearance_class(appearance));
        }
    }

    /// The response activated by Enter (via the window default widget; entries
    /// opt in with `set_activates_default(true)`).
    pub fn set_default_response(&self, id: Option<&str>) {
        *self.state.default_response.borrow_mut() = id.map(str::to_string);
    }

    /// The response emitted when the dialog is dismissed without a button
    /// (Escape, the WM close). Defaults to `"close"`, matching adw.
    pub fn set_close_response(&self, id: &str) {
        *self.state.close_response.borrow_mut() = id.to_string();
    }

    pub fn connect_response(&self, handler: impl Fn(&str) + 'static) {
        *self.state.handler.borrow_mut() = Some(Box::new(handler));
    }

    /// Present, transient for the widget's root window (anchors may be buttons
    /// inside another dialog; the transient parent must be that window, not the
    /// main one).
    pub fn present(&self, parent: Option<&impl IsA<gtk::Widget>>) {
        if let Some(parent) = parent {
            let root = parent
                .as_ref()
                .root()
                .and_then(|r| r.downcast::<gtk::Window>().ok());
            self.win.set_transient_for(root.as_ref());
        }
        if let Some(id) = self.state.default_response.borrow().as_deref()
            && let Some((_, button)) = self
                .state
                .responses
                .borrow()
                .iter()
                .find(|(rid, _)| rid == id)
        {
            self.win.set_default_widget(Some(button));
        }
        self.win.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_maps_to_the_owned_classes() {
        assert_eq!(appearance_class(Appearance::Suggested), "suggested-action");
        assert_eq!(
            appearance_class(Appearance::Destructive),
            "destructive-action"
        );
    }
}
