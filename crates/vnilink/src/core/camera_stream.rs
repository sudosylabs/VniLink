//! Independently owned camera transport, using the shared H.264 frame format.
// trace: records every ignored failure
use super::service_stream::{preamble, read, status, write};
use super::service_stream::{HostedEnd, HostedEnding};
use super::*;
use crate::{
    availability::Capability,
    camera::*,
    video::{VideoInbox, VideoPush},
    CloseCause, ServiceKind, ServiceLease, ServicePhase,
};
use iroh::endpoint::{RecvStream, SendStream};
use std::time::Instant;
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a viewer whose requests failed still reads for the host's
/// last word, which a host ending the stream sends after it stops reading.
const LAST_WORD: Duration = Duration::from_secs(1);
const LIST: u8 = 2;
/// Last message on a camera stream: the host closed it, its capture failed,
/// or it withdrew the camera (access revoked or its camera setting changed).
const END_CLOSED: u8 = 0;
const END_FAILED: u8 = 2;
const END_WITHDRAWN: u8 = 3;
/// `[RECORDING, allowed]`: whether the host lets this viewer record the
/// camera. Sent before the first picture and whenever its user changes it.
const RECORDING: u8 = 4;
/// From the viewer: `[KEYFRAME]` asks for a keyframe, and `[RECEIVED,
/// number]` says it received every picture up to `number` (big-endian
/// u32), which paces what the host writes (`InFlight`).
const KEYFRAME: u8 = 1;
const RECEIVED: u8 = 2;

