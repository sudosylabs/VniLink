//! Focused pages for this computer, a saved computer, and pairing.

use adw::prelude::*;
use gtk::Align;
use std::cell::Cell;
use std::rc::Rc;
use vnilink::{ControlRequest, DeviceInfo, SessionPhase, Snapshot};

use crate::events::LiveState;
use crate::pairing::PairingFlow;
use crate::presentation::{self, relay_description};

use super::widgets::{self, action_button, digit_entry, digits_label, flow_card, watch_digits};

pub(crate) struct PageActions {
    pub hosting: Rc<dyn Fn(bool)>,
    pub create_file: Rc<dyn Fn()>,
    pub save_computer: Rc<dyn Fn(String)>,
    pub cancel_file: Rc<dyn Fn()>,
    pub connect: Rc<dyn Fn(String)>,
    pub files: Rc<dyn Fn(String)>,
    pub allow: Rc<dyn Fn(String)>,
    pub refuse: Rc<dyn Fn()>,
}

pub(crate) struct Pages {
    detail: gtk::Box,
    title: adw::WindowTitle,
    detail_page: adw::NavigationPage,
    painting: Rc<Cell<bool>>,
}

impl Pages {
    pub(crate) fn from_builder(builder: &gtk::Builder, painting: Rc<Cell<bool>>) -> Self {
        Self {
            detail: widgets::widget(builder, "detail"),
            title: widgets::widget(builder, "title"),
            detail_page: widgets::widget(builder, "detail_page"),
            painting,
        }
    }

    pub(crate) fn detail(&self) -> &gtk::Box {
        &self.detail
    }
    pub(crate) fn title(&self) -> &adw::WindowTitle {
        &self.title
    }

    pub(crate) fn render(
        &self,
        snapshot: &Snapshot,
        flow: &PairingFlow,
        live: &LiveState,
        chosen: Option<DeviceInfo>,
        actions: &PageActions,
    ) {
        // Selection must not survive removal of the page's selectable address.
        // Moving focus first also prevents GTK retaining a disposed entry.
        let focused = self
            .detail
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        if let Some(window) = focused {
            if gtk::prelude::GtkWindowExt::focus(&window)
                .is_some_and(|focus| focus.is_ancestor(&self.detail))
            {
                gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            }
        }
        while let Some(child) = self.detail.first_child() {
            self.detail.remove(&child);
        }
        if let Some(request) = live.request() {
            self.set_titles("Access request");
            self.fill_request(request, actions);
        } else if let Some((endpoint_id, _)) = flow.file() {
            self.set_titles("Add computer");
            self.fill_file(endpoint_id, actions);
        } else if let Some(device) = chosen {
            self.set_titles(&presentation::device_name(&device));
            self.fill_peer(snapshot, &device, flow, live, actions);
        } else {
            self.set_titles("This computer");
            self.fill_computer(snapshot, flow, actions);
        }
    }

    fn set_titles(&self, title: &str) {
        self.title.set_title(title);
        self.title.set_subtitle("");
        self.detail_page.set_title(title);
    }

    fn fill_request(&self, request: &ControlRequest, actions: &PageActions) {
        self.detail.append(&widgets::introduction("Allow access?",
            "A computer is asking to control this desktop. Enter the code shown on that computer to allow it.",
        ));
        let card = flow_card();
        card.append(&widgets::center_label(&format!(
            "{} wants to connect",
            presentation::computer_name(&request.endpoint_id)
        )));
        let entry = digit_entry();
        card.append(&entry);
        card.append(&widgets::card_copy("Code from the other computer"));
        let allow = action_button("Allow control", true);
        allow.set_widget_name("allow_control");
        allow.set_sensitive(false);
        let typed = watch_digits(&entry, &allow);
        let action = Rc::clone(&actions.allow);
        allow.connect_clicked(move |_| action(typed.borrow().clone()));
        let refuse = action_button("Decline", false);
        refuse.set_widget_name("refuse_control");
        let action = Rc::clone(&actions.refuse);
        refuse.connect_clicked(move |_| action());
        card.append(&button_row(&refuse, &allow));
        self.detail.append(&card);
        self.detail
            .append(&widgets::card_copy("You can stop access at any time."));
    }

