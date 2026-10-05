//! Native settings backed by core operations, including saved relay credentials.
use super::{tasks, widgets};
use adw::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use vnilink::{Core, Grants, KeyboardMode, Snapshot};

pub(crate) fn open(parent: &adw::ApplicationWindow, core: Arc<Core>) {
    let window = shell(parent);
    let loading = gtk::Spinner::builder().spinning(true).build();
    let page = page("Loading", "content-loading-symbolic");
    let group = adw::PreferencesGroup::new();
    group.add(&loading);
    page.add(&group);
    let pages = widgets::named::<gtk::Stack>(&window, "settings_pages");
    pages.add_named(&page, Some("loading"));
    window.present();
    let worker = core.clone();
    tasks::run(
        move || {
            Ok((
                worker.snapshot().map_err(|e| e.to_string())?,
                worker.relay_configuration().map_err(|e| e.to_string())?,
            ))
        },
        move |result| {
            pages.remove(&page);
            match result {
                Ok((snapshot, saved)) => populate(&window, core.clone(), &snapshot, saved),
                Err(error) => report(&window, &error),
            }
        },
    );
}

pub(crate) fn populate(
    window: &adw::Window,
    core: Arc<Core>,
    snapshot: &Snapshot,
    saved: vnilink::RelaySettings,
) {
    let pages = widgets::named::<gtk::Stack>(window, "settings_pages");
    let sections = widgets::named::<gtk::ListBox>(window, "settings_sections");
    for page in [
        connection_page(window, core.clone(), snapshot, saved),
        super::background::page(window),
        keyboard_page(window, core.clone()),
        computers_page(window, core.clone(), snapshot),
        super::files::page(window, core.clone()),
        pairing_page(window, core),
    ] {
        let name = page.name().unwrap().to_string();
        pages.add_named(&page, Some(&name));
        let row = adw::ActionRow::builder()
            .title(page.title())
            .activatable(true)
            .build();
        row.set_widget_name(&name);
        let image = gtk::Image::from_icon_name(&page.icon_name().unwrap());
        row.add_prefix(&image);
        sections.append(&row);
    }
    sections.select_row(sections.row_at_index(0).as_ref());
    pages.set_visible_child_name("connection");
}

fn page(title: &str, icon: &str) -> adw::PreferencesPage {
    adw::PreferencesPage::builder()
        .title(title)
        .name(title.to_lowercase())
        .icon_name(icon)
        .build()
}

