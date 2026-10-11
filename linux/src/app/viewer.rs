//! Remote desktop tab content. Frames are decoded off-thread and only the newest
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
use vnilink_media::{ControllerOutput, ControllerVideo, DecodedFrame};

/// Native runtime ownership, shared by the video worker and input callbacks.
/// Closing during Open cancels the admitted record before the backend replies.
struct DesktopBackend {
    core: Arc<Core>,
    state: Mutex<(
        bool,
        Option<vnilink::ServiceInfo>,
        Option<Arc<vnilink::RemoteDesktopService>>,
    )>,
}
impl DesktopBackend {
    fn new(core: Arc<Core>) -> Self {
        Self {
            core,
            state: Mutex::new((false, None, None)),
        }
    }
    fn open(
        &self,
        session: &SessionInfo,
    ) -> Result<Arc<vnilink::RemoteDesktopService>, vnilink::Error> {
        let info = self.core.attach_controller_service(
            session.id,
            session.generation,
            vnilink::ServiceKind::Desktop,
        )?;
        {
            let mut state = self.state.lock().unwrap();
            if state.0 {
                drop(state);
                let _ = self.core.close_controller_service(info.id, info.generation);
                let _ = self.core.forget_controller_service(info.id);
                return Err(vnilink::Error::NoSession);
            }
            state.1 = Some(info.clone());
        }
        let next = match self.core.open_desktop_service(info.id, info.generation) {
            Ok(owner) => Arc::new(owner),
            Err(error) => {
                self.close();
                return Err(error);
            }
        };
        let mut state = self.state.lock().unwrap();
        if state.0 {
            drop(state);
            next.close();
            return Err(vnilink::Error::NoSession);
        }
        state.2 = Some(next.clone());
        Ok(next)
    }
    fn current(&self) -> Option<Arc<vnilink::RemoteDesktopService>> {
        let state = self.state.lock().unwrap();
        if state.0 {
            return None;
        }
        state
            .2
            .as_ref()
            .filter(|owner| !owner.is_cancelled())
            .cloned()
    }
    fn close(&self) {
        let (info, owner) = {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            (state.1.take(), state.2.take())
        };
        if let Some(owner) = owner {
            owner.close();
        }
        if let Some(info) = info {
            let _ = self.core.close_controller_service(info.id, info.generation);
            let _ = self.core.forget_controller_service(info.id);
        }
    }
}
impl Drop for DesktopBackend {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) struct Viewer {
    pub window: adw::ApplicationWindow,
    pub content: adw::ToastOverlay,
    pub picture: gtk::Picture,
    pub status: gtk::Label,
    stack: gtk::Stack,
    stop: Arc<AtomicBool>,
    pub session: SessionInfo,
    desktop: Arc<DesktopBackend>,
    size: Rc<Cell<(u16, u16)>>,
    frame_source: Rc<Cell<(u64, u64)>>,
    release_input: RefCell<Option<Rc<dyn Fn()>>>,
    active_handler: RefCell<Option<glib::SignalHandlerId>>,
}

impl Viewer {
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
        let window = parent.clone();
        let view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
        header.set_title_widget(Some(&adw::WindowTitle::new(&title, "Remote desktop")));
        let tools = gtk::MenuButton::builder().label("Session").build();
        let desktop = Arc::new(DesktopBackend::new(core.clone()));
        tools.set_popover(Some(&session_popover(
            &window,
            core.clone(),
            &session,
            desktop.clone(),
        )));
        header.pack_start(&tools);
        let files_window = gtk::Button::with_label("Files…");
        files_window.set_tooltip_text(Some("Browse both computers and copy files either way"));
        let win = window.clone();
        let worker = core.clone();
        let peer = session.peer_id.clone();
        files_window.connect_clicked(move |_| {
            super::filemanager::FileManager::open(
                &win,
                worker.clone(),
                peer.clone(),
                "This computer".into(),
                crate::presentation::computer_name(&peer),
            );
        });
        header.pack_start(&files_window);
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

        super::files::install_drop(
            &picture,
            &window,
            core.clone(),
            session.id,
            session.generation,
        );

