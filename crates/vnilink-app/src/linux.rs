//! Linux screen hosting: one monitor through the desktop portal, served to
//! every viewer.
//!
//! Starting a host asks the RemoteDesktop portal for one screen, keyboard
//! and pointer, and returns at once: someone at this computer answers the
//! dialog. The grant then holds for the host's life, so viewers connect
//! without another dialog, and the compositor shows the screen as shared
//! meanwhile. PipeWire delivers pictures only when the screen changes, and
//! nothing is encoded while no one watches. Sharing the screen without
//! allowing remote interaction, which GNOME's dialog leaves off by default,
//! shares it view-only. Declining the dialog, or ending the share from the
//! compositor's own controls, ends the host.
//!
//! Given a grant file, the portal keeps the consent until it is revoked in
//! the system's privacy settings, as RustDesk's does: a later host shares
//! the same screen, with the same interaction, without asking. Only the
//! first host, and the first after a revocation, asks.
use crate::{FfiError, VnilinkCore};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vnilink::Core;
use vnilink_linux::WaylandCapture;
use vnilink_media::FrameSource;

/// How long the portal dialog may wait for an answer. The host may start
/// while no one is at this computer, so this is generous.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(600);

/// The one display this host captures: the monitor chosen in the dialog.
/// Never 0, which names no display, so a viewer can select it and place its
/// pointer on it.
const DISPLAY: u64 = 1;