fn connection_page(
    window: &adw::Window,
    core: Arc<Core>,
    snapshot: &Snapshot,
    saved: vnilink::RelaySettings,
) -> adw::PreferencesPage {
    let page = page("Connection", "network-server-symbolic");
    let group = adw::PreferencesGroup::builder().title("Relays")
        .description("Choose the relays this computer will use. Changes take effect after restarting VniLink.").build();
    let mode = adw::ComboRow::builder()
        .title("Relay network")
        .model(&gtk::StringList::new(&["Public relays", "Custom relays"]))
        .selected(if saved.is_public() { 0 } else { 1 })
        .build();
    mode.set_widget_name("relay_mode");
    group.add(&mode);
    page.add(&group);
    let custom = adw::PreferencesGroup::builder().title("Custom relays")
        .description("Use HTTPS URLs. Authentication tokens are saved in the session keyring. Leave the new-token field empty to keep a saved token, or turn authentication off to remove it.").build();
    let rows = Rc::new(std::cell::RefCell::new(Vec::new()));
    for index in 0..8 {
        let url = adw::EntryRow::builder()
            .title(format!("Relay {} URL", index + 1))
            .build();
        url.set_widget_name(&format!("relay_url_{index}"));
        let previous = match &saved {
            vnilink::RelaySettings::Custom(relays) => relays.get(index),
            _ => None,
        };
        if let Some(relay) = previous {
            url.set_text(relay.url());
        }
        let authentication = adw::SwitchRow::builder()
            .title("Relay authentication")
            .active(previous.is_some_and(|r| r.has_auth_token()))
            .build();
        authentication.set_widget_name(&format!("relay_auth_{index}"));
        let token = adw::PasswordEntryRow::builder()
            .title("New authentication token")
            .build();
        token.set_widget_name(&format!("relay_token_{index}"));
        custom.add(&url);
        custom.add(&authentication);
        custom.add(&token);
        // Each pair can be revealed with Add relay, rather than a wall of fields.
        let shown = index == 0 || previous.is_some();
        url.set_visible(shown);
        authentication.set_visible(shown);
        token.set_visible(shown && authentication.is_active());
        let token_watch = token.clone();
        let url_watch = url.clone();
        authentication.connect_active_notify(move |switch| {
            token_watch.set_visible(url_watch.get_visible() && switch.is_active())
        });
        rows.borrow_mut().push((url, token, authentication));
    }
    let add = gtk::Button::with_label("Add relay");
    add.set_halign(gtk::Align::Start);
    custom.add(&action_group(&[&add]));

    let fields = rows.clone();
    add.connect_clicked(move |button| {
        if let Some((url, token, authentication)) = fields
            .borrow()
            .iter()
            .find(|(url, _, _)| !url.get_visible())
        {
            url.set_visible(true);
            authentication.set_visible(true);
            token.set_visible(authentication.is_active());
            url.grab_focus();
        }
        button.set_sensitive(fields.borrow().iter().any(|(url, _, _)| !url.get_visible()));
    });
    custom.set_visible(!saved.is_public());
    let custom_watch = custom.clone();
    mode.connect_selected_notify(move |mode| custom_watch.set_visible(mode.selected() == 1));
    page.add(&custom);
    let save_group = adw::PreferencesGroup::new();
    let save = gtk::Button::with_label("Save relay settings");
    save.add_css_class("suggested-action");
    save.set_halign(gtk::Align::Start);
    save.set_widget_name("save_relays");
    save_group.add(&action_group(&[&save]));
    page.add(&save_group);
    let win = window.clone();
    let save_core = core.clone();
    let saved = Rc::new(std::cell::RefCell::new(saved));
    save.connect_clicked(move |button| {
        let relay = if mode.selected() == 0 {
            Ok(vnilink::RelaySettings::public_n0())
        } else {
            crate::settings::relay_configuration(
                &rows
                    .borrow()
                    .iter()
                    .map(|(url, token, authentication)| {
                        (
                            url.text().to_string(),
                            token.text().to_string(),
                            authentication.is_active(),
                        )
                    })
                    .collect::<Vec<_>>(),
                &saved.borrow(),
            )
        };
        let relay = match relay {
            Ok(relay) => relay,
            Err(error) => {
                report(&win, &error.to_string());
                return;
            }
        };
        button.set_sensitive(false);
        let button = button.clone();
        let core = save_core.clone();
        let saved = saved.clone();
        let stored = relay.clone();
        let win = win.clone();
        tasks::run(
            move || core.set_relay(relay).map_err(|e| e.to_string()),
            move |result| {
                button.set_sensitive(true);
                match result {
                    Ok(()) => {
                        saved.replace(stored);
                        toast(
                            &win,
                            adw::Toast::new("Saved. Restart VniLink to use these relays."),
                        );
                    }
                    Err(error) => report(&win, &error),
                }
            },
        );
    });
    let screen = adw::PreferencesGroup::builder().title("Screen sharing")
        .description("Enable desktop sharing and control for this launch. On Wayland, approve a screen, keyboard and pointer in the desktop permission dialog. This starts local capture; paired computers still need permission to connect.").build();
    let choose = gtk::Button::with_label("Enable desktop sharing and control");
    choose.set_halign(gtk::Align::Start);
    screen.add(&action_group(&[&choose]));
    let win = window.clone();
    let worker = core.clone();
    choose.connect_clicked(move |button| super::capture::choose(&win, worker.clone(), button));
    page.add(&screen);
    let identity = adw::PreferencesGroup::builder()
        .title("This computer")
        .build();
    identity.add(&widgets::address_group(&snapshot.endpoint_id, None));
    page.add(&identity);
    let reset_group = adw::PreferencesGroup::builder()
        .title("Installation identity")
        .description(
            "Resetting forgets all paired computers and permissions. Relay settings are kept.",
        )
        .build();
    let reset = gtk::Button::with_label("Reset identity…");
    reset.add_css_class("destructive-action");
    reset.set_halign(gtk::Align::Start);
    reset_group.add(&action_group(&[&reset]));
    page.add(&reset_group);
    let win = window.clone();
    reset.connect_clicked(move |_| {
        let prompt = adw::MessageDialog::new(Some(&win),Some("Reset this computer's identity?"),Some("All paired computers and permissions will be forgotten. VniLink will close; relaunch it to use the new identity. This cannot be undone."));
        prompt.add_responses(&[("cancel","Cancel"),("reset","Reset identity")]); prompt.set_response_appearance("reset",adw::ResponseAppearance::Destructive); prompt.set_close_response("cancel");
        let worker = core.clone(); let win = win.clone();
        prompt.connect_response(None,move |_,response| if response == "reset" {
            let worker = worker.clone(); let win = win.clone();
            tasks::run(move || worker.reset_identity().map_err(|e| e.to_string()),move |r| match r { Ok(()) => { if let Some(parent) = win.transient_for() { if let Some(app) = parent.application() { app.quit(); } } }, Err(e) => report(&win,&e) });
        }); prompt.present();
    });

    page
}

