//! User-selected screen capture. X11 and the Wayland permission portal remain
//! distinct; one portal grant is never treated as unattended desktop control.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vnilink_media::{FrameSource, RawFrame};

static ERROR: Mutex<Option<String>> = Mutex::new(None);
pub(crate) fn take_error() -> Option<String> {
    ERROR.lock().unwrap().take()
}
static CURRENT: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);
struct Monitor(u16, u16);
impl vnilink::HostCapture for Monitor {
    fn monitor(&self) -> (u16, u16) {
        (self.0, self.1)
    }
}
enum Source {
    X11(vnilink_linux::X11Capture),
    Portal(vnilink_linux::PortalCapture),
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

pub(crate) fn choose(parent: &adw::Window, core: Arc<vnilink::Core>, button: &gtk::Button) {
    button.set_sensitive(false);
    let button = button.clone();
    let parent = parent.clone();
    tasks::run(
        move || {
            let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
            let source = if wayland {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "Could not open the screen permission portal.".to_string())?;
                let mut session = vnilink_linux::PortalSession::new();
                let remote = runtime
                    .block_on(vnilink_linux::open_monitor(&mut session))
                    .map_err(|e| e.to_string())?;
                let node = session
                    .pipewire_node()
                    .ok_or("The portal did not select a monitor.")?;
                Source::Portal(
                    vnilink_linux::PortalCapture::open(remote, node).map_err(|e| e.to_string())?,
                )
            } else {
                Source::X11(
                    vnilink_linux::X11Capture::open(&vnilink_linux::X11Plan::current_session())
                        .map_err(|e| e.to_string())?,
                )
            };
            let (width, height) = source.size();
            core.set_capture(Box::new(Monitor(width, height)))
                .map_err(|e| e.to_string())?;
            if !wayland {
                core.enable_os_input().map_err(|e| e.to_string())?;
            }
            let stop = Arc::new(AtomicBool::new(false));
            if let Some(previous) = CURRENT.lock().unwrap().replace(stop.clone()) {
                previous.store(true, Ordering::Relaxed);
            }
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
            Ok((width, height, wayland))
        },
        move |result| {
            button.set_sensitive(true);
            match result {
                Ok((width, height, wayland)) => {
                    preferences::toast(
                        &parent,
                        adw::Toast::new(&format!(
                            "Screen selected: {width} × {height}{}",
                            if wayland { " (viewing only)" } else { "" }
                        )),
                    );
                }
                Err(error) => preferences::report(&parent, &error),
            }
        },
    );
}
