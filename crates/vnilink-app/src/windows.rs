//! Windows screen hosting: the primary output, served to every viewer.
//!
//! Starting a host registers the screen size and returns at once. Desktop
//! Duplication opens only while a viewer is connected and closes when the
//! last one leaves, so an idle host captures nothing. The secure UAC desktop
//! is never captured.
use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vnilink::Core;

/// How long a viewer may wait for Desktop Duplication to open.
const CAPTURE_STARTUP: Duration = Duration::from_secs(15);

/// The screen size admission and input mapping use; frames come from the worker.
struct WindowsCaptureDescriptor {
    width: u16,
    height: u16,
}
impl vnilink::HostCapture for WindowsCaptureDescriptor {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
    /// Desktop Duplication reports the pointer apart from the picture.
    fn separate_cursor(&self) -> bool {
        true
    }
    /// The shared capture makes them ([`vnilink_media::StillEncoder`]).
    fn stills(&self) -> bool {
        true
    }
}

/// A running Windows screen host. Dropping it stops the worker.
pub struct WindowsHost {
    size: (u16, u16),
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl WindowsHost {
    /// The captured output's size in pixels.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The last capture failure, cleared when a viewer is served again.
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for WindowsHost {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VnilinkCore {
    /// Hosts the primary output for remote desktop viewers. Windows asks for
    /// no consent; sharing policy and peer grants still decide who may view.
    pub fn windows_host(&self) -> Result<WindowsHost, FfiError> {
        let core = Arc::clone(&self.inner);
        // Read the size once; the capture itself waits for a viewer.
        let probe =
            vnilink::BackendStartup::with_stop(Arc::new(AtomicBool::new(false)), CAPTURE_STARTUP);
        let size = vnilink_windows::WindowsCapture::open(&probe)
            .map_err(|_| FfiError::Unsupported {
                message: "This screen cannot be captured".into(),
            })?
            .size();
        match core.set_capture(Box::new(WindowsCaptureDescriptor {
            width: size.0,
            height: size.1,
        })) {
            // A live viewer pins the descriptor; the capture it describes is the same output.
            Ok(()) | Err(vnilink::Error::Busy) => {}
            Err(error) => return Err(map_error(error)),
        }
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let worker = {
            let stop = Arc::clone(&stop);
            let error = Arc::clone(&error);
            std::thread::Builder::new()
                .name("vnilink-windows-host".into())
                .spawn(move || serve(&core, &stop, &error))
                .map_err(|_| FfiError::Unsupported {
                    message: "Screen sharing could not start".into(),
                })?
        };
        Ok(WindowsHost {
            size,
            stop,
            error,
            worker: Mutex::new(Some(worker)),
        })
    }
}

/// The one display this host captures: the primary display, on whichever
/// adapter it is attached to. Never 0, which names no display, so a viewer
/// can select it and place its pointer on it.
const DISPLAY: u64 = 1;

static TOPOLOGY_REVISION: AtomicU64 = AtomicU64::new(0);

/// Tells viewers where the captured display is, so their pointer lands on it
/// and the host accepts it. A resize or move reopens the capture, which
/// publishes again.
fn publish_topology(core: &Core, capture: &vnilink_windows::WindowsCapture) {
    let revision = TOPOLOGY_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
    let (width, height) = capture.size();
    let (origin_x, origin_y) = capture.origin();
    let display = vnilink::DisplayInfo {
        id: DISPLAY,
        origin_x,
        origin_y,
        width,
        height,
        rotation_deg: 0,
        scale_percent: 100,
        revision,
    };
    let _ = core.set_topology(revision, &[display]);
}

/// Opens Desktop Duplication for each run of viewers and closes it between them.
fn serve(core: &Arc<Core>, stop: &Arc<AtomicBool>, error: &Mutex<Option<String>>) {
    let set_error = |message: Option<String>| {
        *error.lock().unwrap_or_else(|e| e.into_inner()) = message;
    };
    while !stop.load(Ordering::Acquire) {
        if core.video_targets().unwrap_or_default().is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let startup = match core.desktop_capture_startup(DISPLAY, Arc::clone(stop), CAPTURE_STARTUP)
        {
            Ok(startup) => startup,
            Err(vnilink::Error::NoSession) => continue,
            Err(vnilink::Error::ShutDown) => break,
            Err(failure) => {
                set_error(Some(failure.to_string()));
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let opened_at = std::time::Instant::now();
        let result = (|| -> Result<(), vnilink_media::PublishError> {
            let capture = vnilink_windows::WindowsCapture::open(&startup)?;
            // Encoders open on the capture's GPU, where its pictures live.
            let device = capture.device();
            publish_topology(core, &capture);
            set_error(None);
            vnilink_media::serve_shared_capture(
                &capture,
                core,
                stop,
                true,
                DISPLAY,
                // Pictures carry the pointer while any viewer cannot draw it.
                |targets| {
                    capture.set_embeds_cursor(targets.iter().any(|target| !target.separate_cursor));
                },
                move |target, width, height, fps, bps| {
                    vnilink_windows::WindowsEncoder::open_codec_on(
                        width,
                        height,
                        fps,
                        bps,
                        target.codec,
                        &device,
                    )
                },
            )
        })();
        // Viewers learn of a secure desktop as a condition, not an end: the
        // capture opens again once the input desktop is the user's.
        let secure = secure_desktop(&result, opened_at.elapsed());
        if let Some(active) = secure {
            let _ = core.report_secure_desktop(active);
        }
        if result.is_err() && !stop.load(Ordering::Acquire) && !startup.is_cancelled() {
            if secure != Some(true) {
                set_error(Some("Screen sharing stopped. Try connecting again.".into()));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Whether a capture run says the screen is, or is no longer, on a secure
/// desktop: its failure names one, and a run that served a second or ended
/// by itself was on the user's. `None` when it says neither.
fn secure_desktop(
    result: &Result<(), vnilink_media::PublishError>,
    served: Duration,
) -> Option<bool> {
    match result {
        Err(vnilink_media::PublishError::Media(vnilink_media::Error::SecureDesktop)) => Some(true),
        Ok(()) => Some(false),
        Err(_) if served >= Duration::from_secs(1) => Some(false),
        Err(_) => None,
    }
}

#[cfg(test)]
mod secure_desktop_tests {
    use super::secure_desktop;
    use std::time::Duration;
    use vnilink_media::{Error, PublishError};

    #[test]
    fn a_secure_desktop_is_reported_and_cleared_by_a_run_on_the_users() {
        let secure = Err(PublishError::Media(Error::SecureDesktop));
        assert_eq!(secure_desktop(&secure, Duration::ZERO), Some(true));
        assert_eq!(secure_desktop(&Ok(()), Duration::ZERO), Some(false));
        let other = Err(PublishError::Media(Error::CaptureFailed(5)));
        assert_eq!(secure_desktop(&other, Duration::from_millis(10)), None);
        assert_eq!(secure_desktop(&other, Duration::from_secs(2)), Some(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn core() -> VnilinkCore {
        VnilinkCore {
            inner: Arc::new(
                Core::start(vnilink::CoreConfig {
                    listen: vnilink::Listen::Loopback,
                    relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                        "http://127.0.0.1:9",
                    )
                    .unwrap()])
                    .unwrap(),
                    ..vnilink::CoreConfig::default()
                })
                .unwrap(),
            ),
            previews: Mutex::new(std::collections::HashMap::new()),
            next_preview: std::sync::atomic::AtomicU64::new(1),
            audio_coordinator: Arc::new(crate::audio_native::AudioCoordinator::default()),
        }
    }

    /// Desktop Duplication opens only on an interactive desktop, so this is run
    /// explicitly there: `cargo test -p vnilink-app windows -- --ignored`.
    #[test]
    #[ignore = "needs an interactive Windows desktop"]
    fn a_viewer_receives_frames_from_the_windows_host() {
        let host = core();
        let viewer = core();
        host.prepare_native_video().unwrap();
        viewer.prepare_native_video().unwrap();
        let hosting = host.windows_host().unwrap();
        assert!(hosting.size().0 >= 2 && hosting.size().1 >= 2);
        host.start_hosting().unwrap();
        viewer
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(viewer.create_invitation(60).unwrap())
            .unwrap();
        host.set_desktop_sharing(true, false, false, false).unwrap();
        let parent = viewer.begin_connect_services(host.endpoint_id()).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while viewer.controller_session(parent.id).unwrap().phase != crate::FfiSessionPhase::Active
        {
            assert!(Instant::now() < until, "the viewer did not connect");
            std::thread::sleep(Duration::from_millis(10));
        }
        let parent = viewer.controller_session(parent.id).unwrap();
        let owner = viewer
            .attach_controller_service(parent.id, parent.generation, crate::FfiServiceKind::Desktop)
            .unwrap();
        let desktop = viewer
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let video = desktop.subscribe_video().unwrap();
        // Capture opens for this viewer only; a keyframe proves capture, encode and transport.
        let until = Instant::now() + Duration::from_secs(20);
        let frame = loop {
            if let Some(frame) = video.recv_timeout(500).unwrap() {
                if frame.keyframe {
                    break frame;
                }
            }
            assert!(
                Instant::now() < until,
                "no frame arrived; host error: {:?}",
                hosting.error()
            );
        };
        assert!(!frame.payload.is_empty());
        drop(desktop);
        hosting.stop();
    }
}
