//! Opens the keyring, then the window.
//!
//! The core calls the store on this process's threads, outside the runtime
//! it creates. The open itself runs on a plain thread so the spinner can
//! turn. A later save from the window thread is also outside that runtime.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib::{self, ControlFlow};
use vnilink::{Core, OpenConfig, PairingPreview, SessionPhase, PAIRING_FILE_EXTENSION};

use crate::home::{self, Home};
use crate::secret::SecretStore;

pub fn run() -> glib::ExitCode {
    let application = adw::Application::builder()
        .application_id("com.vnilink.VniLink")
        .build();
    application.connect_startup(|_| install_style());
    application.connect_activate(present);
    application.run()
}

fn install_style() {
    let provider = gtk::CssProvider::new();
    // GTK 4.12 replaces this. The application still builds against 4.10.
    #[allow(deprecated)]
    provider.load_from_data(include_str!("style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn present(application: &adw::Application) {
    let builder = gtk::Builder::from_string(include_str!("window.ui"));
    let window: adw::ApplicationWindow = home::widget(&builder, "window");
    window.set_application(Some(application));
    let split: adw::NavigationSplitView = home::widget(&builder, "split");
    let breakpoint = adw::Breakpoint::new(
        adw::BreakpointCondition::parse("max-width: 700sp").expect("breakpoint"),
    );
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);

    let stack: gtk::Stack = home::widget(&builder, "root");
    let spinner: gtk::Spinner = home::widget(&builder, "spinner");
    let failure: adw::StatusPage = home::widget(&builder, "failure");
    let retry: gtk::Button = home::widget(&builder, "retry");
    let toasts: adw::ToastOverlay = home::widget(&builder, "toasts");

    let opening = Rc::new(Cell::new(false));
    let closed = Rc::new(Cell::new(false));
    let home_slot: Rc<RefCell<Option<Rc<Home>>>> = Rc::new(RefCell::new(None));
    let core_slot: Rc<RefCell<Option<Arc<Core>>>> = Rc::new(RefCell::new(None));
    let pending: Rc<RefCell<Option<PairingPreview>>> = Rc::new(RefCell::new(None));
    let saving = Rc::new(Cell::new(false));
    let tick: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let begin: Rc<dyn Fn()> = Rc::new({
        let opening = Rc::clone(&opening);
        let closed = Rc::clone(&closed);
        let home_slot = Rc::clone(&home_slot);
        let core_slot = Rc::clone(&core_slot);
        let tick = Rc::clone(&tick);
        let stack = stack.clone();
        let spinner = spinner.clone();
        let failure = failure.clone();
        let toasts = toasts.clone();
        let builder = builder.clone();
        let window = window.clone();
        let pending = Rc::clone(&pending);
        let saving = Rc::clone(&saving);
        move || {
            if opening.get() || closed.get() {
                return;
            }
            opening.set(true);
            spinner.start();
            stack.set_visible_child_name("startup");
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(open_core());
            });
            let opening = Rc::clone(&opening);
            let closed = Rc::clone(&closed);
            let home_slot = Rc::clone(&home_slot);
            let core_slot = Rc::clone(&core_slot);
            let tick = Rc::clone(&tick);
            let stack = stack.clone();
            let spinner = spinner.clone();
            let failure = failure.clone();
            let toasts = toasts.clone();
            let builder = builder.clone();
            let window = window.clone();
            let pending = Rc::clone(&pending);
            let saving = Rc::clone(&saving);
            // The worker cannot touch the window. Poll until the record is open.
            glib::timeout_add_local(Duration::from_millis(50), move || {
                let opened = match receiver.try_recv() {
                    Ok(opened) => opened,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return ControlFlow::Break,
                };
                opening.set(false);
                spinner.stop();
                if closed.get() {
                    if let Ok(core) = opened {
                        core.shutdown();
                    }
                    return ControlFlow::Break;
                }
                match opened {
                    Ok(core) => {
                        let core = Arc::new(core);
                        core_slot.borrow_mut().replace(Arc::clone(&core));
                        if home_slot.borrow().is_none() {
                            let home = Home::from_builder(
                                &builder,
                                actions(
                                    Arc::clone(&core),
                                    window.clone(),
                                    Rc::clone(&home_slot),
                                    toasts.clone(),
                                    Rc::clone(&pending),
                                    Rc::clone(&saving),
                                ),
                            );
                            home_slot.borrow_mut().replace(Rc::clone(&home));
                            let events = match core.subscribe() {
                                Ok(events) => Some(events),
                                Err(err) => {
                                    toasts.add_toast(adw::Toast::new(&err.to_string()));
                                    None
                                }
                            };
                            let core_watch = Arc::clone(&core);
                            let home_watch = Rc::clone(&home_slot);
                            let toasts_watch = toasts.clone();
                            let lagged = Cell::new(0);
                            *tick.borrow_mut() = Some(glib::timeout_add_local(
                                Duration::from_millis(400),
                                move || {
                                    let mut changed = false;
                                    if let Some(events) = events.as_ref() {
                                        while events.try_recv().is_some() {
                                            changed = true;
                                        }
                                        let behind = events.lagged();
                                        if behind != lagged.get() {
                                            lagged.set(behind);
                                            changed = true;
                                        }
                                    }
                                    if changed {
                                        paint(&core_watch, &home_watch, &toasts_watch);
                                    }
                                    ControlFlow::Continue
                                },
                            ));
                        }
                        paint(&core, &home_slot, &toasts);
                        stack.set_visible_child_name("home");
                    }
                    Err(notice) => {
                        failure.set_title(&notice.title);
                        failure.set_description(Some(&notice.message));
                        stack.set_visible_child_name("failure");
                    }
                }
                ControlFlow::Break
            });
        }
    });
    retry.connect_clicked({
        let begin = Rc::clone(&begin);
        move |_| begin()
    });

    let window_close = window.clone();
    window_close.connect_close_request(move |_| {
        closed.set(true);
        if let Some(source) = tick.borrow_mut().take() {
            source.remove();
        }
        if let Some(core) = core_slot.borrow().clone() {
            core.shutdown();
        }
        glib::Propagation::Proceed
    });
    stack.set_visible_child_name("startup");
    window.present();
    begin();
}