fn keyboard_page(window: &adw::Window, core: Arc<Core>) -> adw::PreferencesPage {
    let page = page("Keyboard", "input-keyboard-symbolic");
    let group = adw::PreferencesGroup::builder().title("Remote keyboard")
        .description("These options apply to connections from this computer for this launch. Release held keys before changing modes.").build();
    let mode = adw::ComboRow::builder()
        .title("Keyboard mode")
        .model(&gtk::StringList::new(&[
            "Characters",
            "Physical keys",
            "Translate to host layout",
        ]))
        .subtitle("Translate requires a Linux host with an X11 keyboard layout.")
        .build();
    let swap = adw::SwitchRow::builder()
        .title("Swap Control and Command")
        .subtitle("Use Control shortcuts when connecting to a Mac.")
        .build();
    group.add(&mode);
    group.add(&swap);
    page.add(&group);
    let worker = core.clone();
    let mode_initial = mode.clone();
    let swap_initial = swap.clone();
    let win = window.clone();
    tasks::run(
        move || {
            Ok((
                worker.keyboard_mode().map_err(|e| e.to_string())?,
                worker
                    .swap_control_and_command()
                    .map_err(|e| e.to_string())?,
            ))
        },
        move |result| {
            match result {
                Ok((keyboard, swapped)) => {
                    mode_initial.set_selected(keyboard as u32);
                    swap_initial.set_active(swapped);
                }
                Err(e) => report(&win, &e),
            }
            let worker = core.clone();
            let keyboard_win = win.clone();
            mode_initial.connect_selected_notify(move |row| {
                let mode = match row.selected() {
                    1 => KeyboardMode::Map,
                    2 => KeyboardMode::Translate,
                    _ => KeyboardMode::Character,
                };
                let worker = worker.clone();
                let win = keyboard_win.clone();
                tasks::run(
                    move || worker.set_keyboard_mode(mode).map_err(|e| e.to_string()),
                    move |r| {
                        if let Err(e) = r {
                            report(&win, &e);
                        }
                    },
                );
            });
            swap_initial.connect_active_notify(move |row| {
                let active = row.is_active();
                let worker = core.clone();
                let win = win.clone();
                tasks::run(
                    move || {
                        worker
                            .set_swap_control_and_command(active)
                            .map_err(|e| e.to_string())
                    },
                    move |r| {
                        if let Err(e) = r {
                            report(&win, &e);
                        }
                    },
                );
            });
        },
    );
    page
}

