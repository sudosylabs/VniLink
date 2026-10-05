//! The window people keep open.
//!
//! This computer creates a pairing file and shows three digits. The sidebar
//! is the other direction: open that file, type the digits, then connect.
//! Connect shows a second set of digits for the first computer to type.
//! A wide window shows both sides. A narrow window shows one at a time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::object::IsA;
use gtk::Align;
use vnilink::{ControlRequest, DeviceInfo, Snapshot};

use crate::presentation::{
    hosting_name, hosting_subtitle, peer_sentence, peer_status, relay_description, relay_summary,
    session_banner, short_id,
};

pub(crate) struct Actions {
    pub hosting: Rc<dyn Fn(bool)>,
    pub stop: Rc<dyn Fn()>,
    pub create_file: Rc<dyn Fn()>,
    pub open_file: Rc<dyn Fn()>,
    pub save_computer: Rc<dyn Fn(String)>,
    pub cancel_file: Rc<dyn Fn()>,
    pub connect: Rc<dyn Fn(String)>,
    pub allow: Rc<dyn Fn(String)>,
    pub refuse: Rc<dyn Fn()>,
}

pub struct Home {
    /// Owns the builder window so dropping the builder does not destroy the pages.
    #[allow(dead_code)]
    window: adw::ApplicationWindow,
    pub root: adw::NavigationSplitView,
    sidebar_stack: gtk::Stack,
    rows: gtk::ListBox,
    title: adw::WindowTitle,
    detail_page: adw::NavigationPage,
    detail: gtk::Box,
    banner: adw::Banner,
    empty_action: gtk::Button,
    selected: Rc<RefCell<Option<String>>>,
    painting: Rc<Cell<bool>>,
    snapshot: Rc<RefCell<Option<Snapshot>>>,
    offer: RefCell<Option<String>>,
    request: RefCell<Option<ControlRequest>>,
    file: RefCell<Option<(String, String)>>,
    waiting: RefCell<Option<String>>,
    connecting: Cell<bool>,
    paint_key: RefCell<Option<String>>,
    actions: Actions,
}

impl Home {
    pub(crate) fn from_builder(builder: &gtk::Builder, actions: Actions) -> Rc<Self> {
        let home = Rc::new(Self {
            window: widget(builder, "window"),
            root: widget(builder, "split"),
            sidebar_stack: widget(builder, "sidebar_stack"),
            rows: widget(builder, "rows"),
            title: widget(builder, "title"),
            detail_page: widget(builder, "detail_page"),
            detail: widget(builder, "detail"),
            banner: widget(builder, "banner"),
            empty_action: widget(builder, "show_computer"),
            selected: Rc::new(RefCell::new(None)),
            painting: Rc::new(Cell::new(false)),
            snapshot: Rc::new(RefCell::new(None)),
            offer: RefCell::new(None),
            request: RefCell::new(None),
            file: RefCell::new(None),
            waiting: RefCell::new(None),
            connecting: Cell::new(false),
            paint_key: RefCell::new(None),
            actions,
        });
        home.banner.set_use_markup(false);

        let open_file: gtk::Button = widget(builder, "open_file");
        let open_header: gtk::Button = widget(builder, "open_file_header");
        let open = Rc::clone(&home.actions.open_file);
        open_file.connect_clicked(move |_| open());
        let open = Rc::clone(&home.actions.open_file);
        open_header.connect_clicked(move |_| open());
        let weak = Rc::downgrade(&home);
        home.rows.connect_row_selected(move |_, row| {
            let Some(home) = weak.upgrade() else {
                return;
            };
            if home.painting.get() {
                return;
            }
            home.file.borrow_mut().take();
            *home.selected.borrow_mut() = peer_id(row);
            home.render_stored();
        });
        let weak = Rc::downgrade(&home);
        home.rows.connect_row_activated(move |_, _| {
            if let Some(home) = weak.upgrade() {
                home.root.set_show_content(true);
            }
        });
        let weak = Rc::downgrade(&home);
        home.empty_action.connect_clicked(move |_| {
            if let Some(home) = weak.upgrade() {
                *home.selected.borrow_mut() = None;
                home.render_stored();
                home.root.set_show_content(true);
            }
        });
        let weak = Rc::downgrade(&home);
        home.root.connect_collapsed_notify(move |_| {
            if let Some(home) = weak.upgrade() {
                home.update_empty_action();
            }
        });
        let weak = Rc::downgrade(&home);
        home.banner.connect_button_clicked(move |_| {
            if let Some(home) = weak.upgrade() {
                (home.actions.stop)();
            }
        });
        home.rows.set_header_func(|row, before| {
            let paired = row.widget_name().starts_with("peer-")
                && before.is_none_or(|previous| previous.widget_name() == "self");
            if !paired {
                row.set_header(None::<&gtk::Widget>);
                return;
            }
            let label = gtk::Label::builder()
                .label("Paired")
                .xalign(0.0)
                .margin_top(18)
                .margin_bottom(6)
                .margin_start(14)
                .margin_end(14)
                .build();
            label.add_css_class("caption-heading");
            row.set_header(Some(&label));
        });
        home
    }