struct Notice {
    title: String,
    message: String,
}

fn open_core() -> Result<Core, Notice> {
    let store = SecretStore::connect().map_err(|err| notice(&err))?;
    let core = Core::open(Arc::new(store), OpenConfig::default()).map_err(|err| notice(&err))?;
    // Report VA-API availability explicitly. Without this a machine without a
    // render node would admit a session that can never carry video.
    #[cfg(target_os = "linux")]
    let _ = core.set_h264_support(
        vnilink_linux::h264_encode_available(),
        vnilink_linux::h264_decode_available(),
    );
    #[cfg(not(target_os = "linux"))]
    let _ = core.set_h264_support(false, false);
    Ok(core)
}

fn notice(error: &vnilink::Error) -> Notice {
    let (title, message) = match error {
        vnilink::Error::StoreLocked => (
            "The keyring is locked",
            "Unlock your login keyring, then try again.",
        ),
        vnilink::Error::StoreUnavailable => (
            "The keyring is not available",
            "VniLink could not open the system keyring. Check that a Secret Service such as GNOME Keyring is running, then try again. Nothing was written to a file.",
        ),
        vnilink::Error::CorruptRecord => (
            "The keyring record is not valid",
            "The installation in the keyring is not valid. VniLink left it untouched.",
        ),
        other => return Notice {
            title: "VniLink did not open".to_string(),
            message: other.to_string(),
        },
    };
    Notice {
        title: title.to_string(),
        message: message.to_string(),
    }
}

