//! Small widget builders. No domain logic lives here; each function takes
//! plain strings and returns a widget.

use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib::object::IsA;
use gtk::Align;

use crate::devices::DeviceRow;

pub(crate) fn widget<T: IsA<gtk::glib::Object>>(builder: &gtk::Builder, name: &str) -> T {
    builder
        .object(name)
        .unwrap_or_else(|| panic!("window.ui has no {name}"))
}

pub(crate) fn peer_id(row: Option<&gtk::ListBoxRow>) -> Option<String> {
    row.and_then(|row| row.widget_name().strip_prefix("peer-").map(str::to_string))
}

pub(crate) fn self_row(row: &DeviceRow) -> adw::ActionRow {
    let widget = adw::ActionRow::builder()
        .title(&row.title)
        .subtitle(&row.subtitle)
        .activatable(true)
        .build();
    widget.set_widget_name(&row.name);
    widget.set_hexpand(true);
    widget.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
    widget
}

pub(crate) fn peer_row(row: &DeviceRow) -> adw::ActionRow {
    let widget = adw::ActionRow::builder()
        .title(&row.title)
        .subtitle(&row.subtitle)
        .title_lines(2)
        .subtitle_lines(2)
        .activatable(true)
        .build();
    widget.set_widget_name(&row.name);
    widget.set_hexpand(true);
    widget.add_css_class("peer");
    widget.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
    widget
}

pub(crate) fn heading(title: &str, sentence: &str) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 8);
    block.set_hexpand(true);
    let name = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .build();
    name.add_css_class("title-1");
    block.append(&name);
    block.append(&body(sentence));
    block
}

pub(crate) fn body(sentence: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(sentence)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .max_width_chars(52)
        .build();
    label
}

pub(crate) fn center_label(sentence: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(sentence)
        .xalign(0.5)
        .justify(gtk::Justification::Center)
        .halign(Align::Center)
        .wrap(true)
        .hexpand(true)
        .max_width_chars(36)
        .build();
    label
}

pub(crate) fn card_copy(sentence: &str) -> gtk::Label {
    let label = center_label(sentence);
    label.add_css_class("dim-label");
    label
}

pub(crate) fn flow_card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.set_hexpand(true);
    card.add_css_class("card");
    card.add_css_class("flow-card");
    card
}

pub(crate) fn digits_label(value: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(value)
        .halign(Align::Center)
        .hexpand(true)
        .build();
    label.add_css_class("digits");
    label.set_widget_name("check_digits");
    label
}

pub(crate) fn pill(label: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::builder().label(label).build();
    button.add_css_class("pill");
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

pub(crate) fn digit_entry() -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .placeholder_text("Three digits")
        .max_length(3)
        .input_purpose(gtk::InputPurpose::Digits)
        .halign(Align::Start)
        .width_chars(8)
        .build();
    entry.set_widget_name("digits_entry");
    entry
}

pub(crate) fn watch_digits(entry: &gtk::Entry, gate: &gtk::Button) -> Rc<RefCell<String>> {
    let typed = Rc::new(RefCell::new(String::new()));
    let remember = Rc::clone(&typed);
    let gate = gate.clone();
    entry.connect_changed(move |entry| {
        let text = entry.text().to_string();
        gate.set_sensitive(text.len() == 3);
        *remember.borrow_mut() = text;
    });
    typed
}

pub(crate) fn address_group(endpoint: &str, relay: Option<(&str, &str)>) -> adw::PreferencesGroup {
    let identity = adw::ExpanderRow::builder()
        .title("Address")
        .subtitle(crate::presentation::short_id(endpoint))
        .use_markup(false)
        .build();
    identity.set_widget_name("endpoint");
    identity.add_css_class("identity");
    let value = gtk::ListBoxRow::new();
    value.set_activatable(false);
    value.set_selectable(false);
    let label = endpoint_label(endpoint);
    label.set_margin_top(6);
    label.set_margin_bottom(12);
    label.set_margin_start(12);
    label.set_margin_end(12);
    value.set_child(Some(&label));
    identity.add_row(&value);
    if let Some((title, subtitle)) = relay {
        let relay_row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .title_lines(2)
            .subtitle_lines(4)
            .use_markup(false)
            .activatable(false)
            .selectable(false)
            .build();
        identity.add_row(&relay_row);
    }
    let group = adw::PreferencesGroup::new();
    group.set_hexpand(true);
    group.add(&identity);
    group
}

/// GTK 4.14 can keep a clipboard provider aimed at a label after the page
/// rebuild destroys it. Drop selection before that happens.
fn endpoint_label(endpoint: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(endpoint)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .selectable(true)
        .hexpand(true)
        .width_chars(16)
        .max_width_chars(48)
        .build();
    label.add_css_class("endpoint");
    label.add_css_class("dim-label");
    label.connect_destroy(|label| label.set_selectable(false));
    label
}

pub(crate) fn click(root: &impl IsA<gtk::Widget>, name: &str) {
    named::<gtk::Button>(root, name).emit_clicked();
}

pub(crate) fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
    fn walk(widget: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
        if widget.widget_name() == name {
            return Some(widget.clone());
        }
        let mut child = widget.first_child();
        while let Some(widget) = child {
            if let Some(found) = walk(&widget, name) {
                return Some(found);
            }
            child = widget.next_sibling();
        }
        None
    }
    walk(root.as_ref(), name)
        .unwrap_or_else(|| panic!("no widget {name}"))
        .downcast()
        .unwrap_or_else(|_| panic!("{name} has the wrong type"))
}
