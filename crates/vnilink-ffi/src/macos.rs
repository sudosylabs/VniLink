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
pub struct MacFrame {
    pub width: u16,
    pub height: u16,
    pub stride: u32,
    pub bgra: Vec<u8>,
}

struct ViewerState {
    frame: Option<MacFrame>,
    error: Option<String>,
}

#[derive(uniffi::Object)]
pub struct MacViewer {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<ViewerState>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[uniffi::export]
impl MacViewer {
    /// Takes the latest frame; old pictures never accumulate behind the UI.
    pub fn poll(&self) -> Option<MacFrame> {
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
    core.snapshot()
        .ok()
        .and_then(|s| s.outgoing)
        .is_some_and(|s| {
            s.id == id && s.generation == generation && s.phase == SessionPhase::Active
        })
}

/// Probe real encode/decode with synthetic pixels; no screen permission needed.
fn probe() -> Result<(), FfiError> {
    let result = (|| -> Result<(), vnilink_media::Error> {
        let mut encoder = vnilink_macos::MacEncoder::open(64, 64, 30, 500_000)?;
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
            .set_h264_support(true, true)
            .map_err(crate::error::map_error)
    }
    pub fn mac_viewer(&self, session_id: u64, generation: u64) -> Result<MacViewer, FfiError> {
        if !current(&self.inner, session_id, generation) {
            return Err(FfiError::InvalidSetting {
                message: "The desktop session is no longer active".into(),
            });
        }
        let subscription = self
            .inner
            .subscribe_video()
            .map_err(crate::error::map_error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(ViewerState {
            frame: None,
            error: None,
        }));
        let core = Arc::clone(&self.inner);
        let worker_stop = Arc::clone(&stop);
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            let mut decoder = ControllerVideo::new(vnilink_macos::MacDecoder::new());
            while !worker_stop.load(Ordering::Acquire) && current(&core, session_id, generation) {
                let Some(frame) = subscription.recv_timeout(Duration::from_millis(100)) else {
                    continue;
                };
                match decoder.push(&frame) {
                    Ok(ControllerOutput::Decoded(picture)) => {
                        if !current(&core, session_id, generation) {
                            break;
                        }
                        let mut bgra = picture.pixels().to_vec();
                        // Desktop buffers are opaque even when the capture source leaves alpha zero.
                        for pixel in bgra.chunks_exact_mut(4) {
                            pixel[3] = 255;
                        }
                        worker_state.lock().unwrap_or_else(|e| e.into_inner()).frame =
                            Some(MacFrame {
                                width: picture.width(),
                                height: picture.height(),
                                stride: picture.stride() as u32,
                                bgra,
                            });
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
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn mac_video_probe_uses_synthetic_pixels() {
        super::probe().unwrap();
    }
}

/// Explicitly enabled capture. Dropping it stops capture and the hosted session.
#[derive(uniffi::Object)]
pub struct MacHost {
    core: Arc<Core>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
#[uniffi::export]
impl MacHost {
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
    /// Coordinates are main-display logical points, matching macOS input.
    pub fn mac_host(&self, display_id: u32, width: u16, height: u16) -> Result<MacHost, FfiError> {
        if width < 2 || height < 2 {
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
                let quality = vnilink::Quality::new(8_000, 30, width, height)
                    .map_err(crate::error::map_error)?;
                let plan = vnilink_media::CapturePlan::one_monitor(quality)
                    .map_err(|_| FfiError::Unsupported {
                        message: "This display cannot be captured".into(),
                    })?
                    .with_cursor(vnilink_media::CursorPolicy::InFrame);
                let plan = vnilink_macos::ScreenPlan::from_capture(&plan).map_err(|_| {
                    FfiError::Unsupported {
                        message: "This display cannot be captured".into(),
                    }
                })?;
                let capture = vnilink_macos::ScreenCapture::start_display(&plan, Some(display_id))
                    .map_err(|_| FfiError::Unsupported { message: "Screen sharing could not start. Check Screen Recording in System Settings.".into() })?;
                worker_core
                    .set_capture(Box::new(vnilink_media::MonitorSize::new(width, height)))
                    .map_err(crate::error::map_error)?;
                worker_core
                    .set_h264_support(true, true)
                    .map_err(crate::error::map_error)?;
                Ok::<_, FfiError>(capture)
            })();
            let capture = match opened {
                Ok(capture) => {
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    capture
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            while !worker_stop.load(Ordering::Acquire) {
                let Ok(snapshot) = worker_core.snapshot() else {
                    break;
                };
                if let Some(session) = snapshot
                    .incoming
                    .filter(|s| s.phase == SessionPhase::Active)
                {
                    if let Some(quality) = session.quality {
                        if vnilink_macos::serve_stream(
                            &capture,
                            &worker_core,
                            session.id,
                            quality,
                            &worker_stop,
                        )
                        .is_err()
                        {
                            *worker_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                                "Screen sharing stopped. Disable and enable it to try again."
                                    .into(),
                            );
                            let _ = worker_core.stop_hosted_session();
                            break;
                        }
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
        let mut encoder = vnilink_macos::MacEncoder::open(64, 64, 30, 500_000).unwrap();
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
        assert_eq!((frame.width, frame.height), (64, 64));
        let pixel = &frame.bgra[32 * frame.stride as usize + 32 * 4..][..4];
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
