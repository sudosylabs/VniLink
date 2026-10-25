//! VideoToolbox stays on a Rust worker. Swift polls one latest decoded picture.
// trace: records every ignored failure
use crate::{FfiError, VnilinkCore};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use vnilink::{Core, EncodedFrame, SessionPhase};
use vnilink_media::{
    ControllerOutput, ControllerVideo, EncodeParams, RawFrame, VideoDecoder, VideoEncoder,
};

pub struct MacFrameBuffer {
    pub width: u16,
    pub height: u16,
    pub stride: u32,
    pub address: u64,
    pub byte_count: u64,
}

/// Immutable decoded pixels owned by this in-process object. The native image
/// provider retains the object until it finishes drawing; pixels never travel
/// through byte-by-byte language-binding serialization.
pub struct MacFrame {
    picture: RawFrame,
    display_id: u64,
    revision: u64,
}

impl MacFrame {
    /// Read-only BGRA memory, valid for as long as this frame is retained.
    /// Only the native macOS adapter may use this process-local address.
    pub fn buffer(&self) -> MacFrameBuffer {
        MacFrameBuffer {
            width: self.picture.width(),
            height: self.picture.height(),
            stride: self.picture.stride() as u32,
            address: self.picture.pixels().as_ptr() as usize as u64,
            byte_count: self.picture.pixels().len() as u64,
        }
    }
    /// Topology display id attested by the encoded frame. Zero means
    /// unstamped: the viewer must fence pointer input, never guess.
    pub fn display_id(&self) -> u64 {
        self.display_id
    }
    /// Topology revision the frame was captured under.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

pub struct MacCursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    pub bgra: Vec<u8>,
}
pub struct MacCursorUpdate {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub image: Option<MacCursorImage>,
}

/// Live Mac hosts per Core. Each host streams its own display through the
/// shared session and topology; only the first host owns shared setup
/// (capture descriptor) and only the last stop tears shared state down, so
/// a sibling display keeps streaming untouched.
static HOSTS: std::sync::OnceLock<Mutex<HashMap<usize, usize>>> = std::sync::OnceLock::new();

fn host_key(core: &Arc<Core>) -> usize {
    Arc::as_ptr(core) as usize
}

fn hosts() -> std::sync::MutexGuard<'static, HashMap<usize, usize>> {
    HOSTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Registers a host that reached its worker. True when no sibling is live.
fn register_host(key: usize) -> bool {
    let mut hosts = hosts();
    let count = hosts.entry(key).or_insert(0);
    *count += 1;
    *count == 1
}

/// Unregisters a stopped host. True when the last host left and the caller
/// owns shared teardown. Absent keys are teardown-safe.
fn unregister_host(key: usize) -> bool {
    let mut hosts = hosts();
    decrement_locked(&mut hosts, key)
}

fn decrement_locked(hosts: &mut HashMap<usize, usize>, key: usize) -> bool {
    let last = match hosts.get_mut(&key) {
        Some(count) => {
            *count = count.saturating_sub(1);
            *count == 0
        }
        None => true,
    };
    if last {
        hosts.remove(&key);
    }
    last
}

struct ViewerState {
    frame: Option<Arc<MacFrame>>,
    error: Option<String>,
}

pub struct MacViewer {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<ViewerState>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    core: Arc<Core>,
    session_id: u64,
    generation: u64,
    desktop: Option<Arc<vnilink::RemoteDesktopService>>,
    cursor: Mutex<Option<Arc<vnilink::CursorState>>>,
}

impl MacViewer {
    pub fn separate_cursor(&self) -> bool {
        if let Some(desktop) = &self.desktop {
            return desktop
                .desktop_info()
                .is_ok_and(|info| info.separate_cursor);
        }
        self.core
            .separate_cursor(self.session_id, vnilink::SessionRole::Controller)
            .unwrap_or(false)
    }
    pub fn poll_cursor(&self) -> Option<MacCursorUpdate> {
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        let current = if let Some(desktop) = &self.desktop {
            desktop.cursor().ok()??
        } else {
            self.core.cursor(self.session_id, self.generation).ok()??
        };
        let mut previous = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
        if previous
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &current))
        {
            return None;
        }
        let image = current
            .image
            .as_ref()
            .filter(|image| previous.as_ref().and_then(|old| old.image.as_ref()) != Some(image));
        let update = MacCursorUpdate {
            x: current.position.0,
            y: current.position.1,
            visible: current.visible,
            image: image.map(|image| MacCursorImage {
                width: image.width(),
                height: image.height(),
                hotspot_x: image.hotspot().0,
                hotspot_y: image.hotspot().1,
                bgra: image.pixels().to_vec(),
            }),
        };
        *previous = Some(current);
        Some(update)
    }

    /// Takes the latest frame; old pictures never accumulate behind the UI.
    pub fn poll(&self) -> Option<Arc<MacFrame>> {
        if self
            .desktop
            .as_ref()
            .is_some_and(|desktop| desktop.is_cancelled())
        {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).frame = None;
            return None;
        }
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame
            .take()
    }
    pub fn error(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .clone()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if worker.join().is_err() {
                tracing::warn!(target: vnilink::trace::CAPTURE, "a capture worker panicked");
            }
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).frame = None;
    }
}
impl Drop for MacViewer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn current(core: &Core, id: u64, generation: u64) -> bool {
    core.controller_session(id).ok().is_some_and(|s| {
        s.id == id && s.generation == generation && s.phase == SessionPhase::Active
    })
}