fn computers_page(
    window: &adw::Window,
    core: Arc<Core>,
    snapshot: &Snapshot,
) -> adw::PreferencesPage {
    let page = page("Computers", "computer-symbolic");
    let group = adw::PreferencesGroup::builder().title("Access to your desktop")
        .description("Permissions are directional. Choose what each paired computer may do on this computer.").build();
    if snapshot.devices.is_empty() {
        group.add(
            &adw::ActionRow::builder()
                .title("No paired computers")
                .subtitle("Add a computer using a pairing file or an invitation.")
                .build(),
        );
    }
    for device in &snapshot.devices {
        let row = adw::ActionRow::builder()
            .title(crate::presentation::device_name(&device))
            .subtitle(crate::presentation::grant_prose(device.grants))
            .use_markup(false)
            .build();
        let manage = gtk::Button::with_label("Manage");
        manage.set_valign(gtk::Align::Center);
        row.add_suffix(&manage);
        row.set_activatable_widget(Some(&manage));
        group.add(&row);
        let win = window.clone();
        let core = core.clone();
        let id = device.endpoint_id.clone();
        manage.connect_clicked(move |button| {
            let worker = core.clone();
            let core = core.clone();
            let id = id.clone();
            let win = win.clone();
            let row = row.clone();
            let button = button.clone();
            button.set_sensitive(false);
            tasks::run(
                move || {
                    worker
                        .snapshot()
                        .map_err(|e| e.to_string())?
                        .devices
                        .into_iter()
                        .find(|device| device.endpoint_id == id)
                        .ok_or_else(|| "Computer was forgotten".to_string())
                },
                move |result| {
                    button.set_sensitive(true);
                    match result {
                        Ok(device) => {
                            manage_computer(&win, core, device, row);
                        }
                        Err(error) => report(&win, &error),
                    }
                },
            );
        });
    }
    page.add(&group);
    page
}

