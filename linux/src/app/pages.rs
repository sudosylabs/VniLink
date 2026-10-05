//! The detail pages. Owns what the right side shows, not the sidebar.

use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use gtk::Align;
use vnilink::{ControlRequest, DeviceInfo, Snapshot};

use crate::capabilities;
use crate::events::LiveState;
use crate::pairing::PairingFlow;
use crate::presentation::{
    self, hosting_subtitle, peer_sentence, relay_description, short_id,
};

use super::widgets::{self, digit_entry, digits_label, flow_card, pill, watch_digits};

/// Actions the pages call. Owned by the window; see `super::Actions`.
pub(crate) struct PageActions {
    pub hosting: Rc<dyn Fn(bool)>,
    pub create_file: Rc<dyn Fn()>,
    pub save_computer: Rc<dyn Fn(String)>,
    pub cancel_file: Rc<dyn Fn()>,
    pub connect: Rc<dyn Fn(String)>,
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

    /// Renders the right side for the snapshot and flow state.
    pub(crate) fn render(
        &self,
        snapshot: &Snapshot,
        flow: &PairingFlow,
        live: &LiveState,
        chosen: Option<DeviceInfo>,
        actions: &PageActions,
    ) {
        while let Some(child) = self.detail.first_child() {
            self.detail.remove(&child);
        }
        if let Some(request) = live.request() {
            self.set_titles("This computer");
            self.fill_request(request, actions);
            return;
        }
        if let Some((endpoint_id, check)) = flow.file() {
            self.set_titles("Pairing file");
            self.fill_file(endpoint_id, check, actions);
            return;
        }
        if let Some(device) = chosen {
            let short = short_id(&device.endpoint_id);
            self.set_titles(&short);
            self.fill_peer(snapshot, &device, flow, live, actions);
            return;
        }
        self.set_titles("This computer");
        self.fill_computer(snapshot, flow, actions);
    }

    fn set_titles(&self, title: &str) {
        self.title.set_title(title);
        self.title.set_subtitle("");
        self.detail_page.set_title(title);
    }

    fn fill_request(&self, request: &ControlRequest, actions: &PageActions) {
        self.detail.append(&widgets::heading(
            "Allow control",
            "Type the three digits shown on the computer that is asking. The number below is the same check, so you can see that it matches.",
        ));
        let card = flow_card();
        let who = widgets::center_label(&format!(
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
        let allow_action = Rc::clone(&actions.allow);
        allow.connect_clicked(move |_| allow_action(typed.borrow().clone()));
        let refuse_action = Rc::clone(&actions.refuse);
        refuse.connect_clicked(move |_| refuse_action());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(Align::Center);
        row.append(&allow);
        row.append(&refuse);
        self.detail.append(&entry);
        self.detail.append(&row);
    }

    fn fill_file(&self, endpoint_id: &str, check: &str, actions: &PageActions) {
        self.detail.append(&widgets::heading(
            "Check the file",
            "Type the three digits shown on the computer that created this file. The number below is what this file says.",
        ));
        let card = flow_card();
        card.append(&digits_label(check));
        card.append(&widgets::card_copy(&format!(
            "From {}.",
            short_id(endpoint_id)
        )));
        self.detail.append(&card);
        let entry = digit_entry();
        entry.set_halign(Align::Center);
        let save = pill("Save this computer", true);
        save.set_widget_name("save_computer");
        save.set_sensitive(false);
        let cancel = pill("Cancel", false);
        let typed = watch_digits(&entry, &save);
        let save_action = Rc::clone(&actions.save_computer);
        save.connect_clicked(move |_| save_action(typed.borrow().clone()));
        let cancel_action = Rc::clone(&actions.cancel_file);
        cancel.connect_clicked(move |_| cancel_action());
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.set_halign(Align::Center);
        row.append(&save);
        row.append(&cancel);
        self.detail.append(&entry);
        self.detail.append(&row);
    }

    fn fill_computer(&self, snapshot: &Snapshot, flow: &PairingFlow, actions: &PageActions) {
        let status = capabilities::video_status(&snapshot.capabilities);
        if let Some(sentence) = capabilities::host_encoder_sentence(status) {
            // The host admits no video session in this state; name it here so
            // the owner knows before anyone connects.
            let card = flow_card();
            card.append(&widgets::card_copy(sentence));
            self.detail.append(&card);
        }
        if let Some(check) = flow.offer_check() {
            let card = flow_card();
            card.append(&digits_label(check));
            let sentence = if snapshot.hosting {
                "Give the file to the other computer. They type these digits. The file expires in 15 minutes."
            } else {
                "Allow control is off, so this file will not admit anyone. Turn it on, or create a new file."
            };
            card.append(&widgets::card_copy(sentence));
            self.detail.append(&card);
            let again = pill("Create another file", false);
            again.set_widget_name("create_file");
            again.set_halign(Align::Start);
            let create_action = Rc::clone(&actions.create_file);
            again.connect_clicked(move |_| create_action());
            self.detail.append(&again);
        } else {
            let create = pill("Create a pairing file", true);
            create.set_widget_name("create_file");
            create.set_halign(Align::Start);
            let create_action = Rc::clone(&actions.create_file);
            create.connect_clicked(move |_| create_action());
            let block = widgets::heading(
                "Let another computer control this one",
                "Create a pairing file and give it to them. This screen shows three digits. They open the file and type those digits.",
            );
            block.append(&create);
            self.detail.append(&block);
        }

        if snapshot.hosting || flow.offer_check().is_some() {
            let hosting = adw::PreferencesGroup::new();
            hosting.set_hexpand(true);
            let switch = adw::SwitchRow::builder()
                .title("Allow control")
                .subtitle(hosting_subtitle(
                    snapshot.hosting,
                    flow.offer_check().is_some(),
                ))
                .active(snapshot.hosting)
                .build();
            let guard = Rc::clone(&self.painting);
            let on_hosting = Rc::clone(&actions.hosting);
            switch.connect_active_notify(move |row| {
                if !guard.get() {
                    on_hosting(row.is_active());
                }
            });
            hosting.add(&switch);
            self.detail.append(&hosting);
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
        let waiting = flow.waiting().filter(|_| {
            snapshot
                .outgoing
                .as_ref()
                .is_some_and(|session| session.peer_id == device.endpoint_id)
        });
        if let Some(check) = waiting {
            let card = flow_card();
            card.append(&digits_label(check));
            card.append(&widgets::card_copy(
                "Read these digits on this computer. The other computer types them to allow control.",
            ));
            self.detail.append(&card);
            return;
        }
        let connect = pill(
            if live.connecting() {
                "Connecting…"
            } else {
                "Connect"
            },
            true,
        );
        connect.set_widget_name("connect");
        connect.set_halign(Align::Start);
        connect.set_sensitive(!live.connecting());
        let endpoint_id = device.endpoint_id.clone();
        let connect_action = Rc::clone(&actions.connect);
        connect.connect_clicked(move |_| connect_action(endpoint_id.clone()));
        let block = widgets::heading("Connect", peer_sentence(device.grants));
        block.append(&connect);
        self.detail.append(&block);
        if !snapshot.capabilities.h264_decode {
            let card = flow_card();
            card.append(&widgets::card_copy(
                "This computer cannot decode video: no H.264 decoder.",
            ));
            self.detail.append(&card);
        }
        self.detail
            .append(&widgets::address_group(&device.endpoint_id, None));
    }
}
