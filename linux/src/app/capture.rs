//! User-selected screen capture. X11 and the Wayland permission portal remain
//! distinct; one portal grant is never treated as unattended desktop control.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vnilink_media::{FrameSource, RawFrame};

static ERROR: Mutex<Option<String>> = Mutex::new(None);
pub(crate) fn take_error() -> Option<String> {
    ERROR.lock().unwrap().take()
}
static GENERATION: AtomicU64 = AtomicU64::new(0);
static CURRENT: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
pub(crate) fn stop() {
    let mut current = CURRENT.lock().unwrap();
    GENERATION.fetch_add(1, Ordering::Relaxed);
    if let Some(stop) = current.take() {
        stop.store(true, Ordering::Relaxed);
    }
}
struct Monitor(u16, u16);
impl vnilink::HostCapture for Monitor {
    fn monitor(&self) -> (u16, u16) {
        (self.0, self.1)
    }
}
enum Source {
    X11(vnilink_linux::X11Capture),
    Portal(vnilink_linux::WaylandCapture),
}
impl FrameSource for Source {
    fn size(&self) -> (u16, u16) {
        match self {
            Self::X11(s) => s.size(),
            Self::Portal(s) => s.size(),
        }
    }
    fn next_frame(&self, time: u64) -> Result<RawFrame, vnilink_media::Error> {
        match self {
            Self::X11(s) => s.next_frame(time),
            Self::Portal(s) => s.next_frame(time),
        }
    }
}

pub(crate) fn choose(
    parent: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    button: &gtk::Button,
) {
    choose_inner(parent, core, button, false);
}
pub(crate) fn resume(parent: &impl IsA<gtk::Window>, core: Arc<vnilink::Core>) {
    // An unattended launch must never request a Wayland portal grant.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        choose_inner(parent, core, &gtk::Button::new(), true);
    }
}
fn choose_inner(
    parent: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    button: &gtk::Button,
    resume_hosting: bool,
) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    button.set_sensitive(false);
    let button = button.clone();
    let parent = parent.as_ref().clone();
    tasks::run(
        move || {
            let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
            let source = if wayland {
                Source::Portal(vnilink_linux::WaylandCapture::open().map_err(|e| e.to_string())?)
            } else {
                Source::X11(
                    vnilink_linux::X11Capture::open(&vnilink_linux::X11Plan::current_session())
                        .map_err(|e| e.to_string())?,
                )
            };
            let (width, height) = source.size();
            // Fence a pending portal/source selection against Stop and newer selections.
            let mut current = CURRENT.lock().unwrap();
            if GENERATION.load(Ordering::Relaxed) != generation {
                return Err("Screen selection was cancelled.".to_string());
            }
            core.set_capture(Box::new(Monitor(width, height)))
                .map_err(|e| e.to_string())?;
            match &source {
                Source::Portal(capture) => core
                    .set_desktop(Box::new(capture.desktop()))
                    .map_err(|e| e.to_string())?,
                Source::X11(_) => core.enable_os_input().map_err(|e| e.to_string())?,
            }
            let decode = core
                .snapshot()
                .map_err(|e| e.to_string())?
                .capabilities
                .h264_decode;
            core.set_h264_support(vnilink_linux::h264_encode_available(), decode)
                .map_err(|e| e.to_string())?;
            if resume_hosting {
                core.start_hosting().map_err(|e| e.to_string())?;
            }
            let stop = Arc::new(AtomicBool::new(false));
            if let Some(previous) = current.replace(stop.clone()) {
                previous.store(true, Ordering::Relaxed);
            }
            drop(current);
            std::thread::spawn(move || {
                let mut served = None;
                while !stop.load(Ordering::Relaxed) {
                    let Ok(snapshot) = core.snapshot() else {
                        break;
                    };
                    if let Some(session) = snapshot
                        .incoming
                        .filter(|s| s.phase == vnilink::SessionPhase::Active)
                    {
                        let key = (session.id, session.generation);
                        if served != Some(key) {
                            served = Some(key);
                            let quality = session
                                .quality
                                .unwrap_or(vnilink::Quality::new(8000, 30, 1920, 1080).unwrap());
                            if let Err(error) = vnilink_media::serve_capture(
                                &source,
                                &core,
                                session.id,
                                quality,
                                &stop,
                                vnilink_linux::LinuxEncoder::open,
                            ) {
                                // Host errors otherwise disappear into a short-lived
                                // toast while the controller receives a generic refusal.
                                eprintln!("VniLink desktop hosting stopped: {error}");
                                *ERROR.lock().unwrap() = Some(error.to_string());
                                if core
                                    .snapshot()
                                    .ok()
                                    .and_then(|s| s.incoming)
                                    .is_some_and(|s| {
                                        (s.id, s.generation) == key
                                            && s.phase == vnilink::SessionPhase::Active
                                    })
                                {
                                    let _ = core.stop_hosted_session();
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
            Ok((width, height))
        },
        move |result| {
            button.set_sensitive(true);
            match result {
                Ok((width, height)) => {
                    if let Some(settings) = parent.downcast_ref::<adw::Window>() {
                        preferences::toast(
                            settings,
                            adw::Toast::new(&format!(
                                "Desktop sharing and control ready: {width} × {height}"
                            )),
                        );
                    }
                }
                Err(error) => preferences::report(&parent, &error),
            }
        },
    );
}