pub struct RemoteCameraService {
    config: CameraConfig,
    lease: ServiceLease,
    stop: tokio::sync::watch::Sender<bool>,
    inbox: Arc<VideoInbox>,
    end: Arc<std::sync::Mutex<Option<CameraEnd>>>,
    /// The host's recording consent. Its lock also orders tap creation
    /// against revocation, so no tap outlives a withdrawn consent.
    recording: Arc<std::sync::Mutex<bool>>,
    keyframe: Arc<tokio::sync::Notify>,
}
impl RemoteCameraService {
    /// Whether the host currently lets this viewer record the camera.
    pub fn recording_allowed(&self) -> bool {
        *lock(&self.recording)
    }
    /// A bounded copy of the encoded pictures for a local recording, starting
    /// at a keyframe the host is asked for. It closes when the host withdraws
    /// recording consent or the stream ends; the viewer keeps its pictures.
    pub fn recording_video_tap(&self) -> Result<crate::RecordingVideoTap, Error> {
        let allowed = lock(&self.recording);
        if !*allowed {
            return Err(Error::Unauthorized);
        }
        let tap = self.inbox.recording_tap()?;
        drop(allowed);
        self.keyframe.notify_one();
        Ok(tap)
    }
    /// Why the stream ended, after the frames sent before the end. Reported
    /// once; `None` while it runs. When reported, this service has finished
    /// and no longer counts as the connection's camera owner.
    pub fn take_end(&self) -> Option<CameraEnd> {
        lock(&self.end).take()
    }
    pub fn config(&self) -> CameraConfig {
        self.config.clone()
    }
    pub fn info(&self) -> &crate::ServiceInfo {
        self.lease.info()
    }
    pub fn is_cancelled(&self) -> bool {
        self.lease.is_cancelled()
    }
    pub fn try_recv(&self) -> Option<crate::video::EncodedFrame> {
        self.inbox.try_pop()
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Option<crate::video::EncodedFrame> {
        self.inbox.pop_timeout(timeout)
    }
    pub fn lagged(&self) -> u64 {
        self.inbox.dropped()
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
        self.inbox.close();
    }
}
impl Drop for RemoteCameraService {
    fn drop(&mut self) {
        self.close();
    }
}
impl Core {
    pub fn set_camera_factory(
        &self,
        factory: Option<Arc<dyn HostCameraFactory>>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        self.replace_camera_factory(factory, false)
    }
    /// Stops offering a camera this computer has: paired computers read it
    /// as turned off, not as missing.
    pub fn turn_off_camera(&self) -> Result<(), Error> {
        let _engine = self.engine();
        self.replace_camera_factory(None, true)
    }
    fn replace_camera_factory(
        &self,
        factory: Option<Arc<dyn HostCameraFactory>>,
        turned_off: bool,
    ) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        *lock(&self.shared.camera_factory) = factory;
        self.shared
            .camera_turned_off
            .store(turned_off, std::sync::atomic::Ordering::Release);
        for record in state
            .hosted_services
            .values_mut()
            .filter(|r| r.info.kind == ServiceKind::Camera)
        {
            if record.change(
                record.info.generation,
                ServicePhase::Closed(CloseCause::DeviceLoss),
            ) {
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
        Ok(())
    }
    /// Lists only the host-approved devices, after checking the Camera grant.
    pub fn remote_camera_devices(
        &self,
        connection: u64,
        generation: u64,
    ) -> Result<Vec<CameraDevice>, Error> {
        let _engine = self.engine();
        let (link, stamp) = self.service_link(connection, generation)?;
        let mut stop = {
            let state = self.alive_state()?;
            state
                .outgoing
                .get(&connection)
                .ok_or(Error::NoSession)?
                .stop
                .subscribe()
        };
        self.runtime.block_on(async {
            let operation = tokio::time::timeout(TIMEOUT, async {
                let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
                write(&mut send, preamble(stamp, 0, LIST, 4)).await?;
                status(&read(&mut recv).await?)?;
                let devices: Vec<CameraDevice> = serde_json::from_slice(&read(&mut recv).await?).map_err(|_| Error::Malformed)?;
                validate_devices(&devices)?;
                crate::ignored!(
                    crate::trace::CAMERA,
                    "a stream that cannot finish is closed with its link",
                    send.finish()
                );
                Ok(devices)
            });
            if *stop.borrow() { return Err(Error::NoSession); }
            tokio::select! { biased; _ = stop.changed() => Err(Error::NoSession), result = operation => result.map_err(|_| Error::Timeout)? }
        })
    }
    pub fn open_camera_service(
        &self,
        id: u64,
        generation: u64,
        config: CameraConfig,
    ) -> Result<RemoteCameraService, Error> {
        let _engine = self.engine();
        config.validate()?;
        if !lock(&self.shared.video_caps).can_decode() {
            return Err(Error::Unsupported("camera H.264 decoder"));
        }
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != ServiceKind::Camera {
            return Err(Error::InvalidSetting("this is not a Camera service"));
        }
        let handle = lease.clone();
        let (link, stamp) = self.service_link(lease.info().connection_id, generation)?;
        let stop = {
            let mut state = self.alive_state()?;
            let record = state.services.get_mut(&id).ok_or(Error::NoSession)?;
            if record.stream_open {
                return Err(Error::Busy);
            }
            if lease.is_cancelled() {
                return Err(Error::NoSession);
            }
            // This flag fences duplicate backend Opens for streamed services.
            record.stream_open = true;
            record.stop.clone()
        };
        let inbox = Arc::new(VideoInbox::new());
        let frames = inbox.clone();
        let end = Arc::new(std::sync::Mutex::new(None));
        let ended = end.clone();
        let recording = Arc::new(std::sync::Mutex::new(false));
        let consent = recording.clone();
        let keyframe = Arc::new(tokio::sync::Notify::new());
        let keyframes = keyframe.clone();
        let shared = self.shared.clone();
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        self.runtime.spawn(async move {
            let opened = lease
                .run(async {
                    tokio::time::timeout(TIMEOUT, async {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, preamble(stamp, id, 1, 4)).await?;
                        write(
                            &mut send,
                            serde_json::to_vec(&config).map_err(|_| Error::Malformed)?,
                        )
                        .await?;
                        status(&read(&mut recv).await?)?;
                        let actual: CameraConfig = serde_json::from_slice(&read(&mut recv).await?)
                            .map_err(|_| Error::Malformed)?;
                        actual.validate()?;
                        if actual.device_id != config.device_id {
                            return Err(Error::Malformed);
                        }
                        Ok((send, recv, actual))
                    })
                    .await
                    .map_err(|_| Error::Timeout)?
                })
                .await;
            let mut phase = ServicePhase::Closed(CloseCause::PeerClose);
            let mut end_reason = None;
            match opened {
                Err(error) => {
                    phase = ServicePhase::Failed(error.to_string());
                    crate::ignored!(
                        crate::trace::CAMERA,
                        "the caller stopped waiting for its answer",
                        ready.send(Err(error))
                    );
                }
                Ok((mut send, mut recv, actual)) => {
                    crate::ignored!(
                        crate::trace::CAMERA,
                        "the caller stopped waiting for its answer",
                        ready.send(Ok(actual))
                    );
                    let mut reason = CameraEnd::Lost;
                    // The newest picture received, acknowledged as it changes.
                    let (received, mut acknowledging) = tokio::sync::watch::channel(None::<u32>);
                    let requests = async {
                        loop {
                            tokio::select! {
                                _ = keyframes.notified() => write(&mut send, vec![KEYFRAME]).await?,
                                changed = acknowledging.changed() => {
                                    if changed.is_err() {
                                        return Ok(());
                                    }
                                    let Some(number) = *acknowledging.borrow_and_update() else {
                                        continue;
                                    };
                                    let mut message = vec![RECEIVED];
                                    message.extend_from_slice(&number.to_be_bytes());
                                    write(&mut send, message).await?;
                                }
                            }
                        }
                    };
                    let pictures = async {
                        let mut needs_keyframe = true;
                        loop {
                            let bytes = read_video(&mut recv).await?;
                            match bytes.as_slice() {
                                [RECORDING, allowed @ (0 | 1)] => {
                                    let mut recording = lock(&consent);
                                    *recording = *allowed == 1;
                                    if !*recording {
                                        frames.close_taps();
                                    }
                                    continue;
                                }
                                [END_CLOSED] => {
                                    reason = CameraEnd::Stopped;
                                    break;
                                }
                                [END_WITHDRAWN] => {
                                    reason = CameraEnd::Withdrawn;
                                    break;
                                }
                                [END_FAILED] => {
                                    reason = CameraEnd::Stopped;
                                    return Err(Error::Unsupported("remote camera capture ended"));
                                }
                                _ => {}
                            }
                            let body = crate::video::decode_video_body(&bytes)?;
                            received.send_replace(Some(body.sequence));
                            let frame = body.frame;
                            if frame.role == crate::FrameRole::Still {
                                return Err(Error::Malformed);
                            }
                            if frame.keyframe {
                                needs_keyframe = false;
                            }
                            if needs_keyframe {
                                continue;
                            }
                            if matches!(frames.push(frame), VideoPush::KeyframeNeeded) {
                                tracing::debug!(
                                    target: crate::trace::CAMERA,
                                    service = id,
                                    "camera pictures dropped for a slow viewer; keyframe asked"
                                );
                                needs_keyframe = true;
                                keyframes.notify_one();
                            }
                        }
                        Ok::<(), Error>(())
                    };
                    let run = async {
                        tokio::pin!(pictures);
                        let asked = tokio::select! {
                            result = &mut pictures => return result,
                            result = requests => result,
                        };
                        // A host ending the stream stops reading it before its
                        // last word arrives here, so asking can fail first:
                        // that word is still worth a moment's wait.
                        match tokio::time::timeout(LAST_WORD, &mut pictures).await {
                            Ok(read) => read,
                            Err(_) => asked,
                        }
                    };
                    if lease.run(run).await.is_err() && !lease.is_cancelled() {
                        phase = ServicePhase::Failed("camera pipe ended".into());
                    } else if lease.is_cancelled() {
                        phase = ServicePhase::Closed(CloseCause::OwnerClose);
                    }
                    // A viewer that closed the picture itself has no reason from
                    // the host to give.
                    let closed_here = lease.is_cancelled()
                        && super::services::closed_here(&shared, id, generation);
                    tracing::info!(
                        target: crate::trace::CAMERA,
                        service = id,
                        reason = (!closed_here).then(|| tracing::field::debug(reason)),
                        closed_here,
                        "camera picture ended"
                    );
                    end_reason = (!closed_here).then_some(reason);
                    crate::ignored!(
                        crate::trace::CAMERA,
                        "a stream that cannot finish is closed with its link",
                        send.finish()
                    );
                }
            }
            frames.close();
            let mut state = lock(&shared.state);
            if let Some(record) = state
                .services
                .get_mut(&id)
                .filter(|r| r.info.generation == generation)
            {
                if record.change(generation, phase) {
                    shared.events.publish(CoreEvent::ServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
            drop(state);
            // Reported only once the owner record has finished, so a viewer
            // that reacts to the end can attach a new owner at once. A viewer
            // that closed the stream itself no longer reads this.
            if let Some(reason) = end_reason {
                *lock(&ended) = Some(reason);
            }
        });
        let config = result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteCameraService {
            config,
            lease: handle,
            stop,
            inbox,
            end,
            recording,
            keyframe,
        })
    }
}
fn validate_devices(devices: &[CameraDevice]) -> Result<(), Error> {
    let mut ids = std::collections::HashSet::new();
    if devices.len() > 32
        || devices.iter().any(|d| {
            d.id.is_empty() || d.id.len() > 256 || d.name.len() > 256 || !ids.insert(&d.id)
        })
    {
        return Err(Error::Malformed);
    }
    Ok(())
}
pub(super) async fn list(
    shared: Arc<Shared>,
    peer: [u8; 32],
    mut send: SendStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let permitted = lock(&shared.state)
        .trust
        .service_granted(&peer, ServiceKind::Camera);
    if !permitted {
        write_status_only(&mut send, 6).await?;
        return Ok(());
    }
    let factory = lock(&shared.camera_factory).clone();
    let Some(factory) = factory else {
        write_status_only(&mut send, 1).await?;
        return Ok(());
    };
    let run = async {
        let enumerate_shared = Arc::clone(&shared);
        let devices = tokio::task::spawn_blocking(move || {
            let devices = factory.devices();
            match &devices {
                Ok(_) => super::availability::note_backend_recovered(
                    &enumerate_shared,
                    Capability::Camera,
                ),
                Err(error) => super::availability::note_backend_failure(
                    &enumerate_shared,
                    Capability::Camera,
                    error.to_string(),
                ),
            }
            devices
        })
        .await
        .map_err(|_| Error::Unsupported("camera enumeration"))??;
        validate_devices(&devices)?;
        // Recheck the grant after enumeration, before releasing device metadata.
        if !lock(&shared.state)
            .trust
            .service_granted(&peer, ServiceKind::Camera)
        {
            write_status_only(&mut send, 6).await?;
            return Ok(());
        }
        write_status_only(&mut send, 0).await?;
        write(
            &mut send,
            serde_json::to_vec(&devices).map_err(|_| Error::Malformed)?,
        )
        .await?;
        crate::ignored!(
            crate::trace::CAMERA,
            "a stream that cannot finish is closed with its link",
            send.finish()
        );
        Ok(())
    };
    if *parent.borrow() {
        return Err(Error::NoSession);
    }
    tokio::select! { biased; _ = parent.changed() => Err(Error::NoSession), result = tokio::time::timeout(TIMEOUT, run) => result.map_err(|_| Error::Timeout)? }
}
async fn write_status_only(send: &mut SendStream, code: u8) -> Result<(), Error> {
    write(send, vec![1, code, 8]).await
}

/// Holds one camera device id in `Shared::camera_open` for a live host
/// serve. Dropping releases it on every exit path: close, revoke, parent
/// loss, shutdown, failed open and cancelled startup.
struct CameraDeviceGuard {
    shared: Arc<Shared>,
    device: String,
}

impl Drop for CameraDeviceGuard {
    fn drop(&mut self) {
        lock(&self.shared.camera_open).remove(&self.device);
    }
}

/// Reserves one camera device for a new host serve. A device with a live
/// serve refuses with structured Busy; the caller reports status 4.
fn reserve_camera_device(shared: &Arc<Shared>, device: &str) -> Result<CameraDeviceGuard, Error> {
    if !lock(&shared.camera_open).insert(device.to_owned()) {
        return Err(Error::Busy);
    }
    Ok(CameraDeviceGuard {
        shared: Arc::clone(shared),
        device: device.to_owned(),
    })
}
async fn read_video(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    crate::sidestream::read_frame(
        recv,
        |len| {
            if len == 0 || len > crate::video::MAX_VIDEO_FRAME + 10 {
                Err(Error::Malformed)
            } else {
                Ok(())
            }
        },
        |bytes| Ok(bytes.to_vec()),
    )
    .await
}
pub(super) async fn serve(
    shared: Arc<Shared>,
    id: u64,
    mut send: SendStream,
    mut recv: RecvStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let factory = lock(&shared.camera_factory)
        .clone()
        .ok_or(Error::Unsupported("camera backend"))?;
    // Clones for startup cancellation: a delayed device open must release the
    // device when parent loss / revoke / shutdown wins before Open completes.
    let mut startup_parent = parent.clone();
    let mut startup_stop = stop.clone();
    // Says how the stream ended when it ends, however it ends: a task the
    // engine drops while tearing down records it too.
    let ending = HostedEnding::new(&shared, id, move |ended| {
        tracing::debug!(
            target: crate::trace::CAMERA,
            service = id,
            ended,
            "camera stream ended"
        );
    });
    let run = async {
        if *startup_parent.borrow() || *startup_stop.borrow() {
            return Err(Error::NoSession);
        }
        // Held for the whole serve; drops on every exit path.
        let mut device_guard: Option<CameraDeviceGuard> = None;
        // Whether the viewer ended its stream while the camera opened.
        let viewer_left = std::sync::atomic::AtomicBool::new(false);
        let startup = async {
            // A stream that ends before its request is a viewer that left.
            let request = read(&mut recv).await.map_err(|_| {
                viewer_left.store(true, std::sync::atomic::Ordering::Relaxed);
                Error::NoSession
            })?;
            let config: CameraConfig =
                serde_json::from_slice(&request).map_err(|_| Error::Malformed)?;
            config.validate()?;
            if *startup_parent.borrow() || *startup_stop.borrow() {
                return Err(Error::NoSession);
            }
            let selected = config.device_id.clone();
            let returned_id = selected.clone();
            device_guard = Some(reserve_camera_device(&shared, &selected)?);
            let startup = crate::BackendStartup::scoped(
                startup_parent.clone(),
                startup_stop.clone(),
                TIMEOUT,
            );
            let _startup_guard = startup.guard();
            let open_shared = Arc::clone(&shared);
            let open = tokio::task::spawn_blocking(move || {
                let outcome = (|| {
                    startup.check()?;
                    let devices = factory.devices()?;
                    validate_devices(&devices)?;
                    if !devices.iter().any(|d| d.id == selected) {
                        return Err(Error::Unsupported("camera device"));
                    }
                    startup.check()?;
                    let backend = factory.open(config, startup.clone())?;
                    startup.check()?;
                    Ok::<_, Error>(backend)
                })();
                // Factory-stage failures blame the backend. Cancellation and
                // controller-caused protocol errors never publish.
                match &outcome {
                    Ok(_) => {
                        super::availability::note_backend_recovered(
                            &open_shared,
                            Capability::Camera,
                        );
                    }
                    Err(Error::NoSession | Error::Malformed) => {}
                    Err(error) => super::availability::note_backend_failure(
                        &open_shared,
                        Capability::Camera,
                        error.to_string(),
                    ),
                }
                outcome
            });
            let backend = tokio::select! {
                biased;
                _ = startup_parent.changed() => return Err(Error::NoSession),
                _ = startup_stop.changed() => return Err(Error::NoSession),
                // The viewer ended its stream: it closed the camera.
                _ = read(&mut recv) => {
                    viewer_left.store(true, std::sync::atomic::Ordering::Relaxed);
                    return Err(Error::NoSession);
                }
                result = open => result
                    .map_err(|_| Error::Unsupported("camera startup"))??,
            };
            // Cancellation may have arrived while waking: drop the backend
            // without admitting so the device indicator releases.
            if *startup_parent.borrow() || *startup_stop.borrow() {
                drop(backend);
                return Err(Error::NoSession);
            }
            backend.config.validate()?;
            if backend.config.device_id != returned_id {
                return Err(Error::Malformed);
            }
            Ok::<_, Error>(backend)
        };
        let opened = tokio::time::timeout(TIMEOUT, startup).await;
        if let Ok(Err(error)) = &opened {
            tracing::info!(
                target: crate::trace::CAMERA,
                service = id,
                error = error.name(),
                reason = error.reason(),
                "camera not opened"
            );
        } else if opened.is_err() {
            tracing::info!(target: crate::trace::CAMERA, service = id, error = "timeout", "camera not opened");
        }
        let backend = match opened {
            Ok(Ok(backend)) => backend,
            Ok(Err(Error::NoSession)) if viewer_left.load(std::sync::atomic::Ordering::Relaxed) => {
                ending.why(HostedEnd::ViewerClosed);
                return Err(Error::NoSession);
            }
            Ok(Err(Error::NoSession)) | Err(_)
                if *startup_parent.borrow() || *startup_stop.borrow() =>
            {
                // Cancellation won before Open completed: release without
                // admitting; backend (if any) was already dropped.
                return Err(Error::NoSession);
            }
            Ok(Err(Error::Busy)) => {
                // Another serve holds this device: structured Busy.
                write_status_only(&mut send, 4).await?;
                return Err(Error::Busy);
            }
            _ => {
                write_status_only(&mut send, 1).await?;
                return Err(Error::Unsupported("camera startup"));
            }
        };
        // A viewer that left while the camera opened can no longer hear it,
        // and the camera never opened for it.
        if write_status_only(&mut send, 0).await.is_err() {
            tracing::info!(
                target: crate::trace::CAMERA,
                service = id,
                error = Error::NoSession.name(),
                "camera not opened"
            );
            ending.why(HostedEnd::ViewerClosed);
            return Err(Error::NoSession);
        }
        write(
            &mut send,
            serde_json::to_vec(&backend.config).map_err(|_| Error::Malformed)?,
        )
        .await
        .inspect_err(|_| ending.why(HostedEnd::ViewerClosed))?;
        tracing::info!(
            target: crate::trace::CAMERA,
            service = id,
            width = backend.config.width,
            height = backend.config.height,
            fps = backend.config.fps,
            "camera opened"
        );
        let mut sent = 0u64;
        // What the viewer has not yet acknowledged paces what is written.
        let in_flight = std::sync::Mutex::new(crate::video::InFlight::default());
        let acknowledged = tokio::sync::Notify::new();
        let commands = async {
            loop {
                let request = read(&mut recv)
                    .await
                    .inspect_err(|_| ending.why(HostedEnd::ViewerClosed))?;
                match request.as_slice() {
                    [KEYFRAME] => backend.source.request_keyframe(),
                    [RECEIVED, number @ ..] => {
                        let number =
                            u32::from_be_bytes(number.try_into().map_err(|_| Error::Malformed)?);
                        lock(&in_flight).acknowledged(number, Instant::now());
                        acknowledged.notify_one();
                    }
                    _ => return Err(Error::Malformed),
                }
            }
        };
        let sending = async {
            let mut timer = tokio::time::interval(Duration::from_millis(25));
            let mut recording = None;
            loop {
                // Recording consent follows the host's sharing policy as it changes.
                let allowed = lock(&shared.state).trust.sharing.recording;
                if recording != Some(allowed) {
                    write(&mut send, vec![RECORDING, u8::from(allowed)])
                        .await
                        .inspect_err(|_| ending.why(HostedEnd::ViewerClosed))?;
                    recording = Some(allowed);
                }
                if backend.source.is_cancelled() {
                    tracing::info!(
                        target: crate::trace::CAMERA,
                        service = id,
                        failed = backend.source.has_failed(),
                        sent,
                        "camera capture ended"
                    );
                    ending.why(if backend.source.has_failed() {
                        HostedEnd::Failed("capture failed")
                    } else {
                        HostedEnd::Finished("capture ended")
                    });
                    write(
                        &mut send,
                        vec![if backend.source.has_failed() {
                            END_FAILED
                        } else {
                            END_CLOSED
                        }],
                    )
                    .await?;
                    return Ok::<(), Error>(());
                }
                // Camera pictures go in order on one stream: each is the next,
                // its number wrapping. A picture waits while the link holds
                // older ones; the queue drops what grows old meanwhile.
                while lock(&in_flight).may_send(Instant::now()) {
                    let Some(frame) = backend.source.frames.pop() else {
                        break;
                    };
                    let number = sent as u32;
                    write(&mut send, crate::video::encode_video_body(&frame, number))
                        .await
                        .inspect_err(|_| ending.why(HostedEnd::ViewerClosed))?;
                    lock(&in_flight).sent(number, Instant::now());
                    sent += 1;
                }
                tokio::select! {
                    _ = backend.source.frames.notified() => {},
                    _ = acknowledged.notified() => {},
                    _ = timer.tick() => {},
                }
            }
        };
        tokio::select! { result = commands => result, result = sending => result }
        // The backend's Drop cancels the native producer on every exit path.
    };
    if *parent.borrow() || *stop.borrow() {
        return Err(Error::NoSession);
    }
    let result = tokio::select! {
        biased;
        _ = parent.changed() => {
            ending.why(HostedEnd::ConnectionEnded);
            Err(Error::NoSession)
        }
        _ = stop.changed() => {
            ending.why(HostedEnd::service_closed(&shared, id));
            Err(Error::NoSession)
        }
        result = run => result,
    };
    // The service closed under the stream, for a reason other than its viewer's
    // close: the viewer hears it was withdrawn.
    let withdrawn =
        *stop.borrow() && HostedEnd::service_closed(&shared, id) == HostedEnd::ServiceClosed;
    ending.close(result.as_ref().err().map(Error::name));
    if withdrawn {
        // Tell the viewer why the picture stopped; it may already be gone.
        crate::ignored!(
            crate::trace::CAMERA,
            "the peer may be gone before it hears why",
            tokio::time::timeout(TIMEOUT, write(&mut send, vec![END_WITHDRAWN])).await
        );
    }
    crate::ignored!(
        crate::trace::CAMERA,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    result
}