    pub fn show_offer(&self, check: &str) {
        self.offer.replace(Some(check.to_string()));
        self.refresh();
    }

    pub fn show_file(&self, endpoint_id: &str, check: &str) {
        self.file
            .replace(Some((endpoint_id.to_string(), check.to_string())));
        self.refresh();
    }

    pub fn clear_file(&self) {
        self.file.borrow_mut().take();
        self.refresh();
    }

    pub fn set_request(&self, request: Option<ControlRequest>) {
        self.request.replace(request);
    }

    pub fn set_waiting(&self, digits: Option<String>) {
        self.waiting.replace(digits);
    }

    pub fn set_connecting(&self, connecting: bool) {
        self.connecting.set(connecting);
        self.refresh();
    }

    pub fn select(&self, endpoint_id: Option<String>) {
        *self.selected.borrow_mut() = endpoint_id;
        self.paint_key.borrow_mut().take();
    }

    pub fn show(&self, snapshot: &Snapshot) {
        self.snapshot.replace(Some(snapshot.clone()));
        if self.request.borrow().is_some() {
            self.selected.borrow_mut().take();
        }
        if self.selected.borrow().as_ref().is_some_and(|id| {
            !snapshot
                .devices
                .iter()
                .any(|device| &device.endpoint_id == id)
        }) {
            self.selected.borrow_mut().take();
        }
        let key = self.key(snapshot);
        if self.paint_key.borrow().as_ref() == Some(&key) {
            return;
        }
        if self.request.borrow().is_some() {
            self.root.set_show_content(true);
        }
        self.painting.set(true);
        self.sidebar_stack
            .set_visible_child_name(if snapshot.devices.is_empty() {
                "empty"
            } else {
                "list"
            });
        self.rebuild_rows(snapshot);
        self.update_empty_action();
        self.render_detail(snapshot);
        self.paint_key.replace(Some(key));
        self.painting.set(false);
    }

    fn refresh(&self) {
        self.paint_key.borrow_mut().take();
        let snapshot = self.snapshot.borrow().as_ref().cloned();
        if let Some(snapshot) = snapshot {
            self.show(&snapshot);
        }
    }