fn actions(
    core: Arc<Core>,
    window: adw::ApplicationWindow,
    home_slot: Rc<RefCell<Option<Rc<Home>>>>,
    toasts: adw::ToastOverlay,
    pending: Rc<RefCell<Option<PairingPreview>>>,
    saving: Rc<Cell<bool>>,
) -> home::Actions {
    home::Actions {
        hosting: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move |active| {
                let core = Arc::clone(&core);
                let home_slot = Rc::clone(&home_slot);
                let toasts = toasts.clone();
                // The switch is still in its notify. Rebuild the page after it returns.
                glib::idle_add_local(move || {
                    if core
                        .snapshot()
                        .ok()
                        .is_some_and(|snapshot| snapshot.hosting == active)
                    {
                        return ControlFlow::Break;
                    }
                    if let Err(err) = if active {
                        core.start_hosting()
                    } else {
                        core.stop_hosting()
                    } {
                        toasts.add_toast(adw::Toast::new(&say(&err)));
                    }
                    paint(&core, &home_slot, &toasts);
                    ControlFlow::Break
                });
            })
        },
        stop: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move || {
                let result = match core.snapshot() {
                    Ok(snapshot) if snapshot.incoming.is_some() => core.stop_hosted_session(),
                    Ok(_) => core.disconnect(),
                    Err(err) => Err(err),
                };
                if let Err(err) = result {
                    toasts.add_toast(adw::Toast::new(&say(&err)));
                }
                paint(&core, &home_slot, &toasts);
            })
        },
        create_file: {
            let core = Arc::clone(&core);
            let window = window.clone();
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            let saving = Rc::clone(&saving);
            Rc::new(move || {
                create_pairing_file(
                    Arc::clone(&core),
                    window.clone(),
                    Rc::clone(&home_slot),
                    toasts.clone(),
                    Rc::clone(&saving),
                )
            })
        },
        open_file: {
            let core = Arc::clone(&core);
            let window = window.clone();
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            let pending = Rc::clone(&pending);
            let saving = Rc::clone(&saving);
            Rc::new(move || {
                open_pairing_file(
                    Arc::clone(&core),
                    window.clone(),
                    Rc::clone(&home_slot),
                    toasts.clone(),
                    Rc::clone(&pending),
                    Rc::clone(&saving),
                )
            })
        },
        save_computer: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            let pending = Rc::clone(&pending);
            Rc::new(move |digits| {
                let Some(preview) = pending.borrow().clone() else {
                    return;
                };
                match core.confirm_pairing(&preview, &digits) {
                    Ok(device) => {
                        pending.borrow_mut().take();
                        if let Some(home) = home_slot.borrow().clone() {
                            home.select(Some(device.endpoint_id));
                            home.clear_file();
                        }
                        paint(&core, &home_slot, &toasts);
                    }
                    Err(err) => toasts.add_toast(adw::Toast::new(&say(&err))),
                }
            })
        },
        cancel_file: {
            let home_slot = Rc::clone(&home_slot);
            let pending = Rc::clone(&pending);
            Rc::new(move || {
                pending.borrow_mut().take();
                if let Some(home) = home_slot.borrow().clone() {
                    home.clear_file();
                }
            })
        },
        connect: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            let busy = Rc::new(Cell::new(false));
            Rc::new(move |endpoint_id| {
                if busy.replace(true) {
                    return;
                }
                if let Some(home) = home_slot.borrow().clone() {
                    home.set_connecting(true);
                }
                let (sender, receiver) = std::sync::mpsc::channel();
                let worker = Arc::clone(&core);
                std::thread::spawn(move || {
                    let _ = sender.send(worker.connect(&endpoint_id));
                });
                let core = Arc::clone(&core);
                let home_slot = Rc::clone(&home_slot);
                let toasts = toasts.clone();
                let busy = Rc::clone(&busy);
                glib::timeout_add_local(Duration::from_millis(50), move || {
                    let finished = match receiver.try_recv() {
                        Ok(result) => Some(result),
                        Err(std::sync::mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            Some(Err(vnilink::Error::ShutDown))
                        }
                    };
                    busy.set(false);
                    if let Some(home) = home_slot.borrow().clone() {
                        home.set_connecting(false);
                    }
                    if let Some(Err(err)) = finished {
                        toasts.add_toast(adw::Toast::new(&say(&err)));
                    }
                    paint(&core, &home_slot, &toasts);
                    ControlFlow::Break
                });
            })
        },
        allow: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move |digits| match core.confirm_control_request(&digits) {
                Ok(_) => paint(&core, &home_slot, &toasts),
                Err(err) => toasts.add_toast(adw::Toast::new(&say(&err))),
            })
        },
        refuse: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move || {
                if let Err(err) = core.reject_control_request() {
                    toasts.add_toast(adw::Toast::new(&say(&err)));
                }
                paint(&core, &home_slot, &toasts);
            })
        },
    }
}

