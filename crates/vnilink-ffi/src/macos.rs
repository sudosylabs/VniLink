//! VideoToolbox stays on a Rust worker. Swift polls one latest decoded picture.
use crate::{FfiError, VnilinkCore};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use vnilink::{Core, EncodedFrame, SessionPhase};
use vnilink_media::{
    ControllerOutput, ControllerVideo, EncodeParams, RawFrame, VideoDecoder, VideoEncoder,
};

#[derive(uniffi::Record)]
pub struct MacFrameBuffer {
    pub width: u16,
    pub height: u16,
    pub stride: u32,
    pub address: u64,
    pub byte_count: u64,
}

/// Immutable decoded pixels owned by this in-process object. The native image
/// provider retains the object until it finishes drawing; pixels never travel
/// through UniFFI's byte-by-byte record serialization.
#[derive(uniffi::Object)]
pub struct MacFrame {
    picture: RawFrame,
}

#[uniffi::export]
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
}

#[derive(uniffi::Record)]
pub struct MacCursorImage {
    pub width: u16,
    pub height: u16,
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    pub bgra: Vec<u8>,
}
#[derive(uniffi::Record)]
pub struct MacCursorUpdate {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub image: Option<MacCursorImage>,
}

struct ViewerState {
    frame: Option<Arc<MacFrame>>,
    error: Option<String>,
}

#[derive(uniffi::Object)]
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

#[uniffi::export]
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
            let _ = worker.join();
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
fn probe() -> Result<(), FfiError> {
    let result = (|| -> Result<(), vnilink_media::Error> {
        let mut encoder = vnilink_macos::MacEncoder::open(64, 64, 30, 500_000, false)?;
        let raw = RawFrame::bgra(64, 64, 0, vec![255; 64 * 64 * 4])?;
        let sample = encoder.encode(
            &raw,
            &EncodeParams {
                force_keyframe: true,
                bitrate_bps: 500_000,
                frame_rate: 30,
            },
        )?;
        let frame = EncodedFrame::h264(sample.keyframe(), 0, sample.bytes().to_vec())
            .map_err(|_| vnilink_media::Error::Unsupported("VideoToolbox emitted invalid H.264"))?;
        let picture = vnilink_macos::MacDecoder::new().decode(&frame)?;
        if picture.width() != 64 || picture.height() != 64 {
            return Err(vnilink_media::Error::Unsupported(
                "VideoToolbox returned invalid dimensions",
            ));
        }
        Ok(())
    })();
    result.map_err(|_| FfiError::Unsupported {
        message: "VideoToolbox H.264 is unavailable on this Mac".into(),
    })
}

