//! User-selected screen capture. X11 and the Wayland permission portal remain
//! distinct; one portal grant is never treated as unattended desktop control.
//!
//! Each shared display runs under its own owner in [`SOURCES`]: one X11 CRTC
//! per owner under a supervisor, or the single portal grant. Opening, closing
//! or switching one owner never touches its siblings.
use super::{preferences, tasks};
use adw::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use vnilink_linux::{SourceOwner, SourceOwners};
use vnilink_media::{FrameSource, RawFrame};

static ERROR: Mutex<Option<String>> = Mutex::new(None);
pub(crate) fn take_error() -> Option<String> {
    ERROR.lock().unwrap().take()
}
static TOPOLOGY_REVISION: AtomicU64 = AtomicU64::new(0);
static SOURCES: OnceLock<Mutex<SourceOwners>> = OnceLock::new();
static SUPERVISOR: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

fn sources() -> std::sync::MutexGuard<'static, SourceOwners> {
    SOURCES
        .get_or_init(|| Mutex::new(SourceOwners::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Remove an owner only if it is still the registered one: a replacement
/// source must survive its predecessor's thread exit.
fn remove_if_current(id: u64, stop: &Arc<AtomicBool>) {
    let mut owners = sources();
    let current = owners
        .get(id)
        .is_some_and(|owner| Arc::ptr_eq(&owner.stop, stop));
    if current {
        owners.remove(id);
    }
}

pub(crate) fn stop() {
    if let Some(stop) = SUPERVISOR.lock().unwrap().take() {
        stop.store(true, Ordering::Relaxed);
    }
    sources().clear();
}

fn sleep_checked(stop: &Arc<AtomicBool>, duration: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < duration {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
struct Monitor(u16, u16, bool);
impl vnilink::HostCapture for Monitor {
    fn monitor(&self) -> (u16, u16) {
        (self.0, self.1)
    }
    fn separate_cursor(&self) -> bool {
        self.2
    }
}
enum Source {
    X11(vnilink_linux::X11Capture),
    Portal(vnilink_linux::WaylandCapture),
}
impl Source {
    fn separate_cursor(&self) -> bool {
        match self {
            Self::Portal(capture) => capture.separate_cursor(),
            Self::X11(capture) => capture.separate_cursor(),
        }
    }

    /// The X11 source publishes the CRTC it cropped; the portal picture is
    /// the one granted source, published as display 1.
    fn capture_display(&self) -> u64 {
        match self {
            Self::Portal(_) => 1,
            Self::X11(capture) => u64::from(capture.crtc().unwrap_or(0)),
        }
    }

    fn topology(&self, revision: u64) -> Result<Vec<vnilink::DisplayInfo>, String> {
        match self {
            Self::X11(capture) => capture
                .displays(revision)
                .map_err(|error| error.to_string()),
            Self::Portal(_) => {
                let (width, height) = self.size();
                Ok(vec![vnilink::DisplayInfo {
                    id: 1,
                    origin_x: 0,
                    origin_y: 0,
                    width,
                    height,
                    rotation_deg: 0,
                    scale_percent: 100,
                    revision,
                }])
            }
        }
    }
}
impl FrameSource for Source {
    fn cursor(&self) -> Option<vnilink::CursorState> {
        match self {
            Self::Portal(capture) => capture.cursor(),
            Self::X11(capture) => capture.cursor(),
        }
    }
    fn capture_revision(&self) -> Arc<AtomicU64> {
        match self {
            // The supervisor attests the crop to its published revision
            // and re-attests it while the fingerprint matches.
            Self::X11(capture) => capture.shared_revision(),
            // The fixed portal grant follows the current topology.
            Self::Portal(_) => Arc::new(AtomicU64::new(0)),
        }
    }
    fn size(&self) -> (u16, u16) {
        match self {
            Self::X11(s) => s.size(),
            Self::Portal(s) => s.size(),
        }
    }
    fn poll_frame(&self, time: u64) -> Result<Option<RawFrame>, vnilink_media::Error> {
        match self {
            Self::X11(s) => s.next_frame(time).map(Some),
            Self::Portal(s) => s.poll_frame(time),
        }
    }
}

pub(crate) fn choose(
    parent: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    button: &gtk::Button,
) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        choose_portal(parent, core, button);
    } else {
        choose_x11(parent, core, button, false);
    }
}
pub(crate) fn resume(parent: &impl IsA<gtk::Window>, core: Arc<vnilink::Core>) {
    // An unattended launch must never request a Wayland portal grant.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        choose_x11(parent, core, &gtk::Button::new(), true);
    }
}

fn open_owner_encoder(
    target: &vnilink::VideoTarget,
    width: u16,
    height: u16,
    fps: u8,
    bps: u32,
) -> Result<vnilink_linux::LinuxEncoder, vnilink_media::Error> {
    let high = target.codec == vnilink::CODEC_H264_HIGH;
    vnilink_linux::LinuxEncoder::open(width, height, fps, bps, high)
}

/// Serve one source until its owner stops. A capture error is recorded and
/// retried; only the owner's own stop (or the global one) ends the thread,
/// and the registry entry goes only if this owner still holds it.
fn serve_owner(core: Arc<vnilink::Core>, stop: Arc<AtomicBool>, source: Source) {
    std::thread::spawn(move || {
        let display = source.capture_display();
        while !stop.load(Ordering::Relaxed) {
            match vnilink_media::serve_shared_capture(
                &source,
                &core,
                &stop,
                false,
                display,
                |_| {},
                open_owner_encoder,
            ) {
                Ok(()) => break,
                Err(error) => {
                    eprintln!("VniLink desktop hosting stopped: {error}");
                    *ERROR.lock().unwrap() = Some(error.to_string());
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        remove_if_current(display, &stop);
    });
}

/// One-shot portal grant: register its owner before consent so Stop also
/// reaches the pending request. Never supervised: the grant is fixed at
/// consent time and cannot be reopened on another display.
fn choose_portal(parent: &impl IsA<gtk::Window>, core: Arc<vnilink::Core>, button: &gtk::Button) {
    // Register before opening the portal so Stop also reaches pending consent.
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut owners = sources();
        if let Some(previous) = owners.get(1).map(|owner| owner.stop.clone()) {
            previous.store(true, Ordering::Release);
        }
        owners.insert(1, SourceOwner::new(stop.clone(), 0, 0));
    }
    button.set_sensitive(false);
    let button = button.clone();
    let parent = parent.as_ref().clone();
    tasks::run(
        move || {
            let outcome = portal_open(&core, &stop);
            if outcome.is_err() {
                remove_if_current(1, &stop);
            }
            outcome
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

fn portal_open(core: &Arc<vnilink::Core>, stop: &Arc<AtomicBool>) -> Result<(u16, u16), String> {
    let startup = vnilink::BackendStartup::with_stop(stop.clone(), Duration::from_secs(120));
    let capture =
        vnilink_linux::WaylandCapture::open(startup.clone()).map_err(|e| e.to_string())?;
    startup.check().map_err(|e| e.to_string())?;
    // Fence the grant against Stop and a newer selection: either signals
    // this owner's stop before the serve thread starts.
    if stop.load(Ordering::Relaxed) {
        return Err("Screen selection was cancelled.".to_string());
    }
    let source = Source::Portal(capture);
    let (width, height) = source.size();
    sources().update_size(1, width, height);
    let revision = TOPOLOGY_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
    let displays = source.topology(revision)?;
    if !displays.is_empty() {
        core.set_topology(revision, &displays)
            .map_err(|e| e.to_string())?;
    }
    // First source wins the shared capture slot; siblings keep serving.
    let _ = core.set_capture(Box::new(Monitor(width, height, source.separate_cursor())));
    let Source::Portal(capture) = &source else {
        return Err("Screen selection was cancelled.".to_string());
    };
    core.set_desktop(Box::new(capture.desktop()))
        .map_err(|e| e.to_string())?;
    // Re-probe encode after the capture changes; decode ability is
    // untouched by capture and stays as registered.
    let probed = vnilink_linux::video_codec_caps();
    let mut caps = core.video_codecs().map_err(|e| e.to_string())?;
    caps.encode_baseline = probed.encode_baseline;
    caps.encode_high = probed.encode_high;
    caps.max_width = probed.max_width;
    caps.max_height = probed.max_height;
    core.set_video_codecs(caps).map_err(|e| e.to_string())?;
    serve_owner(core.clone(), stop.clone(), source);
    Ok((width, height))
}

fn choose_x11(
    parent: &impl IsA<gtk::Window>,
    core: Arc<vnilink::Core>,
    button: &gtk::Button,
    resume_hosting: bool,
) {
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(previous) = SUPERVISOR.lock().unwrap().replace(stop.clone()) {
        previous.store(true, Ordering::Release);
    }
    std::thread::spawn({
        let core = core.clone();
        let stop = stop.clone();
        move || supervise(core, &stop, resume_hosting)
    });
    // The supervisor owns setup from here; the button stays live so Stop
    // and a retry never wedge behind a pending probe.
    button.set_sensitive(true);
    if let Some(window) = parent.as_ref().downcast_ref::<adw::Window>() {
        preferences::toast(window, adw::Toast::new("Desktop sharing started"));
    }
}

/// Hold one owner per wanted display: publish the union topology, retire
/// owners whose display vanished, and open each missing wanted display.
/// A replaced supervisor leaves live owners serving; the replacement
/// adopts them from the registry and only fills the gaps.
fn supervise(core: Arc<vnilink::Core>, stop: &Arc<AtomicBool>, resume_hosting: bool) {
    if resume_hosting {
        if let Err(error) = core.start_hosting() {
            *ERROR.lock().unwrap() = Some(error.to_string());
            return;
        }
    }
    if let Err(error) = core.enable_os_input() {
        *ERROR.lock().unwrap() = Some(error.to_string());
        return;
    }
    // Re-probe encode after the capture changes; decode ability is
    // untouched by capture and stays as registered.
    let probed = vnilink_linux::video_codec_caps();
    match core.video_codecs() {
        Ok(mut caps) => {
            caps.encode_baseline = probed.encode_baseline;
            caps.encode_high = probed.encode_high;
            caps.max_width = probed.max_width;
            caps.max_height = probed.max_height;
            if let Err(error) = core.set_video_codecs(caps) {
                *ERROR.lock().unwrap() = Some(error.to_string());
                return;
            }
        }
        Err(error) => {
            *ERROR.lock().unwrap() = Some(error.to_string());
            return;
        }
    }
    let plan = vnilink_linux::X11Plan::current_session();
    let mut published: Vec<(u64, vnilink_linux::GeometryFingerprint)> = Vec::new();
    let mut published_rev: u64 = 0;
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        if !first {
            sleep_checked(stop, Duration::from_secs(2));
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        first = false;
        let probe = match vnilink_linux::X11Capture::open(&plan) {
            Ok(probe) => probe,
            Err(error) => {
                *ERROR.lock().unwrap() = Some(error.to_string());
                continue;
            }
        };
        let revision = TOPOLOGY_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
        let topology = match probe.displays(revision) {
            Ok(topology) => topology,
            Err(error) => {
                *ERROR.lock().unwrap() = Some(error.to_string());
                continue;
            }
        };
        drop(probe);
        let signature = vnilink_linux::topology_signature(&topology);
        if signature != published {
            match core.set_topology(revision, &topology) {
                Ok(()) => {
                    published = signature;
                    published_rev = revision;
                }
                Err(error) => {
                    *ERROR.lock().unwrap() = Some(error.to_string());
                    continue;
                }
            }
        }
        let requested = core.capture_displays().unwrap_or_default();
        let wanted = vnilink_linux::wanted_owners(&requested, &topology);
        sources().retire_except(&wanted);
        // Same-id geometry changes (move, resize, rotation, scale) retire
        // exactly the affected owners; the open loop below reopens them
        // with the fresh crop and revision. Siblings are untouched.
        for id in vnilink_linux::stale_owners(&sources(), &wanted, &topology) {
            sources().remove(id);
        }
        for display in &topology {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if !wanted.contains(&display.id) {
                continue;
            }
            if sources().contains(display.id) {
                // Fingerprint verified above: the pixels match the current
                // geometry, so re-attest the owner instead of reopening it.
                sources().bump_revision(display.id, published_rev);
                continue;
            }
            let id = u32::try_from(display.id).unwrap_or(0);
            match vnilink_linux::X11Capture::open_display(&plan, id) {
                Ok(capture) => {
                    let owner_stop = Arc::new(AtomicBool::new(false));
                    let (width, height) = capture.size();
                    capture.set_revision(published_rev);
                    sources().insert(
                        display.id,
                        SourceOwner::new(owner_stop.clone(), width, height)
                            .with_fingerprint(vnilink_linux::GeometryFingerprint::of(display))
                            .with_revision(capture.shared_revision()),
                    );
                    // First source wins the shared capture slot; siblings keep serving.
                    let _ = core.set_capture(Box::new(Monitor(
                        width,
                        height,
                        capture.separate_cursor(),
                    )));
                    serve_owner(core.clone(), owner_stop, Source::X11(capture));
                }
                Err(error) => {
                    *ERROR.lock().unwrap() = Some(error.to_string());
                }
            }
        }
    }
}