pub(crate) fn manage_computer(
    parent: &adw::Window,
    core: Arc<Core>,
    device: vnilink::DeviceInfo,
    row: adw::ActionRow,
) -> adw::Window {
    let window = adw::Window::builder()
        .title(crate::presentation::device_name(&device))
        .transient_for(parent)
        .modal(true)
        .default_width(440)
        .default_height(650)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    let names = adw::PreferencesGroup::builder()
        .title("Computer name")
        .description("A name only you see. Leave it empty to use the computer's hostname.")
        .build();
    let name = adw::EntryRow::builder()
        .title("Local name")
        .text(device.pet_name.as_deref().unwrap_or(""))
        .build();
    name.set_widget_name("peer_pet_name");
    names.add(&name);
    if let Some(hostname) = &device.hostname {
        names.add(
            &adw::ActionRow::builder()
                .title("Reported hostname")
                .subtitle(hostname)
                .use_markup(false)
                .build(),
        );
    }
    content.append(&names);
    let rename = gtk::Button::with_label("Save name");
    rename.set_widget_name("save_peer_name");
    content.append(&rename);
    let worker = core.clone();
    let id = device.endpoint_id.clone();
    let win = window.clone();
    rename.connect_clicked(move |button| {
        let text = name.text().to_string();
        let core = worker.clone();
        let id = id.clone();
        let win = win.clone();
        let row = row.clone();
        let button = button.clone();
        button.set_sensitive(false);
        tasks::run(
            move || {
                core.set_peer_pet_name(
                    &id,
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(&text)
                    },
                )
                .map_err(|e| e.to_string())?;
                core.snapshot()
                    .map_err(|e| e.to_string())?
                    .devices
                    .into_iter()
                    .find(|d| d.endpoint_id == id)
                    .ok_or_else(|| "Computer was forgotten".to_string())
            },
            move |result| {
                button.set_sensitive(true);
                match result {
                    Ok(device) => {
                        let title = crate::presentation::device_name(&device);
                        row.set_title(&title);
                        win.set_title(Some(&title));
                    }
                    Err(error) => report(&win, &error),
                }
            },
        );
    });
    let group = adw::PreferencesGroup::builder()
        .title("Access to your desktop")
        .build();
    let control = adw::SwitchRow::builder()
        .title("Desktop control")
        .active(device.grants.control)
        .build();
    let clipboard = adw::SwitchRow::builder()
        .title("Clipboard text")
        .active(device.grants.clipboard)
        .build();
    let files = adw::SwitchRow::builder()
        .title("File transfer")
        .active(device.grants.transfer)
        .build();
    group.add(&control);
    group.add(&clipboard);
    group.add(&files);
    content.append(&group);
    let save = gtk::Button::with_label("Save permissions");
    save.add_css_class("suggested-action");
    content.append(&save);
    let revoke = gtk::Button::with_label("Revoke access");
    content.append(&revoke);
    let forget = gtk::Button::with_label("Forget computer");
    forget.add_css_class("destructive-action");
    content.append(&forget);
    view.set_content(Some(&content));
    window.set_content(Some(&view));
    window.present();
    let worker = core.clone();
    let id = device.endpoint_id.clone();
    let win = window.clone();
    save.connect_clicked(move |button| {
        let grants = Grants {
            control: control.is_active(),
            clipboard: clipboard.is_active(),
            transfer: files.is_active(),
        };
        let core = worker.clone();
        let id = id.clone();
        let win = win.clone();
        button.set_sensitive(false);
        let button = button.clone();
        tasks::run(
            move || core.authorize(&id, grants).map_err(|e| e.to_string()),
            move |r| {
                button.set_sensitive(true);
                match r {
                    Ok(()) => win.close(),
                    Err(e) => report(&win, &e),
                }
            },
        );
    });
    let worker = core.clone();
    let id = device.endpoint_id.clone();
    let win = window.clone();
    revoke.connect_clicked(move |_| {
        let worker = worker.clone();
        let id = id.clone();
        let win = win.clone();
        tasks::run(
            move || worker.revoke(&id).map_err(|e| e.to_string()),
            move |r| match r {
                Ok(()) => win.close(),
                Err(e) => report(&win, &e),
            },
        );
    });
    let win = window.clone();
    forget.connect_clicked(move |_| {
        let prompt = adw::MessageDialog::new(
            Some(&win),
            Some("Forget this computer?"),
            Some("It will lose access to your desktop. Pair it again to reconnect."),
        );
        prompt.add_responses(&[("cancel", "Cancel"), ("forget", "Forget")]);
        prompt.set_response_appearance("forget", adw::ResponseAppearance::Destructive);
        prompt.set_close_response("cancel");
        let worker = core.clone();
        let id = device.endpoint_id.clone();
        let win = win.clone();
        prompt.connect_response(None, move |_, response| {
            if response == "forget" {
                let worker = worker.clone();
                let id = id.clone();
                let win = win.clone();
                tasks::run(
                    move || worker.unpair(&id).map_err(|e| e.to_string()),
                    move |r| match r {
                        Ok(()) => win.close(),
                        Err(e) => report(&win, &e),
                    },
                );
            }
        });
        prompt.present();
    });
    window
}