        let viewer = Rc::new(Self {
            window,
            content: toasts.clone(),
            picture,
            status,
            stack,
            stop: Arc::new(AtomicBool::new(false)),
            session,
            desktop,
            size: Rc::new(Cell::new((0, 0))),
            frame_source: Rc::new(Cell::new((0, 0))),
            release_input: RefCell::new(None),
            active_handler: RefCell::new(None),
        });
        viewer.install_input();
        let weak = Rc::downgrade(&viewer);
        let disconnect_core = core.clone();
        disconnect.connect_clicked(move |_| {
            if let Some(viewer) = weak.upgrade() {
                let _ = core_disconnect(&viewer, &disconnect_core);
            }
        });
        viewer
    }

    pub(crate) fn desktop_owner(&self) -> Option<Arc<vnilink::RemoteDesktopService>> {
        self.desktop.current()
    }
    pub(crate) fn is_retired(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    pub(crate) fn deactivate(&self) {
        if let Some(release) = self.release_input.borrow().as_ref() {
            release();
        }
    }
    pub(crate) fn retire(&self) {
        self.deactivate();
        if let Some(handler) = self.active_handler.borrow_mut().take() {
            self.window.disconnect(handler);
        }
        self.stop.store(true, Ordering::Relaxed);
        self.desktop.close();
        self.picture.set_paintable(None::<&gtk::gdk::Paintable>);
    }

    pub(crate) fn show_frame(&self, frame: &DecodedFrame) {
        // X11's BGRx and decoded BGRA may have a zero fourth byte. The remote
        // desktop is opaque, so normalize alpha rather than painting it transparent.
        let picture = &frame.frame;
        let mut pixels = picture.pixels().to_vec();
        for row in pixels
            .chunks_mut(picture.stride())
            .take(usize::from(picture.height()))
        {
            for pixel in row[..usize::from(picture.width()) * 4].chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        let bytes = glib::Bytes::from_owned(pixels);
        let texture = gtk::gdk::MemoryTexture::new(
            i32::from(picture.width()),
            i32::from(picture.height()),
            gtk::gdk::MemoryFormat::B8g8r8a8,
            &bytes,
            picture.stride(),
        );
        self.picture.set_paintable(Some(&texture));
        self.size.set((picture.width(), picture.height()));
        self.frame_source.set((frame.display_id, frame.revision));
        self.stack.set_visible_child_name("desktop");
    }

    pub(crate) fn start(self: &Rc<Self>, core: Arc<Core>) {
        let latest: Arc<Mutex<Option<Result<DecodedFrame, String>>>> = Arc::new(Mutex::new(None));
        let delivery = latest.clone();
        let stop = self.stop.clone();
        let session = self.session.clone();
        let worker = core.clone();
        let desktop = self.desktop.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let owner = desktop.open(&session).map_err(|e| e.to_string())?;
                owner.acquire_control().map_err(|e| e.to_string())?;
                // A reconnect's previous decoder may still be releasing its subscription.
                let mut subscription = None;
                for _ in 0..20 {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    match owner.subscribe_video() {
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
                    let current = worker
                        .controller_session(session.id)
                        .map_err(|e| e.to_string())?;
                    if !Some(current).is_some_and(|s| {
                        s.id == session.id
                            && s.generation == session.generation
                            && s.phase == SessionPhase::Active
                    }) {
                        return Err("The session ended or is reconnecting.".into());
                    }
                    let Some(frame) = subscription.recv_timeout(Duration::from_millis(100)) else {
                        continue;
                    };
                    let began = std::time::Instant::now();
                    match controller.push(&frame).map_err(|e| e.to_string())? {
                        ControllerOutput::Decoded(frame) => {
                            let _ = owner.report_decode_duration(
                                began.elapsed().as_micros().min(5_000_000) as u64,
                            );
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
            if let Ok(Some(cursor)) = viewer
                .desktop
                .current()
                .ok_or(vnilink::Error::NoSession)
                .and_then(|owner| owner.cursor())
            {
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
                            frame.frame.width(),
                            frame.frame.height()
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

    fn install_input(&self) {
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
        let stop = self.stop.clone();
        let desktop = self.desktop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let command = match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(c) => c,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let Some(owner) = desktop.current() else {
                    continue;
                };
                if owner.control_state().ok() != Some(vnilink::DesktopControlState::Controlling) {
                    continue;
                }
                let result = match command {
                    Command::Event(event) => owner.submit_input(event),
                    Command::Paste(usage, text, clipboard) => {
                        if let Ok(Some(copy)) = clipboard.recv_timeout(Duration::from_millis(500)) {
                            // Apply the clipboard before the shortcut, on the same
                            // ordered input worker; UI and video remain responsive.
                            if let Err(e) = owner.submit_clipboard(&copy) {
                                let _ = notices
                                    .try_send(format!("Clipboard paste could not sync: {e}"));
                            }
                        }
                        owner.submit_local_key(LocalKey {
                            usage,
                            down: true,
                            text: &text,
                            composing: false,
                        })
                    }
                    Command::Key(usage, down, text) => owner.submit_local_key(LocalKey {
                        usage,
                        down,
                        text: &text,
                        composing: false,
                    }),
                };
                if let Err(error) = result {
                    let _ = notices.try_send(error.to_string());
                }
            }
        });
        let fail_stop = self.stop.clone();
        let status = self.status.clone();
        let overflow_desktop = self.desktop.clone();
        let enqueue: Rc<dyn Fn(Command)> = Rc::new(move |command| {
            if sender.try_send(command).is_err() {
                fail_stop.store(true, Ordering::Relaxed);
                status.set_label("Input stopped. Disconnect and reconnect to continue.");
                let desktop = overflow_desktop.clone();
                tasks::run(
                    move || {
                        desktop.close();
                        Ok(())
                    },
                    |_| {},
                );
            }
        });
        let send = enqueue.clone();
        let pointer_desktop = self.desktop.clone();
        let chosen = Rc::new(Cell::new(0u64));
        let sent_revision = Rc::new(Cell::new(0u64));
        // Fresh source per input event: the picture fraction maps onto
        // the displayed source's extent, never onto the handshake
        // monitor. Motion and clicks share one resolver over the same
        // cells, and each event resolves once, so scaling, selection
        // and placement always agree on one topology entry.
        let extent_desktop = self.desktop.clone();
        let extent_chosen = Rc::clone(&chosen);
        let extent_source = Rc::clone(&self.frame_source);
        let resolve: Rc<dyn Fn() -> Option<vnilink::DisplayInfo>> = Rc::new(move || {
            resolve_pointer_display(&extent_desktop, &extent_chosen, extent_source.get())
        });
        let motion_chosen = Rc::clone(&chosen);
        let motion_revision = Rc::clone(&sent_revision);
        install_pointer_motion(
            &self.picture,
            self.size.clone(),
            Rc::clone(&resolve),
            move |x, y, display| {
                if let Some(event) = desktop_pointer(
                    &pointer_desktop,
                    display,
                    x,
                    y,
                    &motion_chosen,
                    &motion_revision,
                ) {
                    send(Command::Event(event));
                }
            },
        );
        let held_buttons = Rc::new(RefCell::new(BTreeSet::new()));
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let picture = self.picture.downgrade();
        let size = self.size.clone();
        let send = enqueue.clone();
        let held = held_buttons.clone();
        let click_desktop = self.desktop.clone();
        let click_chosen = Rc::clone(&chosen);
        let click_revision = Rc::clone(&sent_revision);
        let click_resolve = Rc::clone(&resolve);
        click.connect_pressed(move |gesture, _, x, y| {
            let Some(picture) = picture.upgrade() else {
                return;
            };
            let Some(display) = click_resolve() else {
                return;
            };
            let Some((x, y)) = vnilink_linux::pointer(
                x,
                y,
                (f64::from(picture.width()), f64::from(picture.height())),
                size.get(),
                (display.width, display.height),
            ) else {
                return;
            };
            picture.grab_focus();
            if let Some(event) = desktop_pointer(
                &click_desktop,
                display,
                x,
                y,
                &click_chosen,
                &click_revision,
            ) {
                send(Command::Event(event));
            }
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
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let send = enqueue.clone();
        let picture = self.picture.downgrade();
        let scroll_remainder = Cell::new((0.0, 0.0));
        scroll.connect_scroll(move |scroll, dx, dy| {
            let Some(picture) = picture.upgrade() else {
                return Propagation::Proceed;
            };
            if picture.has_focus() {
                if dx.is_finite() && dy.is_finite() {
                    let factor = if scroll.unit() == gtk::gdk::ScrollUnit::Wheel {
                        120.0
                    } else {
                        12.0
                    };
                    let old = scroll_remainder.get();
                    let limit = f64::from(vnilink::MAX_PRECISE_WHEEL);
                    let x = (dx * factor + old.0).clamp(-limit, limit);
                    let y = (dy * factor + old.1).clamp(-limit, limit);
                    let ix = x.trunc() as i32;
                    let iy = y.trunc() as i32;
                    scroll_remainder.set((x - f64::from(ix), y - f64::from(iy)));
                    if ix != 0 || iy != 0 {
                        send(Command::Event(InputEvent::precise_wheel(ix, iy)));
                    }
                }
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
        self.release_input.replace(Some(release.clone()));
        let handler = self.window.connect_is_active_notify(move |window| {
            if !window.is_active() {
                release();
            }
        });
        self.active_handler.replace(Some(handler));
        self.picture.add_controller(focus);
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        if let Some(handler) = self.active_handler.get_mut().take() {
            self.window.disconnect(handler);
        }
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn pointer_button(button: u32) -> Option<PointerButton> {
    match button {
        1 => Some(PointerButton::Left),
        2 => Some(PointerButton::Middle),
        3 => Some(PointerButton::Right),
        8 => Some(PointerButton::Back),
        9 => Some(PointerButton::Forward),
        _ => None,
    }
}

fn session_popover(
    window: &adw::ApplicationWindow,
    core: Arc<Core>,
    session: &SessionInfo,
    desktop: Arc<DesktopBackend>,
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
            move || {
                worker
                    .controller_session_paths(id)
                    .map_err(|e| e.to_string())
            },
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
    let text = gtk::Button::with_label("Send text…");
    content.append(&text);
    let desktop = desktop.clone();
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
        let desktop = desktop.clone();
        let win = win.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "send" {
                let text = entry.text().to_string();
                let desktop = desktop.clone();
                let win = win.clone();
                tasks::run(
                    move || {
                        desktop
                            .current()
                            .ok_or_else(|| vnilink::Error::NoSession.to_string())?
                            .submit_text(&text)
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

fn core_disconnect(viewer: &Viewer, core: &Core) -> Result<(), vnilink::Error> {
    let _ = core;
    viewer.retire();
    viewer.status.set_label("Disconnected");
    Ok(())
}

/// Resolve the live topology entry the pointer acts on. A stamped frame
/// names its exact live source: id and revision must both match, or the
/// held picture is stale and the pointer is fenced until matching
/// pixels arrive. The remembered preference never overrides an attested
/// displayed source. Only unstamped whole-desktop pictures fall back to
/// preferred-or-unique matching, and an ambiguous frame sends nothing.
/// Each input event resolves once; scaling, selection and placement
/// share the returned entry, so a topology change between events can
/// only drop one event, never misdirect it.
fn resolve_pointer_display(
    backend: &DesktopBackend,
    chosen: &Cell<u64>,
    frame_source: (u64, u64),
) -> Option<vnilink::DisplayInfo> {
    let desktop = backend.current()?;
    let displays = desktop.topology().ok()?;
    let (frame_id, frame_revision) = frame_source;
    if frame_id != 0 {
        vnilink_linux::display_for_frame_id(&displays, frame_id, frame_revision)
    } else {
        let preferred = match chosen.get() {
            0 => None,
            id => Some(id),
        };
        vnilink_linux::display_for_picture(&displays, preferred).or_else(|| {
            // No preferred id yet, or it left the topology: fall back to
            // the unique geometry match instead of stranding the session
            // with an id that can never become chosen.
            chosen.set(0);
            let info = desktop.desktop_info().ok()?;
            vnilink_linux::display_for_frame(&displays, (info.monitor_width, info.monitor_height))
        })
    }
}

/// Map source pixels onto the resolved display and select it once per
/// revision. The point was already scaled to the display's extent from
/// the same resolution, so placement only bounds-checks and adds the
/// origin. Selection routes through a fresh service handle: the handle
/// is stable across topology updates, while the entry snapshot stays
/// the resolved one.
fn desktop_pointer(
    backend: &DesktopBackend,
    display: vnilink::DisplayInfo,
    x: u16,
    y: u16,
    chosen: &Cell<u64>,
    sent_revision: &Cell<u64>,
) -> Option<InputEvent> {
    if sent_revision.get() != display.revision {
        backend.current()?.select_displays(&[display.id]).ok()?;
        sent_revision.set(display.revision);
        chosen.set(display.id);
    }
    let (x, y) = vnilink_linux::place_on_display(x, y, display)?;
    Some(InputEvent::pointer(x, y, display.id, display.revision))
}

/// Mouse motion belongs to the image under the pointer; keyboard focus is
/// acquired by clicking and is handled separately by the key controller.
/// The resolver names the displayed source fresh per motion event; its
/// extent scales the picture fraction, and the resolved entry travels
/// with the point so selection and placement agree on one snapshot.
pub(super) fn install_pointer_motion(
    picture: &gtk::Picture,
    size: Rc<Cell<(u16, u16)>>,
    resolve: Rc<dyn Fn() -> Option<vnilink::DisplayInfo>>,
    send: impl Fn(u16, u16, vnilink::DisplayInfo) + 'static,
) -> gtk::EventControllerMotion {
    let picture_for_controller = picture;
    let motion = gtk::EventControllerMotion::new();
    let picture = picture.downgrade();
    let last_position = Cell::new(None);
    motion.connect_motion(move |_, x, y| {
        let Some(picture) = picture.upgrade() else {
            return;
        };
        let Some(display) = resolve() else {
            return;
        };
        if let Some((x, y)) = vnilink_linux::pointer(
            x,
            y,
            (f64::from(picture.width()), f64::from(picture.height())),
            size.get(),
            (display.width, display.height),
        ) {
            // Replacing the paintable can cause GTK to emit motion without
            // hardware movement. Resending it fights the host's local mouse.
            if last_position.replace(Some((x, y))) != Some((x, y)) {
                send(x, y, display);
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