fn create_pairing_file(
    core: Arc<Core>,
    window: adw::ApplicationWindow,
    home_slot: Rc<RefCell<Option<Rc<Home>>>>,
    toasts: adw::ToastOverlay,
    saving: Rc<Cell<bool>>,
) {
    if saving.get() {
        return;
    }
    if let Err(err) = core.start_hosting() {
        toasts.add_toast(adw::Toast::new(&say(&err)));
        return;
    }
    let file = match core.export_pairing_file(Duration::from_secs(15 * 60), true) {
        Ok(file) => file,
        Err(err) => {
            toasts.add_toast(adw::Toast::new(&say(&err)));
            return;
        }
    };
    saving.set(true);
    paint(&core, &home_slot, &toasts);
    let dialog = gtk::FileDialog::builder()
        .title("Save the pairing file")
        .initial_name("computer.vnl")
        .accept_label("Save")
        .build();
    let bytes = file.bytes;
    let check = file.check;
    glib::spawn_future_local(async move {
        let saved = dialog.save_future(Some(&window)).await;
        saving.set(false);
        match saved {
            Ok(file) => {
                let Some(path) = file.path() else {
                    toasts.add_toast(adw::Toast::new("Could not save the pairing file."));
                    return;
                };
                if write_private(&path, &bytes).is_err() {
                    toasts.add_toast(adw::Toast::new("Could not save the pairing file."));
                    return;
                }
                if let Some(home) = home_slot.borrow().clone() {
                    home.show_offer(&check);
                }
            }
            Err(error) if error.matches(gtk::DialogError::Dismissed) => {}
            Err(_) => toasts.add_toast(adw::Toast::new("Could not save the pairing file.")),
        }
    });
}

fn open_pairing_file(
    core: Arc<Core>,
    window: adw::ApplicationWindow,
    home_slot: Rc<RefCell<Option<Rc<Home>>>>,
    toasts: adw::ToastOverlay,
    pending: Rc<RefCell<Option<PairingPreview>>>,
    saving: Rc<Cell<bool>>,
) {
    if saving.get() {
        return;
    }
    saving.set(true);
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("VniLink pairing file"));
    filter.add_suffix(PAIRING_FILE_EXTENSION);
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Open a pairing file")
        .accept_label("Open")
        .default_filter(&filter)
        .filters(&filters)
        .build();
    glib::spawn_future_local(async move {
        let picked = dialog.open_future(Some(&window)).await;
        saving.set(false);
        let file = match picked {
            Ok(file) => file,
            Err(error) if error.matches(gtk::DialogError::Dismissed) => return,
            Err(_) => {
                toasts.add_toast(adw::Toast::new("Could not open that file."));
                return;
            }
        };
        let Some(path) = file.path() else {
            toasts.add_toast(adw::Toast::new("Could not open that file."));
            return;
        };
        let Some(bytes) = read_limited(&path) else {
            toasts.add_toast(adw::Toast::new("That file is not a pairing file."));
            return;
        };
        match core.open_pairing_file(&bytes) {
            Ok(preview) => {
                if let Some(home) = home_slot.borrow().clone() {
                    home.show_file(&preview.endpoint_id, &preview.check);
                    home.root.set_show_content(true);
                }
                pending.replace(Some(preview));
            }
            Err(err) => toasts.add_toast(adw::Toast::new(&say(&err))),
        }
    });
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn read_limited(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > 20_000 {
        return None;
    }
    std::fs::read(path).ok()
}

fn say(error: &vnilink::Error) -> String {
    match error {
        vnilink::Error::PairingCheckMismatch => "Those digits do not match.".to_string(),
        vnilink::Error::InvalidPairingFile => "That file is not a pairing file.".to_string(),
        vnilink::Error::InvitationExpired => "That pairing file has expired.".to_string(),
        vnilink::Error::InvitationReplayed => "That pairing file was already used.".to_string(),
        vnilink::Error::NotPaired => "Save that computer before connecting.".to_string(),
        vnilink::Error::Timeout => "That computer did not answer.".to_string(),
        vnilink::Error::NotHosting => "Allow control is off.".to_string(),
        vnilink::Error::Unauthorized => "That computer did not allow control.".to_string(),
        vnilink::Error::Busy => "A connection is already in progress.".to_string(),
        vnilink::Error::Network(_) => "Could not reach that computer.".to_string(),
        vnilink::Error::Unsupported(reason) => {
            format!("That computer cannot share its desktop ({reason}).")
        }
        other => other.to_string(),
    }
}

fn paint(core: &Core, home_slot: &RefCell<Option<Rc<Home>>>, toasts: &adw::ToastOverlay) {
    let Some(home) = home_slot.borrow().clone() else {
        return;
    };
    match core.snapshot() {
        Ok(snapshot) => {
            let waiting = snapshot.outgoing.as_ref().and_then(|session| {
                matches!(
                    session.phase,
                    SessionPhase::Connecting
                        | SessionPhase::Authenticated
                        | SessionPhase::Authorized
                        | SessionPhase::Negotiating
                        | SessionPhase::Reconnecting
                )
                .then(|| core.control_check())
            });
            home.set_waiting(waiting);
            home.set_request(core.control_request().ok().flatten());
            home.show(&snapshot);
        }
        Err(err) => toasts.add_toast(adw::Toast::new(&say(&err))),
    }
}