#[uniffi::export]
impl VnilinkCore {
    pub fn prepare_mac_video(&self) -> Result<(), FfiError> {
        probe()?;
        self.inner
            .set_cursor_receiver(true)
            .map_err(crate::error::map_error)?;
        self.inner
            .set_h264_support(true, true)
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
            let mut ticks = 0u32;
            while !worker_stop.load(Ordering::Acquire)
                && current(&core, session_id, generation)
                && !worker_desktop
                    .as_ref()
                    .is_some_and(|desktop| desktop.is_cancelled())
            {
                let Some(frame) = subscription.recv_timeout(Duration::from_millis(100)) else {
                    // Keep adapting on idle too: a silent path still reports RTT.
                    ticks = ticks.saturating_add(1);
                    if ticks % 20 == 0 && worker_desktop.is_none() {
                        let _ = core.auto_quality_tick(session_id);
                    }
                    continue;
                };
                ticks = ticks.saturating_add(1);
                if ticks % 20 == 0 && worker_desktop.is_none() {
                    let _ = core.auto_quality_tick(session_id);
                }
                let result = decoder.push(&frame);
                match result {
                    Ok(ControllerOutput::Decoded(picture)) => {
                        if !current(&core, session_id, generation)
                            || worker_desktop
                                .as_ref()
                                .is_some_and(|desktop| desktop.is_cancelled())
                        {
                            break;
                        }
                        worker_state.lock().unwrap_or_else(|e| e.into_inner()).frame =
                            Some(Arc::new(MacFrame { picture }));
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
        super::probe().unwrap();
    }
}

/// Sharing configuration with a capture resource opened for each Desktop instance.
/// Dropping it stops capture and the hosted session.
#[derive(uniffi::Object)]
pub struct MacHost {
    core: Arc<Core>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    cursor: Mutex<HostCursorState>,
}

#[derive(Default)]
struct HostCursorState {
    state: Option<vnilink::CursorState>,
    session: Option<(bool, u64, u64)>,
}

struct HostTarget {
    service: bool,
    id: u64,
    generation: u64,
    separate: bool,
    quality: vnilink::Quality,
}
impl HostTarget {
    fn stamp(&self) -> (bool, u64, u64) {
        (self.service, self.id, self.generation)
    }
    fn close(&self, core: &Core) {
        if self.service {
            let _ = core.close_hosted_service(self.id, self.generation);
        } else if core
            .snapshot()
            .ok()
            .and_then(|s| s.incoming)
            .is_some_and(|s| {
                s.id == self.id
                    && s.generation == self.generation
                    && s.phase == SessionPhase::Active
            })
        {
            let _ = core.stop_hosted_session();
        }
    }
}
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
            separate: info.separate_cursor,
            quality: info.quality,
        });
    }
    let session = core
        .snapshot()
        .ok()?
        .incoming
        .filter(|s| s.phase == SessionPhase::Active)?;
    Some(HostTarget {
        service: false,
        id: session.id,
        generation: session.generation,
        separate: core
            .separate_cursor(session.id, vnilink::SessionRole::Host)
            .unwrap_or(false),
        quality: session.quality?,
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
}

#[uniffi::export]
impl MacHost {
    /// Cursor updates never inject input. Retain a shape across position-only
    /// samples and resend it when a new hosted generation negotiates metadata.
    pub fn update_cursor(&self, update: MacCursorUpdate) -> Result<(), FfiError> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let target = host_target(&self.core);
        let stamp = target.as_ref().map(HostTarget::stamp);
        let mut cache = self.cursor.lock().unwrap_or_else(|e| e.into_inner());
        let current = merge_host_cursor(cache.state.as_ref(), update)?;
        let changed = cache.state.as_ref() != Some(&current) || cache.session != stamp;
        cache.state = Some(current.clone());
        cache.session = stamp;
        drop(cache);
        if changed {
            if let Some(target) = target.filter(|target| target.separate) {
                let result = if target.service {
                    self.core
                        .publish_desktop_cursor(target.id, target.generation, current)
                } else {
                    self.core
                        .publish_cursor(target.id, target.generation, current)
                };
                match result {
                    Ok(()) | Err(vnilink::Error::NoSession | vnilink::Error::StaleSession) => {}
                    Err(error) => return Err(crate::error::map_error(error)),
                }
            }
        }
        Ok(())
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn stop(&self) {
        if self.stop.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.core.stop_hosted_session();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
        let _ = self.core.set_h264_support(false, true);
    }
}
impl Drop for MacHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[uniffi::export]
impl VnilinkCore {
    /// Call only after the user enables sharing and grants Screen Recording.
    /// Input coordinates use main-display logical points. Capture dimensions
    /// use backing pixels so Retina detail is preserved before encoding.
    pub fn mac_host(
        &self,
        display_id: u32,
        width: u16,
        height: u16,
        pixel_width: u16,
        pixel_height: u16,
        cursor_supported: bool,
    ) -> Result<MacHost, FfiError> {
        if width < 2 || height < 2 || pixel_width < 2 || pixel_height < 2 {
            return Err(FfiError::InvalidSetting {
                message: "No main display is available".into(),
            });
        }
        let core = Arc::clone(&self.inner);
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let worker_core = Arc::clone(&core);
        let worker_stop = Arc::clone(&stop);
        let worker_error = Arc::clone(&error);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let opened = (|| {
                let plan = mac_capture_plan(pixel_width, pixel_height)?;
                let plan = vnilink_macos::ScreenPlan::from_capture(&plan).map_err(|_| {
                    FfiError::Unsupported {
                        message: "This display cannot be captured".into(),
                    }
                })?;
                worker_core
                    .set_capture(Box::new(MacCaptureDescriptor {
                        width,
                        height,
                        cursor: cursor_supported,
                    }))
                    .map_err(crate::error::map_error)?;
                worker_core
                    .set_h264_support(true, true)
                    .map_err(crate::error::map_error)?;
                // VideoToolbox encodes High profile, so this host negotiates
                // it when the controller offers it.
                worker_core
                    .set_h264_high(true)
                    .map_err(crate::error::map_error)?;
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
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let mut served = None;
            while !worker_stop.load(Ordering::Acquire) {
                let target = host_target(&worker_core);
                if let Some(target) = target.filter(|target| served != Some(target.stamp())) {
                    served = Some(target.stamp());
                    *worker_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    // The OS stream belongs to this instance, not the sharing switch.
                    let result = (|| -> Result<(), vnilink_media::PublishError> {
                        let capture =
                            vnilink_macos::ScreenCapture::start_display(&plan, Some(display_id))?;
                        capture.set_shows_cursor(!target.separate)?;
                        if target.service {
                            vnilink_macos::serve_desktop(
                                &capture,
                                &worker_core,
                                target.id,
                                target.generation,
                                &worker_stop,
                            )
                        } else {
                            vnilink_macos::serve_stream(
                                &capture,
                                &worker_core,
                                target.id,
                                target.quality,
                                &worker_stop,
                            )
                        }
                        // capture drops here even when this owner failed or closed.
                    })();
                    if result.is_err() {
                        *worker_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                            "Screen sharing stopped. Check Screen Recording and try connecting again.".into());
                        target.close(&worker_core);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = worker_core.set_h264_support(false, true);
        });
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(MacHost {
                core,
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
        }
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
        host.authorize(controller.endpoint_id(), true, false, false)
            .unwrap();
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
        let mut encoder = vnilink_macos::MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
        let raw = RawFrame::bgra(64, 64, 0, vec![255; 64 * 64 * 4]).unwrap();
        let sample = encoder
            .encode(
                &raw,
                &EncodeParams {
                    force_keyframe: true,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        host.publish_desktop_video(
            target.id,
            target.generation,
            true,
            0,
            sample.bytes().to_vec(),
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
        host.authorize(controller.endpoint_id(), true, false, false)
            .unwrap();
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
        let mut encoder = vnilink_macos::MacEncoder::open(64, 64, 30, 500_000, false).unwrap();
        let sample = encoder
            .encode(
                &raw,
                &EncodeParams {
                    force_keyframe: true,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )
            .unwrap();
        let incoming = host.snapshot().unwrap().incoming.unwrap();
        host.publish_video(incoming.id, true, 0, sample.bytes().to_vec())
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

// Capture supports the highest negotiated rate; the session encoder still
// applies its own quality limits. Starting at 30 here otherwise caps every
// later 60fps quality update at the source.
fn mac_capture_plan(width: u16, height: u16) -> Result<vnilink_media::CapturePlan, FfiError> {
    let quality =
        vnilink::Quality::new(8_000, 60, width, height).map_err(crate::error::map_error)?;
    let plan = vnilink_media::CapturePlan::one_monitor(quality)
        .map_err(|_| FfiError::Unsupported {
            message: "This display cannot be captured".into(),
        })?
        .with_cursor(vnilink_media::CursorPolicy::InFrame);
    Ok(plan)
}

#[cfg(test)]
mod capture_rate_tests {
    #[test]
    fn mac_capture_can_supply_the_highest_session_rate() {
        let plan = super::mac_capture_plan(3840, 2160).unwrap();
        assert_eq!(plan.frame_rate(), 60);
        assert_eq!((plan.width(), plan.height()), (3840, 2160));
        assert_eq!(plan.cursor(), vnilink_media::CursorPolicy::InFrame);
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
