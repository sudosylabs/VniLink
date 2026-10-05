//! Remote desktop window. Frames are decoded off-thread and only the newest
//! complete BGRA frame is uploaded to GTK. Input is fenced by session generation.
use super::{preferences, tasks};
use adw::prelude::*;
use gtk::glib::{self, ControlFlow, Propagation};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vnilink::{Core, InputEvent, LocalKey, PointerButton, SessionInfo, SessionPhase};
use vnilink_media::{ControllerOutput, ControllerVideo, RawFrame};

pub(crate) struct Viewer {
    pub window: adw::ApplicationWindow,
    pub picture: gtk::Picture,
    pub status: gtk::Label,
    stack: gtk::Stack,
    stop: Arc<AtomicBool>,
    disconnect_on_close: Rc<Cell<bool>>,
    pub session: SessionInfo,
    size: Rc<Cell<(u16, u16)>>,
}

impl Viewer {
    pub(crate) fn open(
        parent: &adw::ApplicationWindow,
        core: Arc<Core>,
        session: SessionInfo,
    ) -> Rc<Self> {
        let viewer = Self::build(parent, core.clone(), session);
        viewer.start(core);
        viewer.window.present();
        viewer
    }

    pub(crate) fn build(
        parent: &adw::ApplicationWindow,
        core: Arc<Core>,
        session: SessionInfo,
    ) -> Rc<Self> {
        let title = core
            .snapshot()
            .ok()
            .and_then(|s| {
                s.devices
                    .into_iter()
                    .find(|d| d.endpoint_id == session.peer_id)
            })
            .map(|d| crate::presentation::device_name(&d))
            .unwrap_or_else(|| crate::presentation::computer_name(&session.peer_id));
        let window = adw::ApplicationWindow::builder()
            .title(&title)
            .default_width(1120)
            .default_height(760)
            .build();
        if let Some(application) = parent.application() {
            window.set_application(Some(&application));
        }
        let view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&adw::WindowTitle::new(&title, "Remote desktop")));
        let quality = gtk::MenuButton::builder().label("Quality").build();
        quality.set_popover(Some(&quality_popover(&window, core.clone(), &session)));
        header.pack_start(&quality);
        let tools = gtk::MenuButton::builder().label("Session").build();
        tools.set_popover(Some(&session_popover(&window, core.clone(), &session)));
        header.pack_start(&tools);
        let fullscreen = gtk::ToggleButton::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text("Full screen")
            .build();
        let win = window.clone();
        fullscreen.connect_toggled(move |button| {
            if button.is_active() {
                win.fullscreen();
            } else {
                win.unfullscreen();
            }
        });
        header.pack_end(&fullscreen);
        let disconnect = gtk::Button::with_label("Disconnect");
        header.pack_end(&disconnect);
        view.add_top_bar(&header);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        let waiting = gtk::Box::new(gtk::Orientation::Vertical, 12);
        waiting.set_halign(gtk::Align::Center);
        waiting.set_valign(gtk::Align::Center);
        let heading = gtk::Label::new(Some("Waiting for the desktop"));
        heading.add_css_class("title-2");
        waiting.append(&heading);
        let description = gtk::Label::new(Some(
            "The connection is open. The host is preparing its screen.",
        ));
        description.set_wrap(true);
        waiting.append(&description);
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::Center)
            .build();
        waiting.append(&spinner);
        stack.add_named(&waiting, Some("waiting"));
        let picture = gtk::Picture::builder()
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .focusable(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        picture.set_widget_name("remote_desktop");
        picture.update_property(&[gtk::accessible::Property::Label(
            "Remote desktop; click to send keyboard and pointer input",
        )]);
        stack.add_named(&picture, Some("desktop"));
        stack.set_visible_child_name("waiting");
        view.set_content(Some(&stack));
        let status = gtk::Label::builder()
            .label("Connecting video…")
            .xalign(0.0)
            .margin_start(16)
            .margin_end(16)
            .margin_top(8)
            .margin_bottom(8)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("caption");
        view.add_bottom_bar(&status);
        window.set_content(Some(&view));
        let viewer = Rc::new(Self {
            window,
            picture,
            status,
            stack,
            stop: Arc::new(AtomicBool::new(false)),
            disconnect_on_close: Rc::new(Cell::new(true)),
            session,
            size: Rc::new(Cell::new((0, 0))),
        });
        viewer.install_input(core.clone());
        let weak = Rc::downgrade(&viewer);
        disconnect.connect_clicked(move |_| {
            if let Some(viewer) = weak.upgrade() {
                viewer.window.close();
            }
        });
        let stop = viewer.stop.clone();
        let id = viewer.session.id;
        let generation = viewer.session.generation;
        let disconnect_on_close = viewer.disconnect_on_close.clone();
        let picture = viewer.picture.downgrade();
        viewer.window.connect_close_request(move |_| {
            stop.store(true, Ordering::Relaxed);
            if let Some(picture) = picture.upgrade() {
                picture.set_paintable(None::<&gtk::gdk::Paintable>);
            }
            if !disconnect_on_close.get() {
                return Propagation::Proceed;
            }
            let core = core.clone();
            tasks::run(
                move || {
                    if core
                        .snapshot()
                        .ok()
                        .and_then(|s| s.outgoing)
                        .is_some_and(|s| s.id == id && s.generation == generation)
                    {
                        core.disconnect().map_err(|e| e.to_string())?;
                    }
                    Ok(())
                },
                |_| {},
            );
            Propagation::Proceed
        });
        viewer
    }

    fn retire(&self) {
        self.disconnect_on_close.set(false);
        self.stop.store(true, Ordering::Relaxed);
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
        self.window.close();
    }

    pub(crate) fn show_frame(&self, frame: &RawFrame) {
        // X11's BGRx and decoded BGRA may have a zero fourth byte. The remote
        // desktop is opaque, so normalize alpha rather than painting it transparent.
        let mut pixels = frame.pixels().to_vec();
        for row in pixels
            .chunks_mut(frame.stride())
            .take(usize::from(frame.height()))
        {
            for pixel in row[..usize::from(frame.width()) * 4].chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        let bytes = glib::Bytes::from_owned(pixels);
        let texture = gtk::gdk::MemoryTexture::new(
            i32::from(frame.width()),
            i32::from(frame.height()),
            gtk::gdk::MemoryFormat::B8g8r8a8,
            &bytes,
            frame.stride(),
        );
        self.picture.set_paintable(Some(&texture));
        self.size.set((frame.width(), frame.height()));
        self.stack.set_visible_child_name("desktop");
    }

    fn start(self: &Rc<Self>, core: Arc<Core>) {
        let latest: Arc<Mutex<Option<Result<RawFrame, String>>>> = Arc::new(Mutex::new(None));
        let delivery = latest.clone();
        let stop = self.stop.clone();
        let session = self.session.clone();
        let worker = core.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                // A reconnect's previous decoder may still be releasing its subscription.
                let mut subscription = None;
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match worker.subscribe_video() {
                        Ok(value) => {
                            subscription = Some(value);
                            break;
                        }
                        Err(vnilink::Error::Busy) => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => return Err(e.to_string()),
                    }
                }
                let subscription =
                    subscription.ok_or("The previous video session is still closing.")?;
                let mut controller = ControllerVideo::new(vnilink_linux::LinuxDecoder::new());
                while !stop.load(Ordering::Relaxed) {
                    let snapshot = worker.snapshot().map_err(|e| e.to_string())?;
                    if !snapshot.outgoing.is_some_and(|s| {
                        s.id == session.id
                            && s.generation == session.generation
                            && s.phase == SessionPhase::Active
                    }) {
                        return Err("The session ended or is reconnecting.".into());
                    }
                    let Some(frame) = subscription.recv_timeout(Duration::from_millis(100)) else {
                        continue;
                    };
                    match controller.push(&frame).map_err(|e| e.to_string())? {
                        ControllerOutput::Decoded(frame) => {
                            *delivery.lock().unwrap() = Some(Ok(frame))
                        }
                        ControllerOutput::Skipped => {}
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                *delivery.lock().unwrap() = Some(Err(error));
            }
        });
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(viewer) = weak.upgrade() else {
                return ControlFlow::Break;
            };
            if viewer.stop.load(Ordering::Relaxed) {
                return ControlFlow::Break;
            }
            if let Some(result) = latest.lock().unwrap().take() {
                match result {
                    Ok(frame) => {
                        viewer.show_frame(&frame);
                        viewer.status.set_label(&format!(
                            "{} × {} · Click the desktop to control it",
                            frame.width(),
                            frame.height()
                        ));
                    }
                    Err(error) => {
                        viewer.status.set_label(&error);
                        viewer.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                        viewer.size.set((0, 0));
                        viewer.stack.set_visible_child_name("waiting");
                    }
                }
            }
            ControlFlow::Continue
        });
    }

    fn install_input(&self, core: Arc<Core>) {
        enum Command {
            Event(InputEvent),
            Key(u16, bool, String),
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Command>(128);
        let id = self.session.id;
        let generation = self.session.generation;
        let stop = self.stop.clone();
        let worker = core.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let command = match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(c) => c,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let result = match command {
                    Command::Event(event) => worker.submit_input(id, generation, event),
                    Command::Key(usage, down, text) => worker.submit_local_key(
                        id,
                        generation,
                        LocalKey {
                            usage,
                            down,
                            text: &text,
                        },
                    ),
                };
                if result.is_err() {
                    break;
                }
            }
        });
        let fail_stop = self.stop.clone();
        let status = self.status.clone();
        let enqueue: Rc<dyn Fn(Command)> = Rc::new(move |command| {
            if sender.try_send(command).is_err() {
                fail_stop.store(true, Ordering::Relaxed);
                status.set_label("Input stopped. Disconnect and reconnect to continue.");
                let core = core.clone();
                tasks::run(
                    move || {
                        if core
                            .snapshot()
                            .ok()
                            .and_then(|s| s.outgoing)
                            .is_some_and(|s| s.id == id && s.generation == generation)
                        {
                            core.disconnect().map_err(|e| e.to_string())?;
                        }
                        Ok(())
                    },
                    |_| {},
                );
            }
        });
        let motion = gtk::EventControllerMotion::new();
        let picture = self.picture.downgrade();
        let size = self.size.clone();
        let host = (self.session.monitor_width, self.session.monitor_height);
        let send = enqueue.clone();
        motion.connect_motion(move |_, x, y| {
            let Some(picture) = picture.upgrade() else {
                return;
            };
            if picture.has_focus() {
                if let Some((x, y)) = crate::viewer::pointer(
                    x,
                    y,
                    (f64::from(picture.width()), f64::from(picture.height())),
                    size.get(),
                    host,
                ) {
                    send(Command::Event(InputEvent::pointer(x, y)));
                }
            }
        });
        self.picture.add_controller(motion);
        let held_buttons = Rc::new(RefCell::new(BTreeSet::new()));
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let picture = self.picture.downgrade();
        let size = self.size.clone();
        let send = enqueue.clone();
        let held = held_buttons.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(picture) = picture.upgrade() else {
                return;
            };
            let Some((x, y)) = crate::viewer::pointer(
                x,
                y,
                (f64::from(picture.width()), f64::from(picture.height())),
                size.get(),
                host,
            ) else {
                return;
            };
            picture.grab_focus();
            send(Command::Event(InputEvent::pointer(x, y)));
            if let Some(button) = pointer_button(gesture.current_button()) {
                held.borrow_mut().insert(gesture.current_button());
                send(Command::Event(InputEvent::button_down(button)));
            }
        });
        let send = enqueue.clone();
        let held = held_buttons.clone();
        click.connect_released(move |gesture, _, _, _| {
            if let Some(button) = pointer_button(gesture.current_button()) {
                held.borrow_mut().remove(&gesture.current_button());
                send(Command::Event(InputEvent::button_up(button)));
            }
        });
        self.picture.add_controller(click);
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let send = enqueue.clone();
        let picture = self.picture.downgrade();
        scroll.connect_scroll(move |_, dx, dy| {
            let Some(picture) = picture.upgrade() else {
                return Propagation::Proceed;
            };
            if picture.has_focus() {
                send(Command::Event(InputEvent::wheel(
                    dx.round().clamp(-32768., 32767.) as i16,
                    dy.round().clamp(-32768., 32767.) as i16,
                )));
                Propagation::Stop
            } else {
                Propagation::Proceed
            }
        });
        self.picture.add_controller(scroll);
        let held_keys = Rc::new(RefCell::new(BTreeSet::new()));
        let keys = gtk::EventControllerKey::new();
        let send = enqueue.clone();
        let held = held_keys.clone();
        keys.connect_key_pressed(move |_, key, keycode, _| {
            let Some(usage) = crate::viewer::key_usage(keycode) else {
                return Propagation::Proceed;
            };
            held.borrow_mut().insert(usage);
            let text = key
                .to_unicode()
                .filter(|c| !c.is_control())
                .map(|c| c.to_string())
                .unwrap_or_default();
            send(Command::Key(usage, true, text));
            Propagation::Stop
        });
        let send = enqueue.clone();
        let held = held_keys.clone();
        keys.connect_key_released(move |_, _, keycode, _| {
            if let Some(usage) = crate::viewer::key_usage(keycode) {
                held.borrow_mut().remove(&usage);
                send(Command::Key(usage, false, String::new()));
            }
        });
        self.picture.add_controller(keys);
        let release: Rc<dyn Fn()> = Rc::new(move || {
            for usage in std::mem::take(&mut *held_keys.borrow_mut()) {
                enqueue(Command::Key(usage, false, String::new()));
            }
            for button in std::mem::take(&mut *held_buttons.borrow_mut()) {
                if let Some(button) = pointer_button(button) {
                    enqueue(Command::Event(InputEvent::button_up(button)));
                }
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let release_focus = release.clone();
        focus.connect_leave(move |_| release_focus());
        self.window.connect_is_active_notify(move |window| {
            if !window.is_active() {
                release();
            }
        });
        self.picture.add_controller(focus);
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn pointer_button(button: u32) -> Option<PointerButton> {
    match button {
        1 => Some(PointerButton::Left),
        2 => Some(PointerButton::Middle),
        3 => Some(PointerButton::Right),
        _ => None,
    }
}

fn quality_popover(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    session: &SessionInfo,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let group = adw::PreferencesGroup::builder()
        .title("Video quality")
        .build();
    let quality = session
        .quality
        .unwrap_or(vnilink::Quality::new(8000, 30, 1920, 1080).unwrap());
    let bitrate = adw::SpinRow::with_range(1., 50000., 500.);
    bitrate.set_title("Bitrate (kbps)");
    bitrate.set_value(f64::from(quality.max_bitrate_kbps));
    let fps = adw::SpinRow::with_range(1., 60., 1.);
    fps.set_title("Frames per second");
    fps.set_value(f64::from(quality.max_frame_rate));
    let width = adw::SpinRow::with_range(2., 7680., 2.);
    width.set_title("Maximum width");
    width.set_value(f64::from(quality.max_width));
    let height = adw::SpinRow::with_range(2., 7680., 2.);
    height.set_title("Maximum height");
    height.set_value(f64::from(quality.max_height));
    group.add(&bitrate);
    group.add(&fps);
    group.add(&width);
    group.add(&height);
    content.append(&group);
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    content.append(&apply);
    let win = window.clone();
    let id = session.id;
    let generation = session.generation;
    apply.connect_clicked(move |button| {
        let quality = vnilink::Quality::new(
            bitrate.value() as u32,
            fps.value() as u8,
            width.value() as u16,
            height.value() as u16,
        )
        .unwrap();
        let worker = core.clone();
        let win = win.clone();
        button.set_sensitive(false);
        let button = button.clone();
        tasks::run(
            move || {
                if !worker
                    .snapshot()
                    .map_err(|e| e.to_string())?
                    .outgoing
                    .is_some_and(|s| {
                        s.id == id && s.generation == generation && s.phase == SessionPhase::Active
                    })
                {
                    return Err("The session changed. Reopen the quality controls.".into());
                }
                worker
                    .update_session_quality(id, vnilink::SessionRole::Controller, quality)
                    .map_err(|e| e.to_string())
            },
            move |r| {
                button.set_sensitive(true);
                if let Err(e) = r {
                    preferences::report(&win, &e);
                }
            },
        );
    });
    popover.set_child(Some(&content));
    popover
}

fn session_popover(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    session: &SessionInfo,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let diagnostics = gtk::Button::with_label("Connection details");
    content.append(&diagnostics);
    let worker = core.clone();
    let win = window.clone();
    let id = session.id;
    diagnostics.connect_clicked(move |_| {
        let worker = worker.clone();
        let win = win.clone();
        tasks::run(
            move || worker.session_paths(id).map_err(|e| e.to_string()),
            move |r| match r {
                Ok(paths) => {
                    let text = if paths.is_empty() {
                        "No active transport path.".into()
                    } else {
                        paths
                            .into_iter()
                            .map(|p| {
                                format!(
                                    "{}{} · {} ms · {} sent / {} received bytes",
                                    if p.relay { "Relay" } else { "Direct" },
                                    if p.selected { " (active)" } else { "" },
                                    p.rtt_ms,
                                    p.tx_bytes,
                                    p.rx_bytes
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    let dialog = adw::MessageDialog::new(
                        Some(&win),
                        Some("Connection details"),
                        Some(&text),
                    );
                    dialog.add_response("close", "Close");
                    dialog.present();
                }
                Err(e) => preferences::report(&win, &e),
            },
        );
    });
    let send_clipboard = gtk::Button::with_label("Send clipboard text");
    content.append(&send_clipboard);
    let worker = core.clone();
    let win = window.clone();
    let generation = session.generation;
    send_clipboard
        .connect_clicked(move |_| super::clipboard::send(&win, worker.clone(), id, generation));
    let receive = gtk::Button::with_label("Copy remote clipboard");
    content.append(&receive);
    let worker = core.clone();
    let win = window.clone();
    receive.connect_clicked(move |_| super::clipboard::receive(&win, worker.clone()));
    let send_file = gtk::Button::with_label("Send file…");
    content.append(&send_file);
    let worker = core.clone();
    let win = window.clone();
    send_file.connect_clicked(move |_| super::files::send(&win, worker.clone(), id, generation));
    let text = gtk::Button::with_label("Send text…");
    content.append(&text);
    let worker = core.clone();
    let win = window.clone();
    text.connect_clicked(move |_| {
        let dialog = adw::MessageDialog::new(
            Some(&win),
            Some("Send text"),
            Some("Type a short phrase on the remote computer (up to 128 UTF-8 bytes)."),
        );
        let entry = gtk::Entry::new();
        entry.set_max_length(128);
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", "Cancel"), ("send", "Send")]);
        dialog.set_response_appearance("send", adw::ResponseAppearance::Suggested);
        let worker = worker.clone();
        let win = win.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "send" {
                let text = entry.text().to_string();
                let worker = worker.clone();
                let win = win.clone();
                tasks::run(
                    move || {
                        worker
                            .submit_text(id, generation, &text)
                            .map_err(|e| e.to_string())
                    },
                    move |r| {
                        if let Err(e) = r {
                            preferences::report(&win, &e);
                        }
                    },
                );
            }
        });
        dialog.present();
    });
    let keyboard = gtk::Button::with_label("Keyboard settings");
    content.append(&keyboard);
    let win = window.clone();
    keyboard.connect_clicked(move |_| preferences::open(&win, core.clone()));
    popover.set_child(Some(&content));
    popover
}

/// Own one viewer per active outgoing generation, and close it when its
/// generation ends. Retiring a reconnecting viewer must not cancel redial.
pub(crate) fn watch(parent: &adw::ApplicationWindow, core: Arc<Core>) {
    let parent = parent.downgrade();
    let current: Rc<RefCell<Option<Rc<Viewer>>>> = Rc::new(RefCell::new(None));
    glib::timeout_add_local(Duration::from_millis(200), move || {
        let Some(parent) = parent.upgrade().filter(|p| p.get_visible()) else {
            if let Some(viewer) = current.borrow_mut().take() {
                viewer.retire();
            }
            return ControlFlow::Break;
        };
        let active = core
            .snapshot()
            .ok()
            .and_then(|s| s.outgoing)
            .filter(|s| s.phase == SessionPhase::Active);
        let same = current.borrow().as_ref().is_some_and(|v| {
            active
                .as_ref()
                .is_some_and(|s| s.id == v.session.id && s.generation == v.session.generation)
        });
        if !same {
            if let Some(viewer) = current.borrow_mut().take() {
                viewer.retire();
            }
            if let Some(session) = active {
                current.replace(Some(Viewer::open(&parent, core.clone(), session)));
            }
        }
        ControlFlow::Continue
    });
}
