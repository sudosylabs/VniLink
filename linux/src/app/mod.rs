//! Opens the keyring, then the window.
//!
//! The core calls the store on this process's threads, outside the runtime
//! it creates. The open itself runs on a plain thread so the spinner can
//! turn. A later save from the window thread is also outside that runtime.

use adw::prelude::*;
mod background;
mod capture;
mod clipboard;
mod dialogs;
mod filemanager;
mod files;
mod notifications;
mod pages;
mod preferences;
mod service_files;
mod session_tabs;
mod sidebar;
mod tasks;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
mod viewer;
mod widgets;
mod window;

pub(crate) use window::{Actions, Home};

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gtk::glib::ControlFlow;
use vnilink::{Core, OpenConfig, PairingPreview, SessionPhase};

use crate::error::open_notice;
use crate::events::LiveState;
use crate::pairing::PairingFlow;
use crate::storage::SecretStore;

pub fn run() -> gtk::glib::ExitCode {
    application().run()
}

fn application() -> adw::Application {
    let application = adw::Application::builder()
        .application_id("com.vnilink.VniLink")
        .flags(gtk::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    application.add_main_option(
        "background",
        0u8.into(),
        gtk::glib::OptionFlags::NONE,
        gtk::glib::OptionArg::None,
        "Start without opening the main window",
        None,
    );
    application.connect_startup(|app| {
        install_style();
        let hold = Rc::new(RefCell::new(Some(app.hold())));
        app.connect_shutdown(move |_| {
            hold.borrow_mut().take();
        });
        for name in ["open", "quit", "tray-unavailable"] {
            let action = gtk::gio::SimpleAction::new(name, None);
            let weak = app.downgrade();
            action.connect_activate(move |action, _| {
                if let Some(app) = weak.upgrade() {
                    match action.name().as_str() {
                        "quit" => app.quit(),
                        // A login launch must stay reachable when no tray host exists.
                        _ => app.activate(),
                    }
                }
            });
            app.add_action(&action);
        }
        background::install(app);
    });
    application.connect_command_line(|app, command| {
        let hidden = command
            .options_dict()
            .lookup::<bool>("background")
            .ok()
            .flatten()
            .unwrap_or(false);
        let first = app.windows().is_empty();
        app.activate();
        if first && hidden && background::keep_running() {
            for window in app.windows() {
                window.set_visible(false);
            }
        }
        0.into()
    });
    application.connect_activate(present);
    application
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
    if let Some(window) = application
        .windows()
        .into_iter()
        .find(|w| w.widget_name() == "vnilink-main")
    {
        window.present();
        return;
    }
    let builder = gtk::Builder::from_string(include_str!("window.ui"));
    let window: adw::ApplicationWindow = widgets::widget(&builder, "window");
    window.set_application(Some(application));
    window.set_widget_name("vnilink-main");

    let stack: gtk::Stack = widgets::widget(&builder, "root");
    let spinner: gtk::Spinner = widgets::widget(&builder, "spinner");
    let failure: adw::StatusPage = widgets::widget(&builder, "failure");
    let retry: gtk::Button = widgets::widget(&builder, "retry");
    let toasts: adw::ToastOverlay = widgets::widget(&builder, "toasts");

    let opening = Rc::new(Cell::new(false));
    let closed = Rc::new(Cell::new(false));
    let home_slot: Rc<RefCell<Option<Rc<Home>>>> = Rc::new(RefCell::new(None));
    let core_slot: Rc<RefCell<Option<Arc<Core>>>> = Rc::new(RefCell::new(None));
    for id in ["settings", "settings_sidebar"] {
        let settings_button: gtk::Button = widgets::widget(&builder, id);
        settings_button.connect_clicked({
            let core_slot = core_slot.clone();
            let window = window.clone();
            move |_| {
                if let Some(core) = core_slot.borrow().clone() {
                    preferences::open(&window, core);
                }
            }
        });
    }
    let pending: Rc<RefCell<Option<PairingPreview>>> = Rc::new(RefCell::new(None));
    let saving = Rc::new(Cell::new(false));
    let tick: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));

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
            gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
                let opened = match receiver.try_recv() {
                    Ok(opened) => opened,
                    Err(std::sync::mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return ControlFlow::Break,
                };
                opening.set(false);
                spinner.stop();
                if closed.get() {
                    if let Ok((core, _)) = opened {
                        core.shutdown();
                    }
                    return ControlFlow::Break;
                }
                match opened {
                    Ok((core, file_warning)) => {
                        let core = Arc::new(core);
                        // Transfers progress without any Files window open.
                        core.start_job_driver();
                        if let Some(error) = file_warning {
                            notifications::note(&toasts, &error);
                        }
                        core_slot.borrow_mut().replace(Arc::clone(&core));
                        let tabs = session_tabs::SessionTabs::install(&window, core.clone());
                        clipboard::install_for_tabs(&window, core.clone(), tabs.selected.clone());
                        if background::resume_x11() && std::env::var_os("WAYLAND_DISPLAY").is_none()
                        {
                            capture::resume(&window, core.clone());
                        }
                        if home_slot.borrow().is_none() {
                            let flow = Rc::new(RefCell::new(PairingFlow::default()));
                            let live = Rc::new(RefCell::new(LiveState::default()));
                            let home = Home::from_builder(
                                &builder,
                                actions(
                                    Arc::clone(&core),
                                    window.clone(),
                                    Rc::clone(&home_slot),
                                    toasts.clone(),
                                    Rc::clone(&pending),
                                    Rc::clone(&saving),
                                    Rc::clone(&flow),
                                    Rc::clone(&live),
                                    tabs.clone(),
                                ),
                                flow,
                                live,
                            );
                            home_slot.borrow_mut().replace(Rc::clone(&home));
                            let core_watch = Arc::clone(&core);
                            let home_watch = Rc::clone(&home_slot);
                            let toasts_watch = toasts.clone();
                            *tick.borrow_mut() = Some(gtk::glib::timeout_add_local(
                                Duration::from_millis(400),
                                move || {
                                    if let Ok(snapshot) = core_watch.snapshot() {
                                        background::update_status(&snapshot);
                                    }
                                    paint(&core_watch, &home_watch, &toasts_watch);
                                    if let Some(error) = capture::take_error() {
                                        notifications::note(&toasts_watch, &error);
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

    for name in ["settings", "stop-hosting"] {
        let action = gtk::gio::SimpleAction::new(name, None);
        let slot = core_slot.clone();
        let win = window.clone();
        action.connect_activate(move |action, _| {
            if let Some(core) = slot.borrow().clone() {
                if action.name() == "settings" {
                    win.present();
                    preferences::open(&win, core);
                } else {
                    capture::stop();
                    let win = win.clone();
                    tasks::run(
                        move || {
                            core.stop_hosting().map_err(|e| e.to_string())?;
                            core.video_codecs()
                                .map(|mut caps| {
                                    caps.encode_baseline = false;
                                    caps.encode_high = false;
                                    caps
                                })
                                .and_then(|caps| core.set_video_codecs(caps))
                                .map_err(|e| e.to_string())
                        },
                        move |result| {
                            if let Err(error) = result {
                                preferences::report(&win, &error);
                            }
                        },
                    );
                }
            }
        });
        application.add_action(&action);
    }
    let shutdown_slot = core_slot.clone();
    let shutdown_tick = tick.clone();
    let shutting_down = closed.clone();
    application.connect_shutdown(move |_| {
        shutting_down.set(true);
        capture::stop();
        if let Some(source) = shutdown_tick.borrow_mut().take() {
            source.remove();
        }
        if let Some(core) = shutdown_slot.borrow_mut().take() {
            core.shutdown();
        }
    });
    let app = application.clone();
    let window_close = window.clone();
    window_close.connect_close_request(move |window| {
        if background::keep_running() {
            window.set_visible(false);
            return gtk::glib::Propagation::Stop;
        }
        closed.set(true);
        if let Some(source) = tick.borrow_mut().take() {
            source.remove();
        }
        if let Some(core) = core_slot.borrow().clone() {
            core.shutdown();
        }
        app.quit();
        gtk::glib::Propagation::Proceed
    });
    stack.set_visible_child_name("startup");
    window.present();
    begin();
}

struct Notice {
    title: String,
    message: String,
}

fn open_core() -> Result<(Core, Option<String>), Notice> {
    let store = SecretStore::connect().map_err(|err| {
        let notice = open_notice(&err);
        Notice {
            title: notice.title,
            message: notice.message,
        }
    })?;
    let core = Core::open(Arc::new(store), OpenConfig::default()).map_err(|err| {
        let notice = open_notice(&err);
        Notice {
            title: notice.title,
            message: notice.message,
        }
    })?;
    // Report VA-API availability explicitly. Without this a machine without a
    // render node would admit a session that can never carry video.
    #[cfg(target_os = "linux")]
    let _ = core.set_video_codecs(vnilink_linux::video_codec_caps());
    #[cfg(not(target_os = "linux"))]
    let _ = core.set_video_codecs(vnilink::VideoCodecCaps::default());
    #[cfg(target_os = "linux")]
    let _ = core.set_power_factory(Some(Arc::new(vnilink_linux::LinuxPowerFactory)));
    let _ = core.set_cursor_receiver(true);
    let file_warning = files::initialize(&core)
        .err()
        .map(|e| format!("File reception is unavailable: {e}"));
    Ok((core, file_warning))
}

fn actions(
    core: Arc<Core>,
    window: adw::ApplicationWindow,
    home_slot: Rc<RefCell<Option<Rc<Home>>>>,
    toasts: adw::ToastOverlay,
    pending: Rc<RefCell<Option<PairingPreview>>>,
    saving: Rc<Cell<bool>>,
    flow: Rc<RefCell<PairingFlow>>,
    _live: Rc<RefCell<LiveState>>,
    tabs: Rc<session_tabs::SessionTabs>,
) -> Actions {
    Actions {
        hosting: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move |active| {
                let core = Arc::clone(&core);
                let home_slot = Rc::clone(&home_slot);
                let toasts = toasts.clone();
                // The switch is still in its notify. Rebuild the page after it returns.
                gtk::glib::idle_add_local(move || {
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
                        notifications::failed(&toasts, &err);
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
                    Ok(snapshot)
                        if snapshot.incoming.as_ref().is_some_and(|session| {
                            !matches!(
                                session.phase,
                                vnilink::SessionPhase::Closed | vnilink::SessionPhase::Failed(_)
                            )
                        }) =>
                    {
                        core.stop_hosted_session()
                    }
                    Ok(_) => core.disconnect(),
                    Err(err) => Err(err),
                };
                if let Err(err) = result {
                    notifications::failed(&toasts, &err);
                }
                paint(&core, &home_slot, &toasts);
            })
        },
        create_file: {
            let core = Arc::clone(&core);
            let window = window.clone();
            let toasts = toasts.clone();
            let home_slot = Rc::clone(&home_slot);
            let flow = Rc::clone(&flow);
            let saving = Rc::clone(&saving);
            Rc::new(move || {
                dialogs::create_pairing_file(
                    Arc::clone(&core),
                    window.clone(),
                    toasts.clone(),
                    Rc::new({
                        let home_slot = Rc::clone(&home_slot);
                        let flow = Rc::clone(&flow);
                        move |check: String| {
                            flow.borrow_mut().offer(check.clone());
                            if let Some(home) = home_slot.borrow().clone() {
                                home.show_offer(&check);
                            }
                        }
                    }),
                    Rc::new({
                        let core = Arc::clone(&core);
                        let home_slot = Rc::clone(&home_slot);
                        let toasts = toasts.clone();
                        move || paint(&core, &home_slot, &toasts)
                    }),
                    Rc::new({
                        let toasts = toasts.clone();
                        move |err: &vnilink::Error| notifications::failed(&toasts, err)
                    }),
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
            let flow = Rc::clone(&flow);
            let saving = Rc::clone(&saving);
            Rc::new(move || {
                dialogs::open_pairing_file(
                    Arc::clone(&core),
                    window.clone(),
                    toasts.clone(),
                    Rc::new({
                        let home_slot = Rc::clone(&home_slot);
                        let pending = Rc::clone(&pending);
                        let flow = Rc::clone(&flow);
                        move |preview: PairingPreview| {
                            flow.borrow_mut()
                                .open_file(preview.endpoint_id.clone(), preview.check.clone());
                            if let Some(home) = home_slot.borrow().clone() {
                                home.show_file(&preview.endpoint_id, &preview.check);
                                home.root.set_show_content(true);
                            }
                            pending.replace(Some(preview));
                        }
                    }),
                    Rc::new({
                        let toasts = toasts.clone();
                        move |err: &vnilink::Error| notifications::failed(&toasts, err)
                    }),
                    Rc::clone(&saving),
                )
            })
        },
        save_computer: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            let pending = Rc::clone(&pending);
            let flow = Rc::clone(&flow);
            Rc::new(move |digits| {
                let Some(preview) = pending.borrow().clone() else {
                    return;
                };
                match core.confirm_pairing(&preview, &digits) {
                    Ok(device) => {
                        pending.borrow_mut().take();
                        flow.borrow_mut().clear_file();
                        if let Some(home) = home_slot.borrow().clone() {
                            home.select(Some(device.endpoint_id));
                            home.clear_file();
                        }
                        paint(&core, &home_slot, &toasts);
                    }
                    Err(err) => notifications::failed(&toasts, &err),
                }
            })
        },
        cancel_file: {
            let home_slot = Rc::clone(&home_slot);
            let pending = Rc::clone(&pending);
            let flow = Rc::clone(&flow);
            Rc::new(move || {
                pending.borrow_mut().take();
                flow.borrow_mut().clear_file();
                if let Some(home) = home_slot.borrow().clone() {
                    home.clear_file();
                }
            })
        },
        connect: {
            let tabs = tabs.clone();
            let toasts = toasts.clone();
            Rc::new(move |endpoint_id| {
                if let Err(error) = tabs.connect(&endpoint_id) {
                    notifications::failed(&toasts, &error);
                }
            })
        },
        files: {
            let core = Arc::clone(&core);
            let window = window.clone();
            let toasts = toasts.clone();
            Rc::new(move |endpoint_id| {
                // Files without the desktop viewer and without host screen
                // sharing: reuse the live control session when it already
                // targets this peer, otherwise establish one for file roots,
                // listings and transfers only (no video decoder required).
                let peer = endpoint_id.clone();
                let worker = Arc::clone(&core);
                let win = window.clone();
                let toasts = toasts.clone();
                let core_open = Arc::clone(&core);
                tasks::run(
                    move || {
                        if let Ok(sessions) = worker.controller_sessions() {
                            if let Some(session) = sessions.iter().find(|s| {
                                s.peer_id == peer
                                    && !matches!(
                                        s.phase,
                                        SessionPhase::Closed | SessionPhase::Failed(_)
                                    )
                            }) {
                                let _ = session;
                                return Ok(false);
                            }
                        }
                        worker
                            .begin_connect_services(&peer)
                            .map(|_| true)
                            .map_err(|e| e.to_string())
                    },
                    move |result| match result {
                        Ok(_) => {
                            let name = core_open
                                .snapshot()
                                .ok()
                                .and_then(|s| {
                                    s.devices
                                        .iter()
                                        .find(|d| d.endpoint_id == endpoint_id)
                                        .map(|d| crate::presentation::device_name(d))
                                })
                                .unwrap_or_else(|| "Remote computer".into());
                            filemanager::FileManager::open(
                                &win,
                                core_open.clone(),
                                endpoint_id.clone(),
                                "This computer".into(),
                                name,
                            );
                        }
                        Err(error) => notifications::note(&toasts, &error),
                    },
                );
            })
        },
        allow: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move |digits| match core.confirm_control_request(&digits) {
                Ok(_) => paint(&core, &home_slot, &toasts),
                Err(err) => notifications::failed(&toasts, &err),
            })
        },
        refuse: {
            let core = Arc::clone(&core);
            let home_slot = Rc::clone(&home_slot);
            let toasts = toasts.clone();
            Rc::new(move || {
                if let Err(err) = core.reject_control_request() {
                    notifications::failed(&toasts, &err);
                }
                paint(&core, &home_slot, &toasts);
            })
        },
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
            // A local decode check mirrors the dial-time gate so the banner
            // never promises video this computer cannot show.
            home.set_waiting(waiting);
            home.set_request(core.control_request().ok().flatten());
            home.show(&snapshot);
        }
        Err(err) => notifications::failed(toasts, &err),
    }
}