    fn key(&self, snapshot: &Snapshot) -> String {
        let devices = snapshot
            .devices
            .iter()
            .map(|device| {
                format!(
                    "{}:{}{}{}",
                    device.endpoint_id,
                    u8::from(device.grants.control),
                    u8::from(device.grants.clipboard),
                    u8::from(device.grants.transfer)
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let session = |info: &Option<vnilink::SessionInfo>| {
            info.as_ref()
                .map(|info| {
                    format!(
                        "{}:{}:{}",
                        info.peer_id,
                        crate::presentation::role_name(info.role),
                        crate::presentation::phase_name(&info.phase)
                    )
                })
                .unwrap_or_default()
        };
        format!(
            "{}\n{:?}\n{:?}\n{:?}\n{}\n{}\n{}\n{}\n{}\n{}\n{}{}{}",
            self.selected.borrow().clone().unwrap_or_default(),
            self.offer.borrow().clone(),
            self.request.borrow().as_ref().map(|ask| ask.check.clone()),
            self.file.borrow().clone(),
            self.waiting.borrow().clone().unwrap_or_default(),
            self.connecting.get(),
            snapshot.hosting,
            devices,
            session(&snapshot.incoming),
            session(&snapshot.outgoing),
            u8::from(snapshot.capabilities.desktop_capture),
            u8::from(snapshot.capabilities.h264_encode),
            u8::from(snapshot.capabilities.h264_decode),
        )
    }

    fn update_empty_action(&self) {
        let empty = self.sidebar_stack.visible_child_name().as_deref() == Some("empty");
        self.empty_action
            .set_visible(empty && self.root.is_collapsed());
    }

    fn rebuild_rows(&self, snapshot: &Snapshot) {
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        if snapshot.devices.is_empty() {
            return;
        }
        let mut matched = None;
        let wanted = self.selected.borrow().clone();
        let rows = std::iter::once(self_row(snapshot.hosting)).chain(
            snapshot
                .devices
                .iter()
                .map(|device| peer_row(&device.endpoint_id, peer_status(device.grants))),
        );
        for row in rows {
            let name = row.widget_name().to_string();
            let is_match = match &wanted {
                None => name == "self",
                Some(id) => name == format!("peer-{id}"),
            };
            self.rows.append(&row);
            if is_match {
                matched = Some(row);
            }
        }
        if let Some(row) = &matched {
            self.rows.select_row(Some(row));
        }
    }

    fn render_stored(&self) {
        let snapshot = self.snapshot.borrow().as_ref().cloned();
        if let Some(snapshot) = snapshot {
            self.painting.set(true);
            self.render_detail(&snapshot);
            self.paint_key.replace(Some(self.key(&snapshot)));
            self.painting.set(false);
        }
    }

    fn render_detail(&self, snapshot: &Snapshot) {
        while let Some(child) = self.detail.first_child() {
            self.detail.remove(&child);
        }
        if let Some(session) = snapshot.incoming.as_ref().or(snapshot.outgoing.as_ref()) {
            self.banner.set_title(&session_banner(session));
            self.banner.set_revealed(true);
        } else {
            self.banner.set_revealed(false);
        }
        if let Some(request) = self.request.borrow().clone() {
            self.title.set_title("This computer");
            self.title.set_subtitle("");
            self.detail_page.set_title("This computer");
            self.fill_request(&request);
            return;
        }
        if let Some((endpoint_id, check)) = self.file.borrow().clone() {
            self.title.set_title("Pairing file");
            self.title.set_subtitle("");
            self.detail_page.set_title("Pairing file");
            self.fill_file(&endpoint_id, &check);
            return;
        }
        let chosen = self.selected.borrow().clone().and_then(|id| {
            snapshot
                .devices
                .iter()
                .find(|device| device.endpoint_id == id)
                .cloned()
        });
        if let Some(device) = chosen {
            let short = short_id(&device.endpoint_id);
            self.title.set_title(&short);
            self.title.set_subtitle("");
            self.detail_page.set_title(&short);
            self.fill_peer(snapshot, &device);
            return;
        }
        self.title.set_title("This computer");
        self.title.set_subtitle("");
        self.detail_page.set_title("This computer");
        self.fill_computer(snapshot);
    }

    fn fill_request(&self, request: &ControlRequest) {
        self.detail.append(&heading(
            "Allow control",
            "Type the three digits shown on the computer that is asking. The number below is the same check, so you can see that it matches.",
        ));
        let card = flow_card();
        let who = center_label(&format!(
            "{} wants to control this desktop",
            short_id(&request.endpoint_id)
        ));
        who.add_css_class("title-4");
        card.append(&who);
        card.append(&digits_label(&request.check));
        self.detail.append(&card);
        let entry = digit_entry();
        entry.set_halign(Align::Center);
        let allow = pill("Allow", true);
        allow.set_widget_name("allow_control");
        allow.set_sensitive(false);
        let refuse = pill("Not now", false);
        let typed = watch_digits(&entry, &allow);
        let allow_action = Rc::clone(&self.actions.allow);
        allow.connect_clicked(move |_| allow_action(typed.borrow().clone()));
        let refuse_action = Rc::clone(&self.actions.refuse);
        refuse.connect_clicked(move |_| refuse_action());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(Align::Center);
        row.append(&allow);
        row.append(&refuse);
        self.detail.append(&entry);
        self.detail.append(&row);
    }

    fn fill_file(&self, endpoint_id: &str, check: &str) {
        self.detail.append(&heading(
            "Check the file",
            "Type the three digits shown on the computer that created this file. The number below is what this file says.",
        ));
        let card = flow_card();
        card.append(&digits_label(check));
        card.append(&card_copy(&format!("From {}.", short_id(endpoint_id))));
        self.detail.append(&card);
        let entry = digit_entry();
        entry.set_halign(Align::Center);
        let save = pill("Save this computer", true);
        save.set_widget_name("save_computer");
        save.set_sensitive(false);
        let cancel = pill("Cancel", false);
        let typed = watch_digits(&entry, &save);
        let save_action = Rc::clone(&self.actions.save_computer);
        save.connect_clicked(move |_| save_action(typed.borrow().clone()));
        let cancel_action = Rc::clone(&self.actions.cancel_file);
        cancel.connect_clicked(move |_| cancel_action());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(Align::Center);
        row.append(&save);
        row.append(&cancel);
        self.detail.append(&entry);
        self.detail.append(&row);
    }

    fn fill_computer(&self, snapshot: &Snapshot) {
        if snapshot.capabilities.desktop_capture && !snapshot.capabilities.h264_encode {
            // The host admits no video session in this state; name it here so
            // the owner knows before anyone connects.
            let card = flow_card();
            card.append(&card_copy(
                "Video unavailable: no H.264 encoder on this computer.",
            ));
            self.detail.append(&card);
        }
        let offer = self.offer.borrow().clone();
        if let Some(check) = offer.as_deref() {
            let card = flow_card();
            card.append(&digits_label(check));
            let sentence = if snapshot.hosting {
                "Give the file to the other computer. They type these digits. The file expires in 15 minutes."
            } else {
                "Allow control is off, so this file will not admit anyone. Turn it on, or create a new file."
            };
            card.append(&card_copy(sentence));
            self.detail.append(&card);
            let again = pill("Create another file", false);
            again.set_widget_name("create_file");
            again.set_halign(Align::Start);
            let create_action = Rc::clone(&self.actions.create_file);
            again.connect_clicked(move |_| create_action());
            self.detail.append(&again);
        } else {
            let create = pill("Create a pairing file", true);
            create.set_widget_name("create_file");
            create.set_halign(Align::Start);
            let create_action = Rc::clone(&self.actions.create_file);
            create.connect_clicked(move |_| create_action());
            let block = heading(
                "Let another computer control this one",
                "Create a pairing file and give it to them. This screen shows three digits. They open the file and type those digits.",
            );
            block.append(&create);
            self.detail.append(&block);
        }

        if snapshot.hosting || offer.is_some() {
            let hosting = adw::PreferencesGroup::new();
            hosting.set_hexpand(true);
            let switch = adw::SwitchRow::builder()
                .title("Allow control")
                .subtitle(hosting_subtitle(snapshot.hosting, offer.is_some()))
                .active(snapshot.hosting)
                .build();
            let guard = self.painting.clone();
            let on_hosting = Rc::clone(&self.actions.hosting);
            switch.connect_active_notify(move |row| {
                if !guard.get() {
                    on_hosting(row.is_active());
                }
            });
            hosting.add(&switch);
            self.detail.append(&hosting);
        }
        let summary = relay_summary(snapshot.public_relay, &snapshot.relay_urls);
        self.detail.append(&address_group(
            &snapshot.endpoint_id,
            Some((summary.as_str(), relay_description(snapshot.public_relay))),
        ));
    }

    fn fill_peer(&self, snapshot: &Snapshot, device: &DeviceInfo) {
        let waiting = self.waiting.borrow().clone().filter(|_| {
            snapshot
                .outgoing
                .as_ref()
                .is_some_and(|session| session.peer_id == device.endpoint_id)
        });
        if let Some(check) = waiting {
            let card = flow_card();
            card.append(&digits_label(&check));
            card.append(&card_copy(
                "Read these digits on this computer. The other computer types them to allow control.",
            ));
            self.detail.append(&card);
            return;
        }
        let connect = pill(
            if self.connecting.get() {
                "Connecting…"
            } else {
                "Connect"
            },
            true,
        );
        connect.set_widget_name("connect");
        connect.set_halign(Align::Start);
        connect.set_sensitive(!self.connecting.get());
        let endpoint_id = device.endpoint_id.clone();
        let connect_action = Rc::clone(&self.actions.connect);
        connect.connect_clicked(move |_| connect_action(endpoint_id.clone()));
        let block = heading("Connect", peer_sentence(device.grants));
        block.append(&connect);
        self.detail.append(&block);
        if !snapshot.capabilities.h264_decode {
            let card = flow_card();
            card.append(&card_copy(
                "This computer cannot decode video: no H.264 decoder.",
            ));
            self.detail.append(&card);
        }
        self.detail
            .append(&address_group(&device.endpoint_id, None));
    }
}

pub(crate) fn widget<T: IsA<gtk::glib::Object>>(builder: &gtk::Builder, name: &str) -> T {
    builder
        .object(name)
        .unwrap_or_else(|| panic!("window.ui has no {name}"))
}

fn peer_id(row: Option<&gtk::ListBoxRow>) -> Option<String> {
    row.and_then(|row| row.widget_name().strip_prefix("peer-").map(str::to_string))
}

fn self_row(hosting: bool) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("This computer")
        .subtitle(hosting_name(hosting))
        .activatable(true)
        .build();
    row.set_widget_name("self");
    row.set_hexpand(true);
    row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
    row
}

fn peer_row(endpoint_id: &str, summary: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(short_id(endpoint_id))
        .subtitle(summary)
        .title_lines(2)
        .subtitle_lines(2)
        .activatable(true)
        .build();
    row.set_widget_name(&format!("peer-{endpoint_id}"));
    row.set_hexpand(true);
    row.add_css_class("peer");
    row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
    row
}

fn heading(title: &str, sentence: &str) -> gtk::Box {
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

fn body(sentence: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(sentence)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .max_width_chars(52)
        .build();
    label
}

fn center_label(sentence: &str) -> gtk::Label {
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

fn card_copy(sentence: &str) -> gtk::Label {
    let label = center_label(sentence);
    label.add_css_class("dim-label");
    label
}

fn flow_card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.set_hexpand(true);
    card.add_css_class("card");
    card.add_css_class("flow-card");
    card
}

fn digits_label(value: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(value)
        .halign(Align::Center)
        .hexpand(true)
        .build();
    label.add_css_class("digits");
    label.set_widget_name("check_digits");
    label
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::builder().label(label).build();
    button.add_css_class("pill");
    if suggested {
        button.add_css_class("suggested-action");
    }
    button
}

fn digit_entry() -> gtk::Entry {
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

fn watch_digits(entry: &gtk::Entry, gate: &gtk::Button) -> Rc<RefCell<String>> {
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

fn address_group(endpoint: &str, relay: Option<(&str, &str)>) -> adw::PreferencesGroup {
    let identity = adw::ExpanderRow::builder()
        .title("Address")
        .subtitle(short_id(endpoint))
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

#[cfg(test)]
mod tests {
    use super::*;
    use vnilink::{Capabilities, Grants, SessionInfo, SessionPhase, SessionRole};

    fn open_home(actions: Actions) -> Rc<Home> {
        use std::sync::Once;
        static STARTED: Once = Once::new();
        STARTED.call_once(|| {
            gtk::init().expect("gtk");
        });
        let builder = gtk::Builder::from_string(include_str!("window.ui"));
        let root: gtk::Stack = super::widget(&builder, "root");
        assert!(root.child_by_name("startup").is_some());
        assert!(root.child_by_name("home").is_some());
        assert!(root.child_by_name("failure").is_some());
        Home::from_builder(&builder, actions)
    }

    fn quiet() -> Actions {
        Actions {
            hosting: Rc::new(|_| {}),
            stop: Rc::new(|| {}),
            create_file: Rc::new(|| {}),
            open_file: Rc::new(|| {}),
            save_computer: Rc::new(|_| {}),
            cancel_file: Rc::new(|| {}),
            connect: Rc::new(|_| {}),
            allow: Rc::new(|_| {}),
            refuse: Rc::new(|| {}),
        }
    }

    fn snapshot(devices: Vec<DeviceInfo>, hosting: bool) -> Snapshot {
        Snapshot {
            endpoint_id: "this-computer".to_string(),
            public_relay: true,
            relay_urls: Vec::new(),
            hosting,
            devices,
            incoming: None,
            outgoing: None,
            capabilities: Capabilities::UNSUPPORTED,
        }
    }

    fn peer(grants: Grants) -> DeviceInfo {
        DeviceInfo {
            endpoint_id: "other-computer".to_string(),
            relays: Vec::new(),
            direct: Vec::new(),
            grants,
        }
    }

    fn labels(widget: &impl IsA<gtk::Widget>) -> Vec<String> {
        let mut found = Vec::new();
        fn walk(widget: &gtk::Widget, found: &mut Vec<String>) {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                found.push(label.label().to_string());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                walk(&widget, found);
                child = widget.next_sibling();
            }
        }
        walk(widget.as_ref(), &mut found);
        found
    }

    /// GTK has to run on the process main thread. The test harness puts other
    /// tests on worker threads, so this one repeats itself in a fresh process.
    #[test]
    fn the_window_matches_the_computer_and_the_paired_list() {
        if std::env::var_os("VNILINK_GTK_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "home::tests::the_window_matches_the_computer_and_the_paired_list",
                    "--test-threads=1",
                ])
                .env("VNILINK_GTK_CHILD", "1")
                .env("GTK_IM_MODULE", "gtk-im-context-simple")
                .status()
                .expect("respawn gtk test");
            assert!(status.success());
            return;
        }
        an_unpaired_computer_shows_the_empty_state_and_this_computer();
        choosing_a_paired_computer_offers_connect();
        a_pairing_file_asks_for_the_digits_on_the_other_screen();
        a_control_request_asks_for_the_digits_on_the_other_screen();
        an_active_session_is_a_banner_with_stop();
        a_removed_computer_returns_to_this_computer();
    }

    fn an_unpaired_computer_shows_the_empty_state_and_this_computer() {
        let calls = Rc::new(Cell::new(0));
        let created = Rc::new(Cell::new(0));
        let mut actions = quiet();
        actions.hosting = Rc::new({
            let calls = Rc::clone(&calls);
            move |_| calls.set(calls.get() + 1)
        });
        actions.create_file = Rc::new({
            let created = Rc::clone(&created);
            move || created.set(created.get() + 1)
        });
        let home = open_home(actions);
        home.show(&snapshot(Vec::new(), true));
        assert_eq!(
            home.sidebar_stack.visible_child_name().as_deref(),
            Some("empty")
        );
        assert_eq!(home.title.title().as_str(), "This computer");
        assert!(!home.banner.is_revealed());
        assert!(!home.empty_action.is_visible());
        assert_eq!(calls.get(), 0);
        let text = labels(&home.window);
        assert!(text.iter().any(|label| label == "Open a pairing file"));
        assert!(text.iter().any(|label| label == "Reach another computer"));
        assert!(!text
            .iter()
            .any(|label| label.contains("Pairing starts here")));
        let page = labels(&home.detail);
        assert!(page.iter().any(|label| label == "this-computer"));
        assert!(page.iter().any(|label| label == "Allow control"));
        assert!(page
            .iter()
            .any(|label| label == "Let another computer control this one"));
        assert!(page.iter().any(|label| label == "Create a pairing file"));
        click(&home.detail, "create_file");
        assert_eq!(created.get(), 1);
        home.show_offer("042");
        let page = labels(&home.detail);
        assert!(page.iter().any(|label| label == "042"));
    }

    fn choosing_a_paired_computer_offers_connect() {
        let connected = Rc::new(RefCell::new(None));
        let mut actions = quiet();
        actions.connect = Rc::new({
            let connected = Rc::clone(&connected);
            move |id| *connected.borrow_mut() = Some(id)
        });
        let home = open_home(actions);
        let grants = Grants {
            control: true,
            clipboard: false,
            transfer: false,
        };
        home.show(&snapshot(vec![peer(grants)], false));
        assert_eq!(
            home.sidebar_stack.visible_child_name().as_deref(),
            Some("list")
        );
        let row = home.rows.row_at_index(1).expect("peer row");
        assert_eq!(row.widget_name().as_str(), "peer-other-computer");
        home.rows.select_row(Some(&row));
        assert_eq!(home.title.title().as_str(), "other-computer");
        let text = labels(&home.detail);
        assert!(text.iter().any(|label| label == "Connect"));
        assert!(text.iter().any(|label| {
            label == "Control is allowed. Connect when that computer is on. The clipboard and files stay off."
        }));
        click(&home.detail, "connect");
        assert_eq!(connected.borrow().as_deref(), Some("other-computer"));
    }

    fn a_pairing_file_asks_for_the_digits_on_the_other_screen() {
        let saved = Rc::new(RefCell::new(None));
        let mut actions = quiet();
        actions.save_computer = Rc::new({
            let saved = Rc::clone(&saved);
            move |digits| *saved.borrow_mut() = Some(digits)
        });
        let home = open_home(actions);
        home.show(&snapshot(Vec::new(), false));
        home.show_file("other-computer", "042");
        assert_eq!(home.title.title().as_str(), "Pairing file");
        let text = labels(&home.detail);
        assert!(text.iter().any(|label| label == "042"));
        assert!(text.iter().any(|label| label == "Check the file"));
        let entry = named::<gtk::Entry>(&home.detail, "digits_entry");
        entry.set_text("042");
        click(&home.detail, "save_computer");
        assert_eq!(saved.borrow().as_deref(), Some("042"));
    }

    fn a_control_request_asks_for_the_digits_on_the_other_screen() {
        let allowed = Rc::new(RefCell::new(None));
        let mut actions = quiet();
        actions.allow = Rc::new({
            let allowed = Rc::clone(&allowed);
            move |digits| *allowed.borrow_mut() = Some(digits)
        });
        let home = open_home(actions);
        home.show(&snapshot(vec![peer(Grants::default())], true));
        home.rows.select_row(home.rows.row_at_index(1).as_ref());
        home.set_request(Some(ControlRequest {
            endpoint_id: "other-computer".to_string(),
            check: "847".to_string(),
        }));
        home.show(&snapshot(vec![peer(Grants::default())], true));
        assert_eq!(home.title.title().as_str(), "This computer");
        let text = labels(&home.detail);
        assert!(text.iter().any(|label| label == "847"));
        assert!(text
            .iter()
            .any(|label| label.contains("wants to control this desktop")));
        let entry = named::<gtk::Entry>(&home.detail, "digits_entry");
        entry.set_text("847");
        click(&home.detail, "allow_control");
        assert_eq!(allowed.borrow().as_deref(), Some("847"));
    }

    fn an_active_session_is_a_banner_with_stop() {
        let home = open_home(quiet());
        let mut picture = snapshot(Vec::new(), true);
        picture.incoming = Some(SessionInfo {
            id: 7,
            generation: 1,
            role: SessionRole::Host,
            phase: SessionPhase::Active,
            peer_id: "abcdefghijklmnopqrstuvwxyz012345".to_string(),
            quality: None,
            monitor_width: 0,
            monitor_height: 0,
        });
        home.show(&picture);
        assert!(home.banner.is_revealed());
        assert_eq!(
            home.banner.title().as_str(),
            "abcdefgh…012345 is connected to this desktop"
        );
        assert_eq!(home.banner.button_label().as_deref(), Some("Stop"));
    }

    fn click(root: &impl IsA<gtk::Widget>, name: &str) {
        named::<gtk::Button>(root, name).emit_clicked();
    }

    fn named<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>, name: &str) -> T {
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

    fn a_removed_computer_returns_to_this_computer() {
        let home = open_home(quiet());
        home.show(&snapshot(vec![peer(Grants::default())], false));
        home.rows.select_row(home.rows.row_at_index(1).as_ref());
        home.show(&snapshot(Vec::new(), false));
        assert_eq!(home.title.title().as_str(), "This computer");
        assert_eq!(
            home.sidebar_stack.visible_child_name().as_deref(),
            Some("empty")
        );
    }
}
