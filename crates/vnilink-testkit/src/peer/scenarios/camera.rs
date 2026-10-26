//! `camera`: the agent shares a stand-in camera (`crate::camera`) and,
//! once the suite opens it, sends the pictures `params` ask for at a steady
//! rate, each stamped with the agent's clock when sent
//! (`EncodedFrame::timestamp_us`), so the suite times each picture's way
//! across on its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::camera::HostCameraFactory;
use vnilink::{EncodedFrame, Grants, ServiceKind};

use super::{released, whole, Scenario, NOTHING};
use crate::camera::StandInCameras;
use crate::peer::clock;
use crate::Engine;

/// How long the last scenario's viewer may take to go.
const RELEASE_WAIT: Duration = Duration::from_secs(10);
/// How long the suite may take to open the camera.
const OPEN_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct CameraHost {
    stop: Arc<AtomicBool>,
    /// Sends the pictures; its result is how many it sent.
    sender: Option<JoinHandle<Result<u64, String>>>,
}

impl Scenario for CameraHost {
    fn start(&mut self, host: &Engine, params: &Value) -> Result<Value, String> {
        let pictures = whole(params, "pictures", 1..=10_000, 150)?;
        let bytes = usize::try_from(whole(params, "bytes", 1..=4 * 1024 * 1024, 30 * 1024)?)
            .map_err(|error| error.to_string())?;
        let every = Duration::from_secs(1)
            / u32::try_from(whole(params, "fps", 1..=120, 30)?).unwrap_or(30);
        released(host, RELEASE_WAIT)?;
        let cameras = Arc::new(StandInCameras::default());
        host.core
            .set_camera_factory(Some(Arc::clone(&cameras) as Arc<dyn HostCameraFactory>))
            .map_err(|error| error.to_string())?;
        host.core
            .set_service_sharing(ServiceKind::Camera, true)
            .map_err(|error| error.to_string())?;
        host.core
            .set_desktop_sharing(Grants {
                view: true,
                ..NOTHING
            })
            .map_err(|error| error.to_string())?;
        self.stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&self.stop);
        self.sender = Some(std::thread::spawn(move || {
            let opened = Instant::now();
            let source = loop {
                if let Some(source) = crate::lock(&cameras.sources).last().cloned() {
                    break source;
                }
                if stop.load(Ordering::Acquire) || opened.elapsed() > OPEN_WAIT {
                    return Err("the suite never opened the camera".to_owned());
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let started = Instant::now();
            for index in 0..pictures {
                if stop.load(Ordering::Acquire) {
                    return Ok(index);
                }
                let frame = EncodedFrame::h264(index % 30 == 0, clock::now_us(), vec![7; bytes])
                    .map_err(|error| error.to_string())?;
                source.publish(frame).map_err(|error| error.to_string())?;
                let next = started + every * u32::try_from(index + 1).unwrap_or(u32::MAX);
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
            }
            Ok(pictures)
        }));
        Ok(json!({ "pictures": pictures }))
    }

    fn wait(&mut self, _host: &Engine, what: &str, within: Duration) -> Result<Value, String> {
        if what != "sent" {
            return Err(format!("the camera scenario cannot wait for {what:?}"));
        }
        let started = Instant::now();
        while self
            .sender
            .as_ref()
            .is_some_and(|sender| !sender.is_finished())
        {
            if started.elapsed() > within {
                return Err(format!(
                    "the pictures were not all sent within {} s",
                    within.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let sender = self.sender.take().ok_or("the camera scenario sends once")?;
        let sent = sender
            .join()
            .map_err(|_| "the camera's sender panicked".to_owned())??;
        Ok(json!({ "sent": sent }))
    }

    fn stop(&mut self, host: &Engine) {
        self.stop.store(true, Ordering::Release);
        if let Some(sender) = self.sender.take() {
            let _ = sender.join();
        }
        for result in [
            host.core.set_desktop_sharing(NOTHING),
            host.core.set_service_sharing(ServiceKind::Camera, false),
        ] {
            if let Err(error) = result {
                eprintln!("the agent could not stop sharing: {error}");
            }
        }
    }
}