fn pairing_page(window: &adw::Window, core: Arc<Core>) -> adw::PreferencesPage {
    let page = page("Pairing", "insert-link-symbolic");
    let group = adw::PreferencesGroup::builder().title("Invitation tokens")
        .description("Use a one-time invitation instead of a pairing file. Share tokens only with the computer you want to pair.").build();
    let token = adw::PasswordEntryRow::builder()
        .title("Invitation token")
        .build();
    group.add(&token);
    let add = gtk::Button::with_label("Accept invitation");
    add.add_css_class("suggested-action");
    add.set_halign(gtk::Align::Start);

    let worker = core.clone();
    let win = window.clone();
    add.connect_clicked(move |button| {
        let value = token.text().to_string();
        if value.is_empty() {
            return;
        }
        button.set_sensitive(false);
        let button = button.clone();
        let token = token.clone();
        let worker = worker.clone();
        let win = win.clone();
        tasks::run(
            move || worker.accept_invitation(&value).map_err(|e| e.to_string()),
            move |r| {
                button.set_sensitive(true);
                match r {
                    Ok(_) => {
                        token.set_text("");
                        toast(&win, adw::Toast::new("Computer added."));
                    }
                    Err(e) => report(&win, &e),
                }
            },
        );
    });
    let create = gtk::Button::with_label("Copy a new invitation");
    create.set_halign(gtk::Align::Start);
    group.add(&action_group(&[&add, &create]));
    let win = window.clone();
    create.connect_clicked(move |button| {
        button.set_sensitive(false);
        let button = button.clone();
        let core = core.clone();
        let win = win.clone();
        tasks::run(
            move || {
                core.create_invitation(std::time::Duration::from_secs(900))
                    .map_err(|e| e.to_string())
            },
            move |r| {
                button.set_sensitive(true);
                match r {
                    Ok(token) => {
                        win.clipboard().set_text(&token);
                        toast(
                            &win,
                            adw::Toast::new("Invitation copied. It expires in 15 minutes."),
                        );
                    }
                    Err(e) => report(&win, &e),
                }
            },
        );
    });
    page.add(&group);
    page
}

pub(crate) fn report(window: &impl IsA<gtk::Window>, error: &str) {
    let dialog = adw::MessageDialog::new(
        Some(window),
        Some("Could not complete the action"),
        Some(error),
    );
    dialog.add_response("close", "Close");
    dialog.present();
}

/// Settings always uses a left sidebar; no adaptive top/bottom tab bar.
pub(crate) fn shell(parent: &adw::ApplicationWindow) -> adw::Window {
    let window = adw::Window::builder()
        .title("Settings")
        .transient_for(parent)
        .default_width(900)
        .default_height(760)
        .width_request(640)
        .height_request(480)
        .build();
    let toast = adw::ToastOverlay::new();
    toast.set_widget_name("settings_toasts");
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar.set_width_request(180);
    sidebar.add_css_class("sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let sections = gtk::ListBox::new();
    sections.set_widget_name("settings_sections");
    sections.add_css_class("navigation-sidebar");
    sections.set_margin_top(12);
    sections.set_margin_start(8);
    sections.set_margin_end(8);
    scroll.set_child(Some(&sections));
    sidebar.append(&scroll);
    layout.append(&sidebar);
    layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    let pages = gtk::Stack::new();
    pages.set_widget_name("settings_pages");
    pages.set_hexpand(true);
    pages.set_vexpand(true);
    pages.set_transition_type(gtk::StackTransitionType::Crossfade);
    let stack = pages.clone();
    sections.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            stack.set_visible_child_name(&row.widget_name());
        }
    });
    layout.append(&pages);
    view.set_content(Some(&layout));
    toast.set_child(Some(&view));
    window.set_content(Some(&toast));
    window
}

pub(crate) fn toast(window: &adw::Window, toast: adw::Toast) {
    widgets::named::<adw::ToastOverlay>(window, "settings_toasts").add_toast(toast);
}

#[cfg(test)]
pub(crate) fn show_page(window: &adw::Window, name: &str) {
    let sections = widgets::named::<gtk::ListBox>(window, "settings_sections");
    let mut index = 0;
    while let Some(row) = sections.row_at_index(index) {
        if row.widget_name() == name {
            sections.select_row(Some(&row));
            return;
        }
        index += 1;
    }
    panic!("missing settings section {name}");
}

pub(crate) fn action_group(buttons: &[&gtk::Button]) -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 12);
    actions.set_margin_top(12);
    actions.set_margin_bottom(4);
    for button in buttons {
        button.set_halign(gtk::Align::Start);
        actions.append(*button);
    }
    actions
}