/// The screen size admission and input mapping use; frames come from the worker.
struct LinuxCaptureDescriptor {
    width: u16,
    height: u16,
    cursor: bool,
}
impl vnilink::HostCapture for LinuxCaptureDescriptor {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
    /// The portal reports the pointer apart from the picture when the
    /// compositor offers cursor metadata; otherwise it is drawn in.
    fn separate_cursor(&self) -> bool {
        self.cursor
    }
    /// The shared capture makes them ([`vnilink_media::StillEncoder`]).
    fn stills(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct Status {
    granted: bool,
    error: Option<String>,
}

/// A running Linux screen host. Dropping it stops the worker and closes the
/// portal session.
pub struct LinuxHost {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl LinuxHost {
    /// Whether the portal granted the screen and the host serves it.
    pub fn is_started(&self) -> bool {
        let status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        status.granted && !self.stop.load(Ordering::Acquire)
    }

    /// Why sharing is not working: declined, ended on this computer, or the
    /// last capture failure, cleared when a viewer is served again.
    pub fn error(&self) -> Option<String> {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .clone()
    }

    /// Whether the grant is gone for good: declined, unanswered or ended.
    /// Only a new host asks again.
    pub fn has_ended(&self) -> bool {
        self.worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(|worker| worker.is_finished())
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for LinuxHost {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VnilinkCore {
    /// Hosts the monitor chosen in the portal dialog for remote desktop
    /// viewers, with the keyboard and pointer of the same grant when it
    /// allows them. Sharing policy and peer grants still decide who may view.
    /// `grant` is where the portal's consent is kept between hosts
    /// ([`vnilink_linux::ScreenGrant`]); without it every host asks.
    pub fn linux_host(&self, grant: Option<std::path::PathBuf>) -> Result<LinuxHost, FfiError> {
        let core = Arc::clone(&self.inner);
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));
        let worker = {
            let stop = Arc::clone(&stop);
            let status = Arc::clone(&status);
            std::thread::Builder::new()
                .name("vnilink-linux-host".into())
                .spawn(move || serve(&core, &stop, &status, grant))
                .map_err(|_| FfiError::Unsupported {
                    message: "Screen sharing could not start".into(),
                })?
        };
        Ok(LinuxHost {
            stop,
            status,
            worker: Mutex::new(Some(worker)),
        })
    }
}

static TOPOLOGY_REVISION: AtomicU64 = AtomicU64::new(0);

/// Tells viewers where the captured display is, so their pointer lands on
/// it and the host accepts it. The portal places its one stream at the
/// origin, and input maps the stream's pixels.
fn publish_topology(core: &Core, (width, height): (u16, u16)) {
    let revision = TOPOLOGY_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
    let display = vnilink::DisplayInfo {
        id: DISPLAY,
        origin_x: 0,
        origin_y: 0,
        width,
        height,
        rotation_deg: 0,
        scale_percent: 100,
        revision,
    };
    let _ = core.set_topology(revision, &[display]);
}

/// Peers learn why the screen cannot be viewed or controlled.
fn report_consent(core: &Core, view: bool, control: bool) {
    let _ = core.report_os_permission(vnilink::Capability::DesktopView, !view);
    let _ = core.report_os_permission(vnilink::Capability::DesktopControl, !control);
}

/// Stands in for input when the grant allowed the screen without remote
/// interaction, so no adapter installed earlier, such as X11's, applies
/// input past that choice.
struct ViewOnly((u16, u16));
impl vnilink::HostDesktop for ViewOnly {
    fn apply(&mut self, _: vnilink::InputEvent) -> Result<(), vnilink::Error> {
        Err(vnilink::Error::InputFailed(
            "remote interaction was not allowed on this computer",
        ))
    }
    fn monitor(&self) -> (u16, u16) {
        self.0
    }
}

/// What the person who started sharing is told when the portal gives no
/// grant, or the granted screen cannot be read: no portal, no screen chosen
/// or no PipeWire stream each say so.
fn refusal(error: &vnilink_media::Error) -> String {
    match error {
        vnilink_media::Error::CaptureDenied(_) => {
            "Screen sharing was declined on this computer.".into()
        }
        _ => format!("Screen sharing could not start: {error}."),
    }
}

/// Asks for the grant, installs it, and serves each run of viewers until
/// stopped or until the grant ends.
fn serve(
    core: &Arc<Core>,
    stop: &Arc<AtomicBool>,
    status: &Mutex<Status>,
    grant: Option<std::path::PathBuf>,
) {
    let set = |granted: bool, error: Option<String>| {
        *status.lock().unwrap_or_else(|e| e.into_inner()) = Status { granted, error };
    };
    let startup = vnilink::BackendStartup::with_stop(Arc::clone(stop), CONSENT_TIMEOUT);
    let capture = match WaylandCapture::open(startup, grant.map(vnilink_linux::ScreenGrant::at)) {
        Ok(capture) => capture,
        Err(error) => {
            if !stop.load(Ordering::Acquire) {
                if matches!(error, vnilink_media::Error::CaptureDenied(_)) {
                    report_consent(core, false, false);
                }
                set(false, Some(refusal(&error)));
            }
            return;
        }
    };
    let size = capture.size();
    match core.set_capture(Box::new(LinuxCaptureDescriptor {
        width: size.0,
        height: size.1,
        cursor: capture.separate_cursor(),
    })) {
        // A live viewer pins the descriptor; the capture it describes is the same monitor.
        Ok(()) | Err(vnilink::Error::Busy) => {}
        Err(error) => {
            set(false, Some(error.to_string()));
            return;
        }
    }
    // Input travels the portal session the user granted, never X11.
    let input = capture.desktop();
    let control = input.is_some();
    let desktop: Box<dyn vnilink::HostDesktop> = match input {
        Some(input) => Box::new(input),
        None => Box::new(ViewOnly(size)),
    };
    match core.set_desktop(desktop) {
        Ok(()) | Err(vnilink::Error::Busy) => {}
        Err(error) => {
            set(false, Some(error.to_string()));
            return;
        }
    }
    publish_topology(core, size);
    report_consent(core, true, control);
    set(true, None);
    while !stop.load(Ordering::Acquire) && !capture.revoked() {
        if core.video_targets().unwrap_or_default().is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let result = vnilink_media::serve_shared_capture(
            &capture,
            core,
            stop,
            true,
            DISPLAY,
            // The cursor mode is fixed by the grant.
            |_| {},
            |target, width, height, fps, bps| {
                vnilink_linux::LinuxEncoder::open_codec(width, height, fps, bps, target.codec)
            },
        );
        match result {
            Ok(()) => set(true, None),
            Err(_) if !stop.load(Ordering::Acquire) && !capture.revoked() => {
                set(
                    true,
                    Some("Screen sharing stopped. Try connecting again.".into()),
                );
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => {}
        }
    }
    if capture.revoked() && !stop.load(Ordering::Acquire) {
        report_consent(core, false, false);
        set(
            false,
            Some("Screen sharing was ended on this computer.".into()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn a_declined_dialog_reads_as_declined_and_any_other_failure_names_its_cause() {
        use vnilink_media::Error;
        assert_eq!(
            refusal(&Error::CaptureDenied("declined")),
            "Screen sharing was declined on this computer."
        );
        assert_eq!(
            refusal(&Error::Unsupported("the RemoteDesktop portal")),
            "Screen sharing could not start: the RemoteDesktop portal is not available on this system."
        );
        assert_eq!(
            refusal(&Error::CaptureRejected(
                "choose one screen in the RemoteDesktop permission dialog"
            )),
            "Screen sharing could not start: choose one screen in the RemoteDesktop permission dialog."
        );
    }

    #[test]
    fn a_view_only_grant_refuses_every_input() {
        use vnilink::HostDesktop;
        let mut desktop = ViewOnly((1920, 1080));
        assert!(matches!(
            desktop.apply(vnilink::InputEvent::key_down(vnilink::usage::A)),
            Err(vnilink::Error::InputFailed(_))
        ));
        assert!(desktop.type_text("text").is_err());
        assert_eq!(desktop.monitor(), (1920, 1080));
    }

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

    /// Stopping while the dialog waits closes the request and leaves
    /// nothing installed. Needs a desktop session bus; the dialog that
    /// appears closes by itself.
    #[test]
    #[ignore = "needs a desktop session with xdg-desktop-portal"]
    fn stopping_while_the_dialog_waits_ends_the_host_without_a_grant() {
        let host = core();
        let hosting = host.linux_host().unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert!(!hosting.is_started());
        let stopped = Instant::now();
        hosting.stop();
        assert!(stopped.elapsed() < Duration::from_secs(5));
        assert!(hosting.has_ended());
        assert_eq!(hosting.error(), None);
        assert!(
            !host
                .capability_availability(None, crate::FfiCapability::DesktopView)
                .unwrap()
                .available
        );
    }

    /// The portal dialog needs someone at this computer to allow sharing,
    /// so this is run explicitly in the desktop session:
    /// `cargo test -p vnilink-app linux -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs a desktop session and someone to allow the portal dialog"]
    fn a_viewer_receives_frames_from_the_linux_host() {
        let host = core();
        let viewer = core();
        host.prepare_native_video().unwrap();
        viewer.prepare_native_video().unwrap();
        let hosting = host.linux_host().unwrap();
        let until = Instant::now() + Duration::from_secs(120);
        while !hosting.is_started() {
            assert!(hosting.error().is_none(), "{:?}", hosting.error());
            assert!(Instant::now() < until, "the dialog was not answered");
            std::thread::sleep(Duration::from_millis(50));
        }
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
        // A keyframe proves the grant, capture, encode and transport.
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
