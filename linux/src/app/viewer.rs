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
        let tools = gtk::MenuButton::builder().label("Session").build();
        tools.set_popover(Some(&session_popover(&window, core.clone(), &session)));
        header.pack_start(&tools);
        let send_file = gtk::Button::with_label("Send file…");
        send_file.set_tooltip_text(Some("Send a file, or drop one onto the remote desktop"));
        let win = window.clone();
        let worker = core.clone();
        let (id, generation) = (session.id, session.generation);
        send_file
            .connect_clicked(move |_| super::files::send(&win, worker.clone(), id, generation));
        header.pack_start(&send_file);
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
            "Remote desktop; move to control the pointer, click to send keyboard input",
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
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&view));
        window.set_content(Some(&toasts));
        super::files::install_drop(
            &picture,
            &window,
            core.clone(),
            session.id,
            session.generation,
        );

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
        let transfer_events = core.subscribe().ok();
        let owner = Rc::downgrade(&viewer);
        glib::timeout_add_local(Duration::from_millis(200), move || {
            let Some(viewer) = owner.upgrade().filter(|v| !v.stop.load(Ordering::Relaxed)) else {
                return ControlFlow::Break;
            };
            if let Some(events) = &transfer_events {
                while let Some(event) = events.try_recv() {
                    if matches!(event, vnilink::CoreEvent::TransferCompleted { id } if id == viewer.session.id)
                    {
                        toasts.add_toast(adw::Toast::new("File saved in your receive folder."));
                    }
                }
            }
            ControlFlow::Continue
        });
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
                let mut ticks = 0u32;
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
                        ticks = ticks.saturating_add(1);
                        if ticks % 20 == 0 {
                            let _ = worker.auto_quality_tick(session.id);
                        }
                        continue;
                    };
                    ticks = ticks.saturating_add(1);
                    if ticks % 20 == 0 {
                        let _ = worker.auto_quality_tick(session.id);
                    }
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
        let mut cursor_image: Option<Arc<vnilink::CursorImage>> = None;
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(viewer) = weak.upgrade() else {
                return ControlFlow::Break;
            };
            if viewer.stop.load(Ordering::Relaxed) {
                return ControlFlow::Break;
            }
            if let Ok(Some(cursor)) = core.cursor(viewer.session.id, viewer.session.generation) {
                if !cursor.visible {
                    viewer.picture.set_cursor_from_name(Some("none"));
                } else if let Some(image) = cursor.image.as_ref() {
                    if cursor_image.as_ref() != Some(image) {
                        let texture = gtk::gdk::MemoryTexture::new(
                            i32::from(image.width()),
                            i32::from(image.height()),
                            gtk::gdk::MemoryFormat::B8g8r8a8Premultiplied,
                            &glib::Bytes::from_owned(image.pixels().to_vec()),
                            usize::from(image.width()) * 4,
                        );
                        let native = gtk::gdk::Cursor::from_texture(
                            &texture,
                            i32::from(image.hotspot().0),
                            i32::from(image.hotspot().1),
                            None,
                        );
                        viewer.picture.set_cursor(Some(&native));
                        cursor_image = Some(Arc::clone(image));
                    } else if viewer
                        .picture
                        .cursor()
                        .is_some_and(|cursor| cursor.name().as_deref() == Some("none"))
                    {
                        // Recreate the cached shape after an invisible update.
                        cursor_image = None;
                    }
                }
            }
            if let Some(result) = latest.lock().unwrap().take() {
                match result {
                    Ok(frame) => {
                        viewer.show_frame(&frame);
                        viewer.status.set_label(&format!(
                            "{} × {} · Click to send keyboard input",
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
            Paste(u16, String, std::sync::mpsc::Receiver<Option<String>>),
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Command>(128);
        let (notices, feedback) = std::sync::mpsc::sync_channel::<String>(1);
        let window = self.window.downgrade();
        let alive = self.stop.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(window) = window.upgrade().filter(|_| !alive.load(Ordering::Relaxed)) else {
                return ControlFlow::Break;
            };
            if let Ok(message) = feedback.try_recv() {
                super::files::feedback(window.upcast_ref(), &message);
            }
            ControlFlow::Continue
        });
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
                    Command::Paste(usage, text, clipboard) => {
                        if let Ok(Some(copy)) = clipboard.recv_timeout(Duration::from_millis(500)) {
                            // Apply the clipboard before the shortcut, on the same
                            // ordered input worker; UI and video remain responsive.
                            if let Err(e) = worker.submit_clipboard(id, generation, &copy) {
                                let _ = notices
                                    .try_send(format!("Clipboard paste could not sync: {e}"));
                            }
                        }
                        worker.submit_local_key(
                            id,
                            generation,
                            LocalKey {
                                usage,
                                down: true,
                                text: &text,
                            },
                        )
                    }
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
        let send = enqueue.clone();
        install_pointer_motion(
            &self.picture,
            self.size.clone(),
            (self.session.monitor_width, self.session.monitor_height),
            move |x, y| send(Command::Event(InputEvent::pointer(x, y))),
        );
        let host = (self.session.monitor_width, self.session.monitor_height);
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
        let synthetic_modifiers = Rc::new(RefCell::new(BTreeSet::new()));
        let synthetic = synthetic_modifiers.clone();
        let keys = gtk::EventControllerKey::new();
        let send = enqueue.clone();
        let held = held_keys.clone();
        let board = self.window.clipboard();
        keys.connect_key_pressed(move |_, key, keycode, state| {
            let Some(usage) = crate::viewer::key_usage(keycode) else {
                return Propagation::Proceed;
            };
            if !(224..=231).contains(&usage) {
                synchronize_modifiers(&mut held.borrow_mut(), state, |usage, down| {
                    if down {
                        synthetic.borrow_mut().insert(usage);
                    } else {
                        synthetic.borrow_mut().remove(&usage);
                    }
                    send(Command::Key(usage, down, String::new()))
                });
            }
            held.borrow_mut().insert(usage);
            let text = key
                .to_unicode()
                .filter(|c| !c.is_control())
                .map(|c| c.to_string())
                .unwrap_or_default();
            if usage == 25
                && state.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::META_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                )
            {
                let (reply, clipboard) = std::sync::mpsc::channel();
                send(Command::Paste(usage, text, clipboard));
                board.read_text_async(None::<&gtk::gio::Cancellable>, move |result| {
                    let _ = reply.send(
                        result
                            .ok()
                            .flatten()
                            .map(|text| text.to_string())
                            .filter(|text| text.len() <= vnilink::MAX_CLIPBOARD_TEXT),
                    );
                });
            } else {
                send(Command::Key(usage, true, text));
            }
            Propagation::Stop
        });
        let send = enqueue.clone();
        let held = held_keys.clone();
        let synthetic = synthetic_modifiers.clone();
        keys.connect_key_released(move |_, _, keycode, _| {
            if let Some(usage) = crate::viewer::key_usage(keycode) {
                let counterpart = if usage < 228 { usage + 4 } else { usage - 4 };
                let usage = if !held.borrow_mut().remove(&usage)
                    && (224..=231).contains(&usage)
                    && synthetic.borrow_mut().remove(&counterpart)
                {
                    held.borrow_mut().remove(&counterpart);
                    counterpart
                } else {
                    synthetic.borrow_mut().remove(&usage);
                    usage
                };
                send(Command::Key(usage, false, String::new()));
            }
        });
        self.picture.add_controller(keys);
        let release: Rc<dyn Fn()> = Rc::new(move || {
            synthetic_modifiers.borrow_mut().clear();
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

fn session_popover(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    session: &SessionInfo,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
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
    let automatic = gtk::Label::new(Some("Video quality adjusts automatically.\nClipboard text syncs while connected, with permission."));
    automatic.set_wrap(true);
    automatic.set_max_width_chars(36);
    automatic.set_xalign(0.0);
    content.append(&automatic);
    let generation = session.generation;
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

/// Mouse motion belongs to the image under the pointer; keyboard focus is
/// acquired by clicking and is handled separately by the key controller.
pub(super) fn install_pointer_motion(
    picture: &gtk::Picture,
    size: Rc<Cell<(u16, u16)>>,
    host: (u16, u16),
    send: impl Fn(u16, u16) + 'static,
) -> gtk::EventControllerMotion {
    let picture_for_controller = picture;
    let motion = gtk::EventControllerMotion::new();
    let picture = picture.downgrade();
    let last_position = Cell::new(None);
    motion.connect_motion(move |_, x, y| {
        let Some(picture) = picture.upgrade() else {
            return;
        };
        if let Some((x, y)) = crate::viewer::pointer(
            x,
            y,
            (f64::from(picture.width()), f64::from(picture.height())),
            size.get(),
            host,
        ) {
            // Replacing the paintable can cause GTK to emit motion without
            // hardware movement. Resending it fights the host's local mouse.
            if last_position.replace(Some((x, y))) != Some((x, y)) {
                send(x, y);
            }
        }
    });
    picture_for_controller.add_controller(motion.clone());
    motion
}

/// Recover modifiers pressed before the remote image acquired keyboard focus.
fn synchronize_modifiers(
    held: &mut BTreeSet<u16>,
    flags: gtk::gdk::ModifierType,
    mut send: impl FnMut(u16, bool),
) {
    for (flag, left, right) in [
        (gtk::gdk::ModifierType::CONTROL_MASK, 224, 228),
        (gtk::gdk::ModifierType::SHIFT_MASK, 225, 229),
        (gtk::gdk::ModifierType::ALT_MASK, 226, 230),
        (
            gtk::gdk::ModifierType::META_MASK | gtk::gdk::ModifierType::SUPER_MASK,
            227,
            231,
        ),
    ] {
        if flags.intersects(flag) {
            if !held.contains(&left) && !held.contains(&right) {
                held.insert(left);
                send(left, true);
            }
        } else {
            for usage in [left, right] {
                if held.remove(&usage) {
                    send(usage, false);
                }
            }
        }
    }
}
#[cfg(test)]
mod keyboard_tests {
    use super::*;
    #[test]
    fn shortcuts_recover_held_modifiers_without_duplicate_or_stuck_keys() {
        let mut held = BTreeSet::from([228]);
        let mut events = Vec::new();
        synchronize_modifiers(
            &mut held,
            gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK,
            |u, d| events.push((u, d)),
        );
        assert_eq!(events, [(225, true)]);
        events.clear();
        synchronize_modifiers(&mut held, gtk::gdk::ModifierType::empty(), |u, d| {
            events.push((u, d))
        });
        assert_eq!(events, [(228, false), (225, false)]);
        assert!(held.is_empty());
    }
}