    fn fill_file(&self, endpoint_id: &str, actions: &PageActions) {
        self.detail.append(&widgets::introduction(
            "Add a computer",
            "Enter the three digits shown in VniLink on the computer that sent this pairing file.",
        ));
        let card = flow_card();
        let entry = digit_entry();
        card.append(&widgets::center_label("Code from the other computer"));
        card.append(&entry);
        let save = action_button("Add computer", true);
        save.set_widget_name("save_computer");
        save.set_sensitive(false);
        let typed = watch_digits(&entry, &save);
        let action = Rc::clone(&actions.save_computer);
        save.connect_clicked(move |_| action(typed.borrow().clone()));
        let cancel = action_button("Cancel", false);
        cancel.set_widget_name("cancel_file");
        let action = Rc::clone(&actions.cancel_file);
        cancel.connect_clicked(move |_| action());
        card.append(&button_row(&cancel, &save));
        self.detail.append(&card);
        self.detail.append(&widgets::card_copy(
            "Adding a computer does not give it access to this desktop.",
        ));
        self.detail
            .append(&widgets::address_group(endpoint_id, None));
    }

    fn fill_computer(&self, snapshot: &Snapshot, flow: &PairingFlow, actions: &PageActions) {
        let active = snapshot
            .incoming
            .as_ref()
            .is_some_and(|session| session.phase == SessionPhase::Active);
        if active {
            self.detail.append(&widgets::introduction(
                "Desktop connected",
                "A paired computer is connected to this desktop. Use Stop to end the session.",
            ));
        } else if let Some(check) = flow.offer_check() {
            self.detail.append(&widgets::introduction(
                "Ready to pair",
                "Open the saved pairing file on your other computer, then enter this code.",
            ));
            let card = flow_card();
            card.append(&widgets::center_label("Pairing code"));
            card.append(&digits_label(check));
            card.append(&widgets::card_copy(
                "The file expires 15 minutes after it was created.",
            ));
            self.detail.append(&card);
            let again = action_button("Create another file", false);
            again.set_widget_name("create_file");
            again.set_halign(Align::Start);
            let action = Rc::clone(&actions.create_file);
            again.connect_clicked(move |_| action());
            self.detail.append(&again);
        } else {
            let introduction = widgets::introduction(
                "This computer",
                "Let your other computers connect to this desktop.",
            );
            introduction.append(&widgets::status_label(
                if snapshot.hosting {
                    "Connections allowed"
                } else {
                    "Connections paused"
                },
                snapshot.hosting,
            ));
            self.detail.append(&introduction);
        }

        let access = adw::PreferencesGroup::new();
        access.set_title("Remote access");
        let switch = adw::SwitchRow::builder()
            .title("Allow connections")
            .subtitle(if snapshot.hosting {
                "Only computers you approve can connect."
            } else {
                "Turn on to receive connections from your computers."
            })
            .use_markup(false)
            .active(snapshot.hosting)
            .build();
        switch.set_widget_name("hosting_switch");
        let guard = Rc::clone(&self.painting);
        let action = Rc::clone(&actions.hosting);
        switch.connect_active_notify(move |row| {
            if !guard.get() {
                action(row.is_active());
            }
        });
        access.add(&switch);
        if flow.offer_check().is_none() && !active {
            let row = adw::ActionRow::builder()
                .title("Pair another computer")
                .subtitle("Create a pairing file.")
                .subtitle_lines(2)
                .use_markup(false)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("document-send-symbolic"));
            let create = gtk::Button::with_label("Create file");
            create.set_valign(Align::Center);
            create.add_css_class("suggested-action");
            create.set_widget_name("create_file");
            let action = Rc::clone(&actions.create_file);
            create.connect_clicked(move |_| action());
            row.add_suffix(&create);
            row.set_activatable_widget(Some(&create));
            access.add(&row);
        }
        self.detail.append(&access);
        if !snapshot.capabilities.desktop_capture {
            self.detail.append(&notice(
                "Choose a screen to share",
                "Open Settings and choose a screen before connecting to this desktop.",
            ));
        }
        if snapshot.capabilities.desktop_capture && !snapshot.capabilities.h264_encode {
            self.detail.append(&notice(
                "Screen sharing unavailable",
                "No supported video encoder is available on this computer.",
            ));
        }
        let summary = presentation::relay_summary(snapshot.public_relay, &snapshot.relay_urls);
        self.detail.append(&widgets::address_group(
            &snapshot.endpoint_id,
            Some((summary.as_str(), relay_description(snapshot.public_relay))),
        ));
    }

    fn fill_peer(
        &self,
        snapshot: &Snapshot,
        device: &DeviceInfo,
        flow: &PairingFlow,
        live: &LiveState,
        actions: &PageActions,
    ) {
        let session = snapshot
            .outgoing
            .as_ref()
            .filter(|session| session.peer_id == device.endpoint_id);
        if let Some(check) = flow.waiting().filter(|_| session.is_some()) {
            self.detail.append(&widgets::introduction(
                "Waiting for approval",
                "Enter this code in VniLink on the other computer to allow the connection.",
            ));
            let card = flow_card();
            card.append(&digits_label(check));
            let spinner = gtk::Spinner::builder()
                .spinning(true)
                .halign(Align::Center)
                .build();
            card.append(&spinner);
            card.append(&widgets::card_copy(
                "Keep this window open while you approve access.",
            ));
            self.detail.append(&card);
            return;
        }
        let active = session.is_some_and(|session| session.phase == SessionPhase::Active);
        let title = presentation::device_name(&device);
        self.detail.append(&widgets::introduction(&title,
            if active {
                "The remote desktop is open in a tab."
            } else {
                "Connect when this computer is on and VniLink is open. It may ask you to approve access."
            },
        ));
        if !active {
            let connect = action_button(
                if live.connecting() {
                    "Connecting…"
                } else {
                    "Connect"
                },
                true,
            );
            connect.set_widget_name("connect");
            connect.set_halign(Align::Start);
            connect.set_sensitive(!live.connecting() && snapshot.capabilities.h264_decode);
            let endpoint = device.endpoint_id.clone();
            let action = Rc::clone(&actions.connect);
            connect.connect_clicked(move |_| action(endpoint.clone()));
            self.detail.append(&connect);
        }
        // Files works without opening the desktop viewer and without host
        // screen sharing: it establishes (or reuses) the control session for
        // file roots/listings/transfers only. No video decoder is required.
        {
            let files = action_button("Files…", false);
            files.set_widget_name("open-files");
            files.set_halign(Align::Start);
            files.set_tooltip_text(Some("Browse both computers and copy files either way"));
            let endpoint = device.endpoint_id.clone();
            let action = Rc::clone(&actions.files);
            files.connect_clicked(move |_| action(endpoint.clone()));
            self.detail.append(&files);
        }
        if !snapshot.capabilities.h264_decode {
            self.detail.append(&notice(
                "Desktop viewing unavailable",
                "No supported video decoder is available on this computer.",
            ));
        }
        let permissions = adw::PreferencesGroup::new();
        permissions.set_title("Access to your desktop");
        let row = adw::ActionRow::builder()
            .title("Desktop control")
            .subtitle(if device.grants.control {
                "This computer can control your desktop"
            } else {
                "This computer needs your approval to control your desktop"
            })
            .use_markup(false)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("input-keyboard-symbolic"));
        permissions.add(&row);
        self.detail.append(&permissions);
        self.detail
            .append(&widgets::address_group(&device.endpoint_id, None));
    }
}

fn button_row(secondary: &gtk::Button, primary: &gtk::Button) -> gtk::FlowBox {
    let row = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(1)
        .max_children_per_line(2)
        .column_spacing(12)
        .row_spacing(8)
        .halign(Align::Center)
        .margin_top(8)
        .build();
    row.insert(secondary, -1);
    row.insert(primary, -1);
    row
}

fn notice(title: &str, description: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::new();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(description)
        .subtitle_lines(3)
        .use_markup(false)
        .build();
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.add_css_class("warning");
    row.add_prefix(&icon);
    group.add(&row);
    group
}