/// Probe real encode/decode with synthetic pixels; no screen permission needed.
/// Both Baseline and High round-trip, so every registered bit means a
/// backend processed a sample.
fn probe_caps() -> Result<vnilink::VideoCodecCaps, FfiError> {
    let result = (|| -> Result<bool, vnilink_media::Error> {
        let mut hardware = true;
        for high in [false, true] {
            let mut encoder = vnilink_macos::MacEncoder::open(
                64,
                64,
                30,
                500_000,
                high,
                vnilink::YuvMatrix::Bt709,
            )?;
            let raw = RawFrame::bgra(64, 64, 0, vec![255; 64 * 64 * 4])?;
            let sample = encoder.encode(
                &raw,
                &EncodeParams {
                    force_keyframe: true,
                    recovery: false,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )?;
            hardware &= encoder.hardware();
            let frame = EncodedFrame::h264(sample.keyframe(), 0, sample.bytes().to_vec()).map_err(
                |_| vnilink_media::Error::Unsupported("VideoToolbox emitted invalid H.264"),
            )?;
            let picture = vnilink_macos::MacDecoder::new().decode(&frame)?;
            if picture.width() != 64 || picture.height() != 64 {
                return Err(vnilink_media::Error::Unsupported(
                    "VideoToolbox returned invalid dimensions",
                ));
            }
        }
        Ok(hardware)
    })();
    result
        .map(|hardware| {
            // 4096x4096 is the conservative documented ceiling for
            // VideoToolbox H.264; offers above it clamp instead of failing.
            let mut caps = vnilink::VideoCodecCaps::h264_full(4096, 4096, "videotoolbox");
            caps.hardware = hardware;
            caps
        })
        .map_err(|_| FfiError::Unsupported {
            message: "VideoToolbox H.264 is unavailable on this Mac".into(),
        })
}

/// Refresh all installed backends together. Starting capture must preserve the
/// software capabilities that service admission negotiated at engine startup.
pub(crate) fn probe_all_codecs() -> Result<vnilink::VideoCodecCaps, FfiError> {
    let mut caps = probe_caps().unwrap_or_default();
    caps.additional = vnilink_media::probe_software_codecs();
    let native = vnilink_macos::probe_hevc();
    if native.encode && native.decode {
        if let Some(hevc) = caps
            .additional
            .iter_mut()
            .find(|c| c.codec == vnilink::VideoCodec::H265)
        {
            *hevc = native;
        }
    }
    if caps.can_encode() && (caps.max_width == 0 || caps.max_height == 0) {
        caps.max_width = 4096;
        caps.max_height = 4096;
    }
    if caps.can_encode() || caps.can_decode() {
        Ok(caps)
    } else {
        Err(FfiError::Unsupported {
            message: "No video backend is available".into(),
        })
    }
}

impl VnilinkCore {
    pub fn prepare_mac_video(&self) -> Result<(), FfiError> {
        let caps = probe_all_codecs()?;
        self.inner
            .set_cursor_receiver(true)
            .map_err(crate::error::map_error)?;
        self.inner
            .set_video_codecs(caps)
            .map_err(crate::error::map_error)
    }
    pub fn mac_viewer(&self, session_id: u64, generation: u64) -> Result<MacViewer, FfiError> {
        self.build_mac_viewer(session_id, generation, None)
    }
    /// Decode one service-owned Desktop. Closing the handle ends this worker;
    /// stopping the viewer alone does not disconnect its owner or parent.
    pub fn mac_desktop_viewer(
        &self,
        desktop: Arc<crate::services::DesktopService>,
    ) -> Result<MacViewer, FfiError> {
        if !desktop
            .core
            .upgrade()
            .is_some_and(|core| Arc::ptr_eq(&core, &self.inner))
        {
            return Err(FfiError::InvalidSetting {
                message: "Desktop belongs to another core".into(),
            });
        }
        let info = desktop
            .inner
            .desktop_info()
            .map_err(crate::error::map_error)?;
        self.build_mac_viewer(
            info.service.connection_id,
            info.service.generation,
            Some(desktop.inner.clone()),
        )
    }
}

impl VnilinkCore {
    fn build_mac_viewer(
        &self,
        session_id: u64,
        generation: u64,
        desktop: Option<Arc<vnilink::RemoteDesktopService>>,
    ) -> Result<MacViewer, FfiError> {
        if !current(&self.inner, session_id, generation) {
            return Err(FfiError::InvalidSetting {
                message: "The desktop session is no longer active".into(),
            });
        }
        let subscription = match &desktop {
            Some(desktop) => desktop.subscribe_video(),
            None => self.inner.subscribe_session_video(session_id, generation),
        }
        .map_err(crate::error::map_error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ViewerState {
            frame: None,
            error: None,
        }));
        let core = Arc::clone(&self.inner);
        let worker_stop = Arc::clone(&stop);
        let worker_state = Arc::clone(&state);
        let worker_desktop = desktop.clone();
        let worker = std::thread::spawn(move || {
            let mut decoder = ControllerVideo::new(vnilink_macos::MacDecoder::new());
            while !worker_stop.load(Ordering::Acquire)
                && current(&core, session_id, generation)
                && !worker_desktop
                    .as_ref()
                    .is_some_and(|desktop| desktop.is_cancelled())
            {
                // Adaptation runs owner-side in Core (`Adaptive::step`);
                // this worker only decodes and reports decode durations.
                let Some(frame) = subscription.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                let began = std::time::Instant::now();
                let result = decoder.push(&frame);
                if matches!(&result, Ok(ControllerOutput::Decoded(_))) {
                    if let Some(desktop) = &worker_desktop {
                        vnilink::ignored!(
                            vnilink::trace::DECODE,
                            "a decode time for a closed service is not needed",
                            desktop.report_decode_duration(
                                began.elapsed().as_micros().min(5_000_000) as u64,
                            )
                        );
                    }
                }
                match result {
                    Ok(ControllerOutput::Decoded(decoded)) => {
                        if !current(&core, session_id, generation)
                            || worker_desktop
                                .as_ref()
                                .is_some_and(|desktop| desktop.is_cancelled())
                        {
                            break;
                        }
                        worker_state.lock().unwrap_or_else(|e| e.into_inner()).frame =
                            Some(Arc::new(MacFrame {
                                picture: decoded.frame,
                                display_id: decoded.display_id,
                                revision: decoded.revision,
                            }));
                    }
                    Ok(ControllerOutput::Skipped) => {}
                    Err(_) => {
                        worker_state.lock().unwrap_or_else(|e| e.into_inner()).error = Some(
                            "The remote video could not be decoded. Reconnect to try again.".into(),
                        );
                        break;
                    }
                }
            }
        });
        Ok(MacViewer {
            stop,
            state,
            worker: Mutex::new(Some(worker)),
            core: Arc::clone(&self.inner),
            session_id,
            generation,
            desktop,
            cursor: Mutex::new(None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retained_frame_keeps_its_pixel_buffer_alive() {
        let frame = Arc::new(MacFrame {
            picture: RawFrame::bgra(8, 8, 0, vec![42; 256]).unwrap(),
            display_id: 3,
            revision: 9,
        });
        let provider_owner = Arc::clone(&frame);
        let weak = Arc::downgrade(&frame);
        let buffer = frame.buffer();
        assert_eq!(
            (
                buffer.width,
                buffer.height,
                buffer.stride,
                buffer.byte_count
            ),
            (8, 8, 32, 256)
        );
        assert_eq!(
            buffer.address,
            frame.picture.pixels().as_ptr() as usize as u64
        );
        drop(frame);
        assert_eq!(provider_owner.buffer().address, buffer.address);
        assert_eq!(provider_owner.picture.pixels(), &[42; 256]);
        assert!(weak.upgrade().is_some());
        drop(provider_owner);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn mac_video_probe_uses_synthetic_pixels() {
        let caps = super::probe_caps().unwrap();
        assert!(caps.can_encode() && caps.can_decode());
        assert!(caps.encode_high);
        assert_eq!(caps.device, "videotoolbox");
    }

    #[test]
    fn host_registry_tracks_first_and_last_per_core() {
        // Keys are unique to this test: the registry is process-global and
        // shared with every other test.
        let left = 0xA11CEusize;
        let right = 0xB00Bu16 as usize;
        assert!(register_host(left));
        assert!(!register_host(left));
        assert!(register_host(right));
        assert!(!unregister_host(left));
        assert!(unregister_host(left));
        assert!(unregister_host(left));
        assert!(unregister_host(right));
    }
}

/// Sharing configuration with a capture resource opened for each Desktop instance.
/// Dropping it retires capture; peer and service teardown is owned by the caller.
pub struct MacHost {
    started: Arc<AtomicBool>,
    core: Arc<Core>,
    display_id: u64,
    capture_size: (u16, u16),
    logical_size: (u16, u16),
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    cursor: Mutex<HostCursorState>,
}

#[derive(Default)]
struct HostCursorState {
    state: Option<vnilink::CursorState>,
}

#[cfg(test)]
struct HostTarget {
    service: bool,
    id: u64,
    generation: u64,
}
#[cfg(test)]
impl HostTarget {
    fn stamp(&self) -> (bool, u64, u64) {
        (self.service, self.id, self.generation)
    }
    fn close(&self, core: &Core) {
        if self.service {
            vnilink::ignored!(
                vnilink::trace::HOSTING,
                "a hosted service already gone needs no close",
                core.close_hosted_service(self.id, self.generation)
            );
        } else if core.hosted_sessions().ok().is_some_and(|sessions| {
            sessions.into_iter().any(|s| {
                s.id == self.id
                    && s.generation == self.generation
                    && s.phase == SessionPhase::Active
            })
        }) {
            vnilink::ignored!(
                vnilink::trace::HOSTING,
                "a hosted session already gone needs no stop",
                core.stop_hosted_session()
            );
        }
    }
}
#[cfg(test)]
fn host_target(core: &Core) -> Option<HostTarget> {
    if let Some(info) = core
        .hosted_services()
        .ok()?
        .into_iter()
        .filter(|s| s.kind == vnilink::ServiceKind::Desktop)
        .find_map(|s| core.hosted_desktop_info(s.id, s.generation).ok())
    {
        return Some(HostTarget {
            service: true,
            id: info.service.id,
            generation: info.service.generation,
        });
    }
    let session = core
        .hosted_sessions()
        .ok()?
        .into_iter()
        .find(|session| session.phase == SessionPhase::Active && session.quality.is_some())?;
    Some(HostTarget {
        service: false,
        id: session.id,
        generation: session.generation,
    })
}

struct MacCaptureDescriptor {
    width: u16,
    height: u16,
    cursor: bool,
}
impl vnilink::HostCapture for MacCaptureDescriptor {
    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
    }
    fn separate_cursor(&self) -> bool {
        self.cursor
    }
    /// The shared capture makes them ([`vnilink_media::StillEncoder`]).
    fn stills(&self) -> bool {
        true
    }
}

impl MacHost {
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire) && !self.stop.load(Ordering::Acquire)
    }
    /// Cursor updates never inject input. Retain a shape across position-only
    /// samples and resend it when a new hosted generation negotiates metadata.
    pub fn update_cursor(&self, update: MacCursorUpdate) -> Result<(), FfiError> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        publish_host_cursor(
            &self.core,
            self.display_id,
            self.capture_size,
            self.logical_size,
            &self.cursor,
            update,
        )
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn stop(&self) {
        if self.stop.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut hosts = hosts();
        if !decrement_locked(&mut hosts, host_key(&self.core)) {
            // A sibling display keeps hosting: stop only this worker.
            let worker = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
            drop(hosts);
            if let Some(worker) = worker {
                if worker.join().is_err() {
                    tracing::warn!(target: vnilink::trace::CAPTURE, "a capture worker panicked");
                }
            }
            return;
        }
        // Last capture retires encoding capability, never the neutral peer
        // or its unrelated services. Explicit stop sharing owns peer teardown.
        // Tear capture state down while holding the registry, so
        // a concurrent start observes either this host (shared setup
        // skipped) or a fully torn-down core (clean setup), never a
        // half-torn one.
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if worker.join().is_err() {
                tracing::warn!(target: vnilink::trace::CAPTURE, "a capture worker panicked");
            }
        }
        vnilink::ignored!(
            vnilink::trace::ENCODE,
            "a shut-down engine needs no codecs",
            self.core
                .set_video_codecs(vnilink::VideoCodecCaps::decode_only("videotoolbox"))
        );
    }
}
impl Drop for MacHost {
    fn drop(&mut self) {
        self.stop();
    }
}

impl VnilinkCore {
    /// Call only after the user enables sharing and grants Screen Recording.
    /// Input coordinates use main-display logical points. Capture dimensions
    /// use backing pixels so Retina detail is preserved before encoding.
    /// Each call opens one display; concurrent hosts share the session and
    /// topology; retirement stops capture without disconnecting service peers.
    pub fn mac_host(
        &self,
        display_id: u32,
        width: u16,
        height: u16,
        pixel_width: u16,
        pixel_height: u16,
        cursor_supported: bool,
    ) -> Result<MacHost, FfiError> {
        if display_id == 0 {
            return Err(FfiError::InvalidSetting {
                message: "a display id is invalid".into(),
            });
        }
        if width < 2 || height < 2 || pixel_width < 2 || pixel_height < 2 {
            return Err(FfiError::InvalidSetting {
                message: "No main display is available".into(),
            });
        }
        // Viewers draw the pointer only while macOS still names its shape;
        // otherwise it stays in the picture.
        let cursor_supported = cursor_supported && vnilink_macos::system_pointer().image.is_some();
        let core = Arc::clone(&self.inner);
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let worker_started = started.clone();
        let error = Arc::new(Mutex::new(None));
        let worker_core = Arc::clone(&core);
        let worker_stop = Arc::clone(&stop);
        let worker_error = Arc::clone(&error);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let opened = (|| {
                let plan = mac_capture_plan(display_id, pixel_width, pixel_height)?;
                let plan = vnilink_macos::ScreenPlan::from_capture(&plan).map_err(|_| {
                    FfiError::Unsupported {
                        message: "This display cannot be captured".into(),
                    }
                })?;
                let key = host_key(&worker_core);
                let first = register_host(key);
                let setup = (|| {
                    if first {
                        // Only the first live host owns the shared capture
                        // descriptor; a sibling streams its own display
                        // through the shared topology instead of failing
                        // here while viewers are connected.
                        match worker_core.set_capture(Box::new(MacCaptureDescriptor {
                            width,
                            height,
                            cursor: cursor_supported,
                        })) {
                            Ok(()) | Err(vnilink::Error::Busy) => {
                                // A live Desktop/legacy consumer pins the shared
                                // descriptor. Keep it; this source still owns its
                                // independent capture and publishes live topology.
                            }
                            Err(error) => return Err(crate::error::map_error(error)),
                        }
                    }
                    worker_core
                        .set_video_codecs(probe_all_codecs()?)
                        .map_err(crate::error::map_error)?;
                    publish_mac_topology(
                        &worker_core,
                        display_id,
                        width,
                        height,
                        pixel_width,
                        pixel_height,
                    )?;
                    Ok::<_, FfiError>(())
                })();
                if setup.is_err() {
                    unregister_host(key);
                }
                setup?;
                if worker_stop.load(Ordering::Acquire) {
                    // The caller already gave up (or stopped): release the
                    // registration instead of stranding a phantom sibling.
                    unregister_host(key);
                    return Err(FfiError::Unsupported {
                        message: "Screen sharing did not start in time".into(),
                    });
                }
                Ok::<_, FfiError>(plan)
            })();
            let plan = match opened {
                Ok(plan) => {
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    plan
                }
                Err(error) => {
                    vnilink::ignored!(
                        vnilink::trace::CAPTURE,
                        "the opener stopped waiting for the capture",
                        ready_tx.send(Err(error))
                    );
                    return;
                }
            };
            // Restarts in a row whose capture never started.
            let mut failed_starts = 0;
            while !worker_stop.load(Ordering::Acquire) {
                let pending = worker_core.video_targets().unwrap_or_default();
                if pending.is_empty() {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                *worker_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
                if let Err(error) = publish_mac_topology(
                    &worker_core,
                    display_id,
                    width,
                    height,
                    pixel_width,
                    pixel_height,
                ) {
                    *worker_error.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(error.to_string());
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                // One OS stream for the requested display. A viewer who selected
                // another id gets no frames from this stream. A second display
                // is another owner and is not opened by this thread.
                let startup = match worker_core.desktop_capture_startup(
                    u64::from(display_id),
                    worker_stop.clone(),
                    Duration::from_secs(15),
                ) {
                    Ok(startup) => startup,
                    Err(vnilink::Error::NoSession) => continue,
                    Err(error) => {
                        *worker_error.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(error.to_string());
                        break;
                    }
                };
                let result = (|| -> Result<(), vnilink_media::PublishError> {
                    let capture = vnilink_macos::ScreenCapture::start_display(
                        &plan,
                        Some(display_id),
                        &startup,
                    )?;
                    worker_started.store(true, Ordering::Release);
                    let served = AtomicBool::new(false);
                    std::thread::scope(|scope| {
                        if cursor_supported {
                            scope.spawn(|| {
                                follow_pointer(
                                    &worker_core,
                                    u64::from(display_id),
                                    (pixel_width, pixel_height),
                                    (width, height),
                                    &served,
                                );
                            });
                        }
                        let result = vnilink_macos::serve_shared(
                            &capture,
                            &worker_core,
                            &worker_stop,
                            true,
                            u64::from(display_id),
                        );
                        served.store(true, Ordering::Release);
                        result
                    })
                })();
                let capture_started = worker_started.swap(false, Ordering::AcqRel);
                if result.is_err()
                    && !worker_stop.load(Ordering::Acquire)
                    && !startup.is_cancelled()
                {
                    failed_starts = if capture_started {
                        0
                    } else {
                        failed_starts + 1
                    };
                    let loss = capture_loss(vnilink_macos::screen_capture_allowed(), failed_starts);
                    let message = match loss {
                        CaptureLoss::Restart => "Screen sharing stopped unexpectedly and is restarting.",
                        CaptureLoss::Permission => "Screen Recording is off for VniLink. Turn it on in System Settings, then connect again.",
                        CaptureLoss::Failed => "Screen sharing could not restart. Try connecting again.",
                    };
                    *worker_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.into());
                    match loss {
                        CaptureLoss::Restart => {}
                        CaptureLoss::Permission => {
                            vnilink::ignored!(
                                vnilink::trace::CAPTURE,
                                "a permission report to a shut-down engine is moot",
                                worker_core.report_os_permission(
                                    vnilink::availability::Capability::DesktopView,
                                    true,
                                )
                            );
                            fail_display_viewers(
                                &worker_core,
                                u64::from(display_id),
                                vnilink::StreamFailure::Permission,
                            );
                        }
                        CaptureLoss::Failed => {
                            failed_starts = 0;
                            fail_display_viewers(
                                &worker_core,
                                u64::from(display_id),
                                vnilink::StreamFailure::Capture,
                            );
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                } else {
                    failed_starts = 0;
                }
            }
            // Shared teardown (session, encode caps) belongs to the last
            // MacHost::stop, never to one worker's exit: a sibling display
            // keeps streaming while this worker is gone.
        });
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(MacHost {
                started,
                core,
                display_id: u64::from(display_id),
                capture_size: (pixel_width, pixel_height),
                logical_size: (width, height),
                stop,
                error,
                worker: Mutex::new(Some(worker)),
                cursor: Mutex::new(HostCursorState::default()),
            }),
            result => {
                stop.store(true, Ordering::Release);
                // If the OS is still opening, let that worker release its stream
                // when ready_tx observes that the receiver is gone.
                Err(match result {
                    Ok(Err(error)) => error,
                    _ => FfiError::Unsupported {
                        message: "Screen sharing did not start in time".into(),
                    },
                })
            }
        }
    }
    /// Close every hosted Desktop owner without disconnecting its parent or
    /// touching sibling Files/Terminal owners. Capture stop is separate.
    pub fn stop_mac_desktop_sharing(&self) -> Result<(), FfiError> {
        for service in self
            .inner
            .hosted_services()
            .map_err(crate::error::map_error)?
        {
            if service.kind == vnilink::ServiceKind::Desktop {
                match self
                    .inner
                    .close_hosted_service(service.id, service.generation)
                {
                    Ok(()) | Err(vnilink::Error::NoSession | vnilink::Error::StaleSession) => {}
                    Err(error) => return Err(crate::error::map_error(error)),
                }
            }
        }
        Ok(())
    }
    /// Source-scoped capture failure. Keeps healthy Desktop owners and every
    /// neutral parent/Files owner alive; the failure includes its display id.
    pub fn fail_mac_display(&self, display_id: u32, message: String) -> Result<(), FfiError> {
        self.inner
            .fail_desktop_source(u64::from(display_id), &message)
            .map_err(crate::error::map_error)
    }
    /// Explicit user action; Accessibility is preflighted by the Swift UI.
    pub fn enable_mac_input(&self) -> Result<(), FfiError> {
        self.inner
            .enable_os_input()
            .map_err(crate::error::map_error)
    }
}

#[cfg(test)]
mod viewer_tests {
    use super::*;
    struct Desktop;
    impl vnilink::HostDesktop for Desktop {
        fn apply(&mut self, _: vnilink::InputEvent) -> Result<(), vnilink::Error> {
            Ok(())
        }
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
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
            audio_coordinator: std::sync::Arc::new(crate::audio_native::AudioCoordinator::default()),
        }
    }
    #[test]
    fn review_v7_last_capture_retirement_preserves_files_parent() {
        let host = core();
        let controller = core();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(controller.create_invitation(60).unwrap())
            .unwrap();
        host.set_desktop_sharing(true, false, false, true).unwrap();
        let parent = controller
            .begin_connect_services(host.endpoint_id())
            .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while controller.controller_session(parent.id).unwrap().phase
            != crate::FfiSessionPhase::Active
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let parent = controller.controller_session(parent.id).unwrap();
        let files = controller
            .attach_controller_service(parent.id, parent.generation, crate::FfiServiceKind::Files)
            .unwrap();
        let kept = controller
            .service_lease(files.id, files.generation)
            .unwrap();
        let incoming = host
            .inner
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.phase == SessionPhase::Active)
            .unwrap();
        let replacement = || {
            assert!(register_host(host_key(&host.inner)));
            MacHost {
                started: Arc::new(AtomicBool::new(true)),
                core: host.inner.clone(),
                display_id: 1,
                capture_size: (1920, 1080),
                logical_size: (1920, 1080),
                stop: Arc::new(AtomicBool::new(false)),
                error: Arc::new(Mutex::new(None)),
                worker: Mutex::new(None),
                cursor: Mutex::new(HostCursorState::default()),
            }
        };
        let capture = replacement();
        capture.stop();
        let phase = host
            .inner
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == incoming.id)
            .map(|s| s.phase);
        assert_eq!(
            phase,
            Some(SessionPhase::Active),
            "capture replacement must preserve the neutral Files parent; files_cancelled={}",
            kept.is_cancelled()
        );
        let reopened = replacement();
        assert_eq!(
            host.inner
                .hosted_sessions()
                .unwrap()
                .into_iter()
                .find(|s| s.id == incoming.id)
                .unwrap()
                .phase,
            SessionPhase::Active
        );
        assert!(!kept.is_cancelled());
        reopened.stop();
        host.stop_hosted_session().unwrap();
        // Stopping is brief: the session may already be gone.
        let phase = host
            .inner
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == incoming.id)
            .map(|s| s.phase);
        assert!(
            matches!(phase, None | Some(SessionPhase::Stopping)),
            "a stopped session is stopping or gone, not {phase:?}"
        );
    }
    #[test]
    fn review_v8_sharing_disable_preserves_each_files_parent() {
        let host = core();
        let first = core();
        let second = core();
        host.prepare_mac_video().unwrap();
        host.inner.set_desktop(Box::new(Desktop)).unwrap();
        host.inner
            .set_capture(Box::new(vnilink_media::MonitorSize::new(64, 64)))
            .unwrap();
        let mut desktops = Vec::new();
        let mut files = Vec::new();
        host.start_hosting().unwrap();
        for viewer in [&first, &second] {
            viewer
                .accept_invitation(host.create_invitation(60).unwrap())
                .unwrap();
            host.accept_invitation(viewer.create_invitation(60).unwrap())
                .unwrap();
            host.set_desktop_sharing(true, true, false, true).unwrap();
            let parent = viewer.begin_connect_services(host.endpoint_id()).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while viewer.controller_session(parent.id).unwrap().phase
                != crate::FfiSessionPhase::Active
            {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(Duration::from_millis(10));
            }
            let parent = viewer.controller_session(parent.id).unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Files,
                )
                .unwrap();
            files.push(viewer.service_lease(owner.id, owner.generation).unwrap());
            viewer.prepare_mac_video().unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Desktop,
                )
                .unwrap();
            desktops.push(
                viewer
                    .open_desktop_service(owner.id, owner.generation)
                    .unwrap(),
            );
        }
        assert_eq!(host.inner.hosted_sessions().unwrap().len(), 2);
        // AppModel's sharing-disable path after source stopAll().
        host.stop_mac_desktop_sharing().unwrap();
        assert_eq!(
            host.inner
                .hosted_services()
                .unwrap()
                .iter()
                .filter(|s| s.kind == vnilink::ServiceKind::Desktop
                    && s.phase == vnilink::ServicePhase::Active)
                .count(),
            0
        );
        assert!(files.iter().all(|lease| !lease.is_cancelled()));
        let phases: Vec<_> = host
            .inner
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.phase)
            .collect();
        assert!(
            phases.iter().all(|p| *p == SessionPhase::Active),
            "Desktop sharing toggle must preserve all neutral Files parents: {phases:?}"
        );
    }
    #[test]
    fn review_v9_source_failure_preserves_healthy_desktop_and_files_parents() {
        let host = core();
        let first = core();
        let second = core();
        host.prepare_mac_video().unwrap();
        host.inner.set_desktop(Box::new(Desktop)).unwrap();
        host.inner
            .set_capture(Box::new(vnilink_media::MonitorSize::new(64, 64)))
            .unwrap();
        host.inner
            .set_topology(
                1,
                &[
                    vnilink::DisplayInfo {
                        id: 1,
                        origin_x: 0,
                        origin_y: 0,
                        width: 64,
                        height: 64,
                        rotation_deg: 0,
                        scale_percent: 100,
                        revision: 1,
                    },
                    vnilink::DisplayInfo {
                        id: 2,
                        origin_x: 64,
                        origin_y: 0,
                        width: 64,
                        height: 64,
                        rotation_deg: 0,
                        scale_percent: 100,
                        revision: 1,
                    },
                ],
            )
            .unwrap();
        let root_path = std::env::temp_dir().join(format!(
            "vnilink-v9-source-isolation-{}",
            host.endpoint_id()
        ));
        std::fs::create_dir(&root_path).unwrap();
        host.inner.set_file_roots(vec![root_path.clone()]).unwrap();
        let root_id = host.inner.file_roots().unwrap()[0].0;
        let mut pipes = Vec::new();
        let mut desktops = Vec::new();
        let mut files = Vec::new();
        host.start_hosting().unwrap();
        for (display_id, viewer) in [(1, &first), (2, &second)] {
            viewer
                .accept_invitation(host.create_invitation(60).unwrap())
                .unwrap();
            host.accept_invitation(viewer.create_invitation(60).unwrap())
                .unwrap();
            host.set_desktop_sharing(true, true, false, true).unwrap();
            let parent = viewer.begin_connect_services(host.endpoint_id()).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while viewer.controller_session(parent.id).unwrap().phase
                != crate::FfiSessionPhase::Active
            {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(Duration::from_millis(10));
            }
            let parent = viewer.controller_session(parent.id).unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Files,
                )
                .unwrap();
            files.push(viewer.service_lease(owner.id, owner.generation).unwrap());
            let pipe = viewer
                .open_files_service(owner.id, owner.generation)
                .unwrap();
            pipe.mkdir(root_id, "".into(), format!("before-{display_id}"))
                .unwrap();
            pipes.push((display_id, pipe));
            viewer.prepare_mac_video().unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Desktop,
                )
                .unwrap();
            let desktop = viewer
                .open_desktop_service(owner.id, owner.generation)
                .unwrap();
            desktop.select_displays(vec![display_id]).unwrap();
            desktops.push(desktop);
        }
        assert_eq!(host.inner.hosted_sessions().unwrap().len(), 2);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while host
            .inner
            .video_targets()
            .unwrap()
            .iter()
            .filter(|target| target.display_id != 0)
            .count()
            != 2
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        let failures = host.inner.subscribe().unwrap();
        let failed = host
            .inner
            .video_targets()
            .unwrap()
            .into_iter()
            .find(|t| t.display_id == 2)
            .unwrap();
        // The production supervisor's sourceFailed consumer uses this FFI
        // operation for open and runtime failures, never global sharing stop.
        host.fail_mac_display(2, "Display 2: secondary source failed".into())
            .unwrap();
        let services = host.inner.hosted_services().unwrap();
        let targets = host.inner.video_targets().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].display_id, 1);
        assert_eq!(
            services
                .iter()
                .filter(|service| service.kind == vnilink::ServiceKind::Desktop
                    && service.phase == vnilink::ServicePhase::Active)
                .count(),
            1
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(vnilink::CoreEvent::HostedServiceChanged { service }) = failures.try_recv()
            {
                if service.id == failed.id
                    && service.generation == failed.generation
                    && service.phase
                        == vnilink::ServicePhase::Failed(
                            "Display 2: secondary source failed".into(),
                        )
                {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "missing terminal source-failure event"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(files.iter().all(|lease| !lease.is_cancelled()));
        assert_eq!(
            services
                .iter()
                .filter(|service| service.kind == vnilink::ServiceKind::Files
                    && service.phase == vnilink::ServicePhase::Active)
                .count(),
            2
        );
        for (display_id, pipe) in &pipes {
            assert!(!pipe.is_cancelled());
            let page = pipe.list(root_id, "".into(), 0, 20).unwrap();
            assert_eq!(page.entries.len(), if *display_id == 1 { 2 } else { 3 });
            pipe.mkdir(root_id, "".into(), format!("after-{display_id}"))
                .unwrap();
        }
        assert_eq!(
            pipes[0]
                .1
                .list(root_id, "".into(), 0, 20)
                .unwrap()
                .entries
                .len(),
            4
        );
        let phases: Vec<_> = host
            .inner
            .hosted_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.phase)
            .collect();
        assert!(
            phases.iter().all(|p| *p == SessionPhase::Active),
            "Source failure must preserve all neutral Files parents: {phases:?}"
        );
        drop(pipes);
        host.shutdown();
        first.shutdown();
        second.shutdown();
        std::fs::remove_dir_all(root_path).unwrap();
    }
    #[test]
    fn review_v10_unselected_source_failure_matches_actual_routing() {
        let host = core();
        let first = core();
        let second = core();
        host.prepare_mac_video().unwrap();
        host.inner.set_desktop(Box::new(Desktop)).unwrap();
        host.inner
            .set_capture(Box::new(vnilink_media::MonitorSize::new(64, 64)))
            .unwrap();
        host.inner
            .set_topology(
                1,
                &[
                    vnilink::DisplayInfo {
                        id: 1,
                        origin_x: 0,
                        origin_y: 0,
                        width: 64,
                        height: 64,
                        rotation_deg: 0,
                        scale_percent: 100,
                        revision: 1,
                    },
                    vnilink::DisplayInfo {
                        id: 2,
                        origin_x: 64,
                        origin_y: 0,
                        width: 64,
                        height: 64,
                        rotation_deg: 0,
                        scale_percent: 100,
                        revision: 1,
                    },
                ],
            )
            .unwrap();
        let root_path = std::env::temp_dir().join(format!(
            "vnilink-v9-source-isolation-{}",
            host.endpoint_id()
        ));
        std::fs::create_dir(&root_path).unwrap();
        host.inner.set_file_roots(vec![root_path.clone()]).unwrap();
        let root_id = host.inner.file_roots().unwrap()[0].0;
        let mut pipes = Vec::new();
        let mut desktops = Vec::new();
        let mut files = Vec::new();
        host.start_hosting().unwrap();
        for (display_id, viewer) in [(1, &first), (2, &second)] {
            viewer
                .accept_invitation(host.create_invitation(60).unwrap())
                .unwrap();
            host.accept_invitation(viewer.create_invitation(60).unwrap())
                .unwrap();
            host.set_desktop_sharing(true, true, false, true).unwrap();
            let parent = viewer.begin_connect_services(host.endpoint_id()).unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(5);
            while viewer.controller_session(parent.id).unwrap().phase
                != crate::FfiSessionPhase::Active
            {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(Duration::from_millis(10));
            }
            let parent = viewer.controller_session(parent.id).unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Files,
                )
                .unwrap();
            files.push(viewer.service_lease(owner.id, owner.generation).unwrap());
            let pipe = viewer
                .open_files_service(owner.id, owner.generation)
                .unwrap();
            pipe.mkdir(root_id, "".into(), format!("before-{display_id}"))
                .unwrap();
            pipes.push((display_id, pipe));
            viewer.prepare_mac_video().unwrap();
            let owner = viewer
                .attach_controller_service(
                    parent.id,
                    parent.generation,
                    crate::FfiServiceKind::Desktop,
                )
                .unwrap();
            let desktop = viewer
                .open_desktop_service(owner.id, owner.generation)
                .unwrap();
            if display_id == 2 {
                desktop.select_displays(vec![display_id]).unwrap();
            }
            desktops.push(desktop);
        }
        assert_eq!(host.inner.hosted_sessions().unwrap().len(), 2);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while host
            .inner
            .video_targets()
            .unwrap()
            .iter()
            .filter(|target| target.display_id != 0)
            .count()
            != 1
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        let targets = host.inner.video_targets().unwrap();
        let unselected = targets
            .iter()
            .find(|target| target.display_id == 0)
            .unwrap();
        let routing = host.inner.video_routing().unwrap();
        let routed_default = routing.default_source;
        assert_eq!(routed_default, Some(2));
        assert!(super::cursor_serves_source(unselected, 2, routed_default));
        assert!(!super::cursor_serves_source(unselected, 1, routed_default));
        host.fail_mac_display(
            1,
            "Display 1 failed, unrelated to current source2 routing".into(),
        )
        .unwrap();
        let surviving = host.inner.video_targets().unwrap();
        assert_eq!(
            surviving.len(),
            2,
            "source1 failure must not cancel the unselected owner actually routed to source2"
        );
        assert!(routing
            .targets
            .iter()
            .all(|t| t.serves_source(2, routing.default_source)));
        host.fail_mac_display(2, "source2 unavailable".into())
            .unwrap();
        assert!(host.inner.video_targets().unwrap().is_empty());
        assert_eq!(host.inner.hosted_sessions().unwrap().len(), 2);
        assert!(files.iter().all(|lease| !lease.is_cancelled()));
        for (display_id, pipe) in &pipes {
            assert!(!pipe.is_cancelled());
            pipe.mkdir(root_id, "".into(), format!("after-{display_id}"))
                .unwrap();
            assert_eq!(
                pipe.list(root_id, "".into(), 0, 20).unwrap().entries.len(),
                if *display_id == 1 { 3 } else { 4 }
            );
        }
        host.shutdown();
        first.shutdown();
        second.shutdown();
        let _ = std::fs::remove_dir_all(root_path);
    }

    #[test]
    fn service_viewer_decodes_and_host_cleanup_preserves_neutral_parent() {
        let host = core();
        let controller = core();
        let unrelated = core();
        host.prepare_mac_video().unwrap();
        controller.prepare_mac_video().unwrap();
        host.inner.set_desktop(Box::new(Desktop)).unwrap();
        host.inner
            .set_capture(Box::new(vnilink_media::MonitorSize::new(64, 64)))
            .unwrap();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(controller.create_invitation(60).unwrap())
            .unwrap();
        host.set_desktop_sharing(true, true, false, false).unwrap();
        let parent = controller
            .begin_connect_services(host.endpoint_id())
            .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while controller.controller_session(parent.id).unwrap().phase
            != crate::FfiSessionPhase::Active
        {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            host_target(&host.inner).is_none(),
            "idle neutral parent must not start capture"
        );
        let parent = controller.controller_session(parent.id).unwrap();
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, crate::FfiServiceKind::Desktop)
            .unwrap();
        let handle = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        assert!(unrelated.mac_desktop_viewer(handle.clone()).is_err());
        let target = host_target(&host.inner).unwrap();
        assert!(target.service);
        let viewer = controller.mac_desktop_viewer(handle.clone()).unwrap();
        assert!(controller.mac_desktop_viewer(handle.clone()).is_err());
        let mut encoder = vnilink_macos::MacVideoEncoder::open(
            64,
            64,
            30,
            500_000,
            host.inner
                .hosted_desktop_info(target.id, target.generation)
                .unwrap()
                .codec,
            vnilink::YuvMatrix::Bt709,
        )
        .unwrap();
        let raw = RawFrame::bgra(64, 64, 0, vec![255; 64 * 64 * 4]).unwrap();
        let sample = encoder
            .encode(
                &raw,
                &EncodeParams {
                    force_keyframe: true,
                    recovery: false,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        host.publish_desktop_frame(
            target.id,
            target.generation,
            EncodedFrame::new(
                sample.codec(),
                sample.keyframe(),
                0,
                sample.bytes().to_vec(),
            )
            .unwrap()
            .into(),
        )
        .unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Some(frame) = viewer.poll() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < until,
                "service viewer did not decode"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!((frame.buffer().width, frame.buffer().height), (64, 64));
        viewer.stop();
        assert!(
            !handle.is_cancelled(),
            "stopping a decoder is not a disconnect"
        );
        let next_viewer = controller.mac_desktop_viewer(handle.clone()).unwrap();
        target.close(&host.inner); // capture/encode failure cleanup uses this path
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while !handle.is_cancelled() {
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(next_viewer.poll().is_none());
        next_viewer.stop();
        assert!(host_target(&host.inner).is_none());
        assert_eq!(
            controller.controller_session(parent.id).unwrap().phase,
            crate::FfiSessionPhase::Active
        );
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, crate::FfiServiceKind::Desktop)
            .unwrap();
        let fresh = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let replacement = host_target(&host.inner).unwrap();
        assert_ne!(replacement.stamp(), target.stamp());
        target.close(&host.inner);
        assert!(!fresh.is_cancelled());
        controller.mac_desktop_viewer(fresh).unwrap().stop();
    }
    #[test]
    fn loopback_viewer_decodes_pixels_and_releases_its_subscription() {
        let host = core();
        let controller = core();
        host.prepare_mac_video().unwrap();
        controller.prepare_mac_video().unwrap();
        host.inner.set_desktop(Box::new(Desktop)).unwrap();
        host.inner
            .set_capture(Box::new(vnilink_media::MonitorSize::new(64, 64)))
            .unwrap();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(controller.create_invitation(60).unwrap())
            .unwrap();
        host.set_desktop_sharing(true, true, false, false).unwrap();
        let session = controller.connect(host.endpoint_id()).unwrap();
        let viewer = controller
            .mac_viewer(session.id, session.generation)
            .unwrap();
        assert!(controller
            .mac_viewer(session.id, session.generation)
            .is_err());
        let mut pixels = vec![0; 64 * 64 * 4];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[2] = 255;
            pixel[3] = 255;
        }
        let raw = RawFrame::bgra(64, 64, 0, pixels).unwrap();
        let codec = host.inner.video_targets().unwrap()[0].codec;
        let mut encoder = vnilink_macos::MacVideoEncoder::open(
            64,
            64,
            30,
            500_000,
            codec,
            vnilink::YuvMatrix::Bt709,
        )
        .unwrap();
        let sample = encoder
            .encode(
                &raw,
                &EncodeParams {
                    force_keyframe: true,
                    recovery: false,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        let incoming = host.snapshot().unwrap().incoming.unwrap();
        host.publish_video_frame(
            incoming.id,
            EncodedFrame::new(
                sample.codec(),
                sample.keyframe(),
                0,
                sample.bytes().to_vec(),
            )
            .unwrap()
            .into(),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let frame = loop {
            if let Some(frame) = viewer.poll() {
                break frame;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "decoded frame did not arrive"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let buffer = frame.buffer();
        assert_eq!((buffer.width, buffer.height), (64, 64));
        let pixel = &frame.picture.pixels()[32 * buffer.stride as usize + 32 * 4..][..4];
        assert!(pixel[2] > 128 && pixel[2] > pixel[0] && pixel[2] > pixel[1]);
        assert_eq!(pixel[3], 255);
        viewer.stop();
        assert!(viewer.poll().is_none());
        let replacement = controller
            .mac_viewer(session.id, session.generation)
            .unwrap();
        replacement.stop();
        controller.disconnect().unwrap();
        assert!(controller
            .mac_viewer(session.id, session.generation)
            .is_err());
        controller.shutdown();
        host.shutdown();
    }
}

/// What a capture that stopped means for its viewers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureLoss {
    /// Start it again; the viewers wait through a short blackout.
    Restart,
    /// macOS no longer lets VniLink record the screen.
    Permission,
    /// It stopped starting at all.
    Failed,
}

/// Restarts in a row that fail to start before the viewers are told.
const FAILED_STARTS: u32 = 3;

/// Classifies a stopped capture from Screen Recording's actual state, read
/// after the failure, and how many restarts in a row did not start.
fn capture_loss(allowed: bool, failed_starts: u32) -> CaptureLoss {
    if !allowed {
        CaptureLoss::Permission
    } else if failed_starts >= FAILED_STARTS {
        CaptureLoss::Failed
    } else {
        CaptureLoss::Restart
    }
}

/// Ends the picture of each Desktop viewer this display serves, telling it
/// what failed. Viewers of other displays keep theirs.
fn fail_display_viewers(core: &vnilink::Core, display_id: u64, failure: vnilink::StreamFailure) {
    let Ok(routing) = core.video_routing() else {
        return;
    };
    for target in routing.targets {
        if target.service && target.serves_source(display_id, routing.default_source) {
            vnilink::ignored!(
                vnilink::trace::HOSTING,
                "a viewer already gone needs no failure",
                core.fail_hosted_service(target.id, target.generation, failure)
            );
        }
    }
}

#[cfg(test)]
mod capture_loss_tests {
    use super::{capture_loss, CaptureLoss, FAILED_STARTS};

    #[test]
    fn a_withdrawn_permission_ends_at_once_and_other_failures_restart_first() {
        assert_eq!(capture_loss(false, 0), CaptureLoss::Permission);
        assert_eq!(capture_loss(false, FAILED_STARTS), CaptureLoss::Permission);
        assert_eq!(capture_loss(true, 0), CaptureLoss::Restart);
        assert_eq!(capture_loss(true, FAILED_STARTS - 1), CaptureLoss::Restart);
        assert_eq!(capture_loss(true, FAILED_STARTS), CaptureLoss::Failed);
    }
}

// Capture supports the highest negotiated rate; the session encoder still
// applies its own quality limits. Starting at 30 here otherwise caps every
// later 60fps quality update at the source.
static TOPOLOGY_REVISION: AtomicU64 = AtomicU64::new(0);

fn publish_mac_topology(
    core: &vnilink::Core,
    display_id: u32,
    width: u16,
    _height: u16,
    pixel_width: u16,
    pixel_height: u16,
) -> Result<(), FfiError> {
    let revision = TOPOLOGY_REVISION.fetch_add(1, Ordering::Relaxed) + 1;
    let id = u64::from(display_id);
    match vnilink_macos::list_displays(revision) {
        Ok(list) => {
            let present = list.iter().any(|display| display.id == id);
            core.set_topology(revision, &list)
                .map_err(crate::error::map_error)?;
            if !present {
                return Err(FfiError::InvalidSetting {
                    message: "the display is not in the current topology".into(),
                });
            }
            Ok(())
        }
        Err(_) => {
            let current = core.topology().map_err(crate::error::map_error)?;
            if current.revision != 0 {
                if current.displays.iter().any(|display| display.id == id) {
                    return Ok(());
                }
                return Err(FfiError::InvalidSetting {
                    message: "the display is not in the current topology".into(),
                });
            }
            let fallback = vnilink::DisplayInfo {
                id,
                origin_x: 0,
                origin_y: 0,
                width: pixel_width,
                height: pixel_height,
                rotation_deg: 0,
                scale_percent: vnilink_macos::display_scale(
                    u32::from(pixel_width),
                    f64::from(width),
                ),
                revision,
            };
            core.set_topology(revision, &[fallback])
                .map_err(crate::error::map_error)
        }
    }
}

fn mac_capture_plan(
    display_id: u32,
    width: u16,
    height: u16,
) -> Result<vnilink_media::CapturePlan, FfiError> {
    let quality =
        vnilink::Quality::new(8_000, 60, width, height).map_err(crate::error::map_error)?;
    let plan = vnilink_media::CapturePlan::for_display(u64::from(display_id), quality)
        .map_err(|error| match error {
            vnilink_media::Error::InvalidSetting(message) => FfiError::InvalidSetting {
                message: message.to_string(),
            },
            vnilink_media::Error::Unsupported(message) => FfiError::Unsupported {
                message: message.to_string(),
            },
            other => FfiError::InvalidSetting {
                message: other.to_string(),
            },
        })?
        .with_cursor(vnilink_media::CursorPolicy::InFrame);
    Ok(plan)
}

#[cfg(test)]
mod capture_rate_tests {
    #[test]
    fn mac_capture_can_supply_the_highest_session_rate() {
        let plan = super::mac_capture_plan(1, 3840, 2160).unwrap();
        assert_eq!(plan.frame_rate(), 60);
        assert_eq!((plan.width(), plan.height()), (3840, 2160));
        assert_eq!(plan.cursor(), vnilink_media::CursorPolicy::InFrame);
    }
}

fn cursor_serves_source(target: &vnilink::VideoTarget, source: u64, default: Option<u64>) -> bool {
    target.serves_source(source, default)
}

/// CGEvent locations and display origins are logical desktop points; frame
/// and cursor coordinates at the receiver are pixels in the encoded source.
fn map_source_cursor(
    current: &vnilink::CursorState,
    display: &vnilink::DisplayInfo,
    capture_size: (u16, u16),
    _logical_size: (u16, u16),
    quality: vnilink::Quality,
) -> Result<vnilink::CursorState, FfiError> {
    let (width, height) = vnilink_media::encoded_size(
        capture_size.0,
        capture_size.1,
        quality.max_width,
        quality.max_height,
    )
    .map_err(|_| FfiError::Unsupported {
        message: "cursor source dimensions are invalid".into(),
    })?;
    let scale = f64::from(display.scale_percent.max(1)) / 100.0;
    let logical_size = (
        f64::from(display.width) / scale,
        f64::from(display.height) / scale,
    );
    let sx = f64::from(width) / logical_size.0;
    let sy = f64::from(height) / logical_size.1;
    let x = f64::from(current.position.0) - f64::from(display.origin_x);
    let y = f64::from(current.position.1) - f64::from(display.origin_y);
    let visible =
        current.visible && x >= 0.0 && y >= 0.0 && x < logical_size.0 && y < logical_size.1;
    let image = current
        .image
        .as_ref()
        .map(|image| {
            let w = (f64::from(image.width()) * sx).round().clamp(1.0, 512.0) as u16;
            let h = (f64::from(image.height()) * sy).round().clamp(1.0, 512.0) as u16;
            let mut bgra = vec![0; usize::from(w) * usize::from(h) * 4];
            for row in 0..usize::from(h) {
                for col in 0..usize::from(w) {
                    let src = ((row * usize::from(image.height()) / usize::from(h))
                        * usize::from(image.width())
                        + col * usize::from(image.width()) / usize::from(w))
                        * 4;
                    let dest = (row * usize::from(w) + col) * 4;
                    bgra[dest..dest + 4].copy_from_slice(&image.pixels()[src..src + 4]);
                }
            }
            vnilink::CursorImage::new(
                w,
                h,
                (
                    (f64::from(image.hotspot().0) * sx)
                        .round()
                        .clamp(0.0, f64::from(w - 1)) as u16,
                    (f64::from(image.hotspot().1) * sy)
                        .round()
                        .clamp(0.0, f64::from(h - 1)) as u16,
                ),
                bgra,
            )
            .map(Arc::new)
            .map_err(crate::error::map_error)
        })
        .transpose()?;
    Ok(vnilink::CursorState {
        position: ((x * sx).round() as i32, (y * sy).round() as i32),
        visible,
        image,
    })
}

/// Sends `update` to every viewer of `display_id` that draws the pointer
/// itself, in its own picture's pixels; `cache` keeps the last shape.
fn publish_host_cursor(
    core: &Core,
    display_id: u64,
    capture_size: (u16, u16),
    logical_size: (u16, u16),
    cache: &Mutex<HostCursorState>,
    update: MacCursorUpdate,
) -> Result<(), FfiError> {
    let routing = match core.video_routing() {
        Ok(targets) => targets,
        Err(vnilink::Error::ShutDown) => return Ok(()),
        Err(error) => return Err(crate::error::map_error(error)),
    };
    let topology = core.topology().map_err(crate::error::map_error)?;
    let Some(display) = topology.displays.iter().find(|d| d.id == display_id) else {
        return Ok(());
    };
    let default_id = routing.default_source;
    let targets = routing.targets;
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let current = merge_host_cursor(cache.state.as_ref(), update)?;
    cache.state = Some(current.clone());
    drop(cache);
    // Every consumer uses the same source as its frame producer. A host
    // for another display must never overwrite this consumer's cursor.
    for target in targets.iter().filter(|target| {
        target.separate_cursor && cursor_serves_source(target, display_id, default_id)
    }) {
        let mapped = map_source_cursor(
            &current,
            display,
            capture_size,
            logical_size,
            target.quality,
        )?;
        let result = if target.service {
            core.publish_desktop_cursor(target.id, target.generation, mapped)
        } else {
            core.publish_cursor(target.id, target.generation, mapped)
        };
        match result {
            Ok(()) | Err(vnilink::Error::NoSession | vnilink::Error::StaleSession) => {}
            Err(error) => return Err(crate::error::map_error(error)),
        }
    }
    Ok(())
}

/// Reads the system pointer for viewers that draw it, until `done`: on each
/// move or new shape, and twice a second regardless, so a viewer that just
/// joined receives it without waiting for the mouse to move.
fn follow_pointer(
    core: &Core,
    display_id: u64,
    capture_size: (u16, u16),
    logical_size: (u16, u16),
    done: &AtomicBool,
) {
    const EVERY: Duration = Duration::from_millis(8);
    const REFRESH: Duration = Duration::from_millis(500);
    let cache = Mutex::new(HostCursorState::default());
    let mut shape: Option<vnilink_macos::PointerImage> = None;
    let mut position = None;
    let mut sent = std::time::Instant::now() - REFRESH;
    while !done.load(Ordering::Acquire) {
        let pointer = vnilink_macos::system_pointer();
        let at = (pointer.x.round() as i32, pointer.y.round() as i32);
        let image = pointer.image.filter(|image| {
            shape.as_ref().is_none_or(|old| {
                (old.width, old.height, old.hotspot, &old.bgra)
                    != (image.width, image.height, image.hotspot, &image.bgra)
            })
        });
        if image.is_some() || position != Some(at) || sent.elapsed() >= REFRESH {
            let update = MacCursorUpdate {
                x: at.0,
                y: at.1,
                visible: true,
                image: image.as_ref().map(|image| MacCursorImage {
                    width: image.width,
                    height: image.height,
                    hotspot_x: image.hotspot.0,
                    hotspot_y: image.hotspot.1,
                    bgra: image.bgra.clone(),
                }),
            };
            // A shape the engine refuses is skipped; the next one may fit.
            let _ =
                publish_host_cursor(core, display_id, capture_size, logical_size, &cache, update);
            position = Some(at);
            sent = std::time::Instant::now();
            if image.is_some() {
                shape = image;
            }
        }
        std::thread::sleep(EVERY);
    }
}

fn merge_host_cursor(
    previous: Option<&vnilink::CursorState>,
    update: MacCursorUpdate,
) -> Result<vnilink::CursorState, FfiError> {
    let image = match update.image {
        Some(image) => Some(Arc::new(
            vnilink::CursorImage::new(
                image.width,
                image.height,
                (image.hotspot_x, image.hotspot_y),
                image.bgra,
            )
            .map_err(crate::error::map_error)?,
        )),
        None => previous.and_then(|state| state.image.clone()),
    };
    Ok(vnilink::CursorState {
        position: (update.x, update.y),
        visible: update.visible,
        image,
    })
}

#[cfg(test)]
mod host_cursor_tests {
    use super::*;
    #[test]
    fn cursor_routes_selected_and_default_consumers_like_video() {
        let mut target = vnilink::VideoTarget {
            service: true,
            id: 1,
            generation: 1,
            quality: vnilink::Quality::new(4000, 30, 1920, 1080).unwrap(),
            separate_cursor: true,
            codec: vnilink::CODEC_H264,
            matrix: vnilink::YuvMatrix::Bt709,
            display_id: 2,
            revision: 1,
        };
        assert!(cursor_serves_source(&target, 2, Some(1)));
        assert!(!cursor_serves_source(&target, 1, Some(1)));
        target.display_id = 0;
        assert!(cursor_serves_source(&target, 1, Some(1)));
        assert!(!cursor_serves_source(&target, 2, Some(1)));
        assert!(cursor_serves_source(&target, 2, None));
    }
    #[test]
    fn cursor_is_source_local_scaled_and_hidden_outside_each_display() {
        let display = |id, origin_x, scale_percent, width| vnilink::DisplayInfo {
            id,
            origin_x,
            origin_y: 0,
            width,
            height: 1080,
            rotation_deg: 0,
            scale_percent,
            revision: 1,
        };
        let global = vnilink::CursorState {
            position: (1930, 10),
            visible: true,
            image: Some(Arc::new(
                vnilink::CursorImage::new(2, 2, (1, 1), vec![255; 16]).unwrap(),
            )),
        };
        let q = vnilink::Quality::new(4000, 30, 1920, 1080).unwrap();
        let primary = map_source_cursor(
            &global,
            &display(1, 0, 100, 1920),
            (1920, 1080),
            (1920, 1080),
            q,
        )
        .unwrap();
        assert!(!primary.visible);
        let secondary = display(2, 1920, 200, 3840);
        let control = map_source_cursor(
            &global,
            &secondary,
            (3840, 1080),
            (1920, 540),
            vnilink::Quality::new(4000, 30, 3840, 1080).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (control.position.0, control.position.1, control.visible),
            (20, 20, true)
        );
        assert_eq!(control.image.as_ref().unwrap().width(), 4);
        let view = map_source_cursor(&global, &secondary, (3840, 1080), (1920, 540), q).unwrap();
        assert_eq!(
            (view.position.0, view.position.1, view.visible),
            (10, 10, true)
        );
        assert_eq!(view.image.as_ref().unwrap().width(), 2);
        assert_eq!(view.image.as_ref().unwrap().hotspot().0, 1);
    }
    #[test]
    fn host_cursor_retains_shape_for_motion_and_validates_replacements() {
        let first = merge_host_cursor(
            None,
            MacCursorUpdate {
                x: 10,
                y: 20,
                visible: true,
                image: Some(MacCursorImage {
                    width: 2,
                    height: 2,
                    hotspot_x: 1,
                    hotspot_y: 0,
                    bgra: vec![64; 16],
                }),
            },
        )
        .unwrap();
        let moved = merge_host_cursor(
            Some(&first),
            MacCursorUpdate {
                x: 30,
                y: 40,
                visible: true,
                image: None,
            },
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            first.image.as_ref().unwrap(),
            moved.image.as_ref().unwrap()
        ));
        assert_eq!(moved.position, (30, 40));
        let hidden = merge_host_cursor(
            Some(&moved),
            MacCursorUpdate {
                x: 30,
                y: 40,
                visible: false,
                image: None,
            },
        )
        .unwrap();
        assert!(!hidden.visible);
        assert!(merge_host_cursor(
            Some(&first),
            MacCursorUpdate {
                x: 0,
                y: 0,
                visible: true,
                image: Some(MacCursorImage {
                    width: 2,
                    height: 2,
                    hotspot_x: 2,
                    hotspot_y: 0,
                    bgra: vec![64; 16]
                })
            }
        )
        .is_err());
        assert_eq!(first.image.unwrap().pixels(), &[64; 16]);
    }
}

#[cfg(test)]
mod review_v8_cursor_mode_change {
    use super::*;
    #[test]
    fn review_v8_cursor_uses_live_logical_extent_after_mode_change() {
        let live = vnilink::DisplayInfo {
            id: 1,
            origin_x: 0,
            origin_y: 0,
            width: 1280,
            height: 720,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 2,
        };
        let current = vnilink::CursorState {
            position: (640, 360),
            visible: true,
            image: None,
        };
        let mapped = map_source_cursor(
            &current,
            &live,
            (1920, 1080),
            (1920, 1080),
            vnilink::Quality::new(8000, 60, 1920, 1080).unwrap(),
        )
        .unwrap();
        // Capture keeps its fixed 1920x1080 output while the full live
        // display is now 1280x720 logical points. Its center stays center.
        assert_eq!(
            mapped.position,
            (960, 540),
            "live-source center must match encoded-picture center"
        );
    }
}
