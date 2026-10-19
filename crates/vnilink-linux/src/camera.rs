//! V4L2 single-planar camera capture with bounded polling and VA-API encoding.
//! Serial-less approvals bind the enumerated *instance*, not just its
//! facts: the factory holds an open descriptor per approved serial-less
//! node, and capture opens through the held instance, never the
//! recyclable path. A replacement unit with identical facts therefore
//! loses consent even when no enumeration observed the gap.
#[cfg(target_os = "linux")]
use crate::LinuxEncoder;
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "linux")]
use std::io::Cursor;
#[cfg(target_os = "linux")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;
#[cfg(target_os = "linux")]
use v4l::{
    buffer::Type,
    io::{mmap::Stream, traits::CaptureStream},
    video::Capture,
    Device, Format, FourCC,
};
use vnilink::{
    camera::{
        CameraApproval, CameraBackend, CameraConfig, CameraDevice, CameraSource, HostCameraFactory,
    },
    Error,
};
#[cfg(target_os = "linux")]
use vnilink_media::{EncodeParams, RawFrame, VideoEncoder};
#[cfg(target_os = "linux")]
const BITRATE: u32 = 4_000_000;
#[cfg(target_os = "linux")]
const IDLE: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const RAW_LIMIT: usize = 128 * 1024 * 1024;
/// One enumerated V4L2 node with the hardware facts that identify it.
/// The path is recyclable — unplugging camera A and attaching camera B can
/// reuse `/dev/video0` — so approvals bind the bus location, driver and
/// card instead. Where the kernel exposes a USB serial it joins the
/// identity and distinguishes two units of the same model on the same
/// port. Two nodes that report no distinguishing facts share one
/// identity and are both withheld as ambiguous.
#[derive(Clone)]
pub struct NodeCaps {
    pub path: String,
    pub driver: String,
    pub card: String,
    pub bus: String,
    pub serial: Option<String>,
}

/// Opaque stable identity of one node. Equality is the only operation;
/// nothing parses it back. A path-shaped string is never an identity, so
/// approvals recorded before identities existed fail closed. A serial-less
/// identity cannot distinguish a replacement unit, so the factory retires
/// it the first time its node is observed absent (see `observe`).
fn identity_of(caps: &NodeCaps) -> String {
    match &caps.serial {
        Some(serial) => format!("v4l2:{}:{}:{}:{}", caps.bus, caps.driver, caps.card, serial),
        None => format!("v4l2:{}:{}:{}", caps.bus, caps.driver, caps.card),
    }
}

/// USB serial from sysfs where the kernel exposes one. `class_dir` is
/// `/sys/class/video4linux`; tests point it at a fixture. The node's
/// `device` link names a USB interface, so the serial is read from the
/// interface directory first and then its parent device. Any failure —
/// missing files, unreadable links, rejected characters — yields `None`
/// and the node falls back to the serial-less identity plus the
/// continuity rule, never to a guess.
fn read_serial(class_dir: &std::path::Path, node: &str) -> Option<String> {
    let link = class_dir.join(node).join("device");
    let target = std::fs::read_link(&link).ok()?;
    let device_dir = if target.is_absolute() {
        target
    } else {
        link.parent()?.join(target)
    };
    [device_dir.join("serial"), device_dir.join("../serial")]
        .into_iter()
        .find_map(|candidate| {
            std::fs::read_to_string(candidate)
                .ok()
                .and_then(|raw| clean_serial(&raw))
        })
}

/// A serial joins an identity only as short graphic ASCII without the
/// field separator. Empty, overlong or odd values are dropped so a
/// corrupt sysfs never mints an unmatchable approval.
fn clean_serial(raw: &str) -> Option<String> {
    let serial = raw.trim();
    if serial.is_empty() || serial.len() > 64 {
        return None;
    }
    if !serial.bytes().all(|b| b.is_ascii_graphic() && b != b':') {
        return None;
    }
    Some(serial.to_owned())
}

fn valid_approval(id: &str) -> bool {
    id.len() <= 256
        && id
            .strip_prefix("v4l2:")
            .is_some_and(|rest| !rest.is_empty())
        && !id.bytes().any(|b| b < 0x20 || b == 0x7f)
}

/// Exactly one node per identity, ordered for stable listing. Identities
/// shared by several nodes are ambiguous and withheld: opening one would
/// authorize hardware the approval cannot distinguish. Each entry carries
/// (identity, node path, card name, serial): the factory needs the serial
/// to know which approvals survive a removal.
fn unique_identities(nodes: Vec<NodeCaps>) -> Vec<(String, String, String, Option<String>)> {
    let mut by_identity: std::collections::HashMap<String, Vec<NodeCaps>> =
        std::collections::HashMap::new();
    for node in nodes {
        by_identity
            .entry(identity_of(&node))
            .or_default()
            .push(node);
    }
    let mut unique: Vec<(String, String, String, Option<String>)> = by_identity
        .into_iter()
        .filter_map(|(identity, mut nodes)| {
            if nodes.len() != 1 {
                return None;
            }
            let node = nodes.pop().unwrap();
            Some((identity, node.path, node.card, node.serial))
        })
        .collect();
    unique.sort_by(|a, b| a.0.cmp(&b.0));
    unique
}

trait Inspector: Send + Sync {
    fn nodes(&self) -> Vec<NodeCaps>;
    /// Open a continuity handle to `path`, pinning the enumerated
    /// instance. None when the node cannot be held: the device is then
    /// withheld, never listed on facts alone.
    fn hold(&self, path: &str) -> Option<Box<dyn HeldNode>>;
}

/// One held device instance. The handle pins the enumerated instance:
/// removal kills it even when a replacement reuses the node path and
/// reports identical bus, driver and card facts.
trait HeldNode: Send + Sync {
    /// True while the held instance is still the live device.
    fn alive(&self) -> bool;
    /// Path that opens the HELD instance (not the recyclable node path),
    /// so resolve-then-open cannot swap instances mid-race.
    fn open_path(&self) -> String;
}

#[cfg(target_os = "linux")]
struct V4lInspector;
#[cfg(target_os = "linux")]
impl V4lInspector {
    fn read(path: &str) -> Option<NodeCaps> {
        let device = Device::with_path(path).ok()?;
        let caps = device.query_caps().ok()?;
        if !caps
            .capabilities
            .contains(v4l::capability::Flags::VIDEO_CAPTURE | v4l::capability::Flags::STREAMING)
        {
            return None;
        }
        let serial = std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|node| read_serial(std::path::Path::new("/sys/class/video4linux"), node));
        Some(NodeCaps {
            path: path.to_owned(),
            driver: caps.driver,
            card: caps.card,
            bus: caps.bus,
            serial,
        })
    }
}
#[cfg(target_os = "linux")]
impl Inspector for V4lInspector {
    fn nodes(&self) -> Vec<NodeCaps> {
        v4l::context::enum_devices()
            .into_iter()
            .filter_map(|node| Self::read(&node.path().to_string_lossy()))
            .collect()
    }
    fn hold(&self, path: &str) -> Option<Box<dyn HeldNode>> {
        let device = Device::with_path(path).ok()?;
        device.query_caps().ok()?;
        Some(Box::new(V4lHeld { device }))
    }
}

/// A V4L2 device held open. Removal fails its ioctls with ENODEV even
/// when a replacement reuses the node path, and capture opens through
/// `/dev/fd`, which addresses this open description rather than
/// re-resolving the recyclable path. One idle read/write description per
/// approved serial-less camera; nothing streams until capture opens.
#[cfg(target_os = "linux")]
struct V4lHeld {
    device: Device,
}

#[cfg(target_os = "linux")]
impl HeldNode for V4lHeld {
    fn alive(&self) -> bool {
        self.device.query_caps().is_ok()
    }
    fn open_path(&self) -> String {
        format!("/dev/fd/{}", self.device.handle().fd())
    }
}

#[cfg(target_os = "linux")]
pub fn camera_devices() -> Result<Vec<CameraDevice>, Error> {
    Ok(unique_identities(V4lInspector.nodes())
        .into_iter()
        .map(|(identity, _, name, _)| CameraDevice { id: identity, name })
        .collect())
}
/// Continuity state for approved serial-less identities, under one
/// lock so observation, validation and retirement never interleave.
#[derive(Default)]
struct FactoryState {
    /// Approved serial-less identities observed present at least once.
    seen: HashSet<String>,
    /// Retired until the operator re-approves (a new factory).
    invalidated: HashSet<String>,
    /// Open handle per approved serial-less identity, pinning the
    /// enumerated instance across the gaps between enumerations.
    held: HashMap<String, Arc<dyn HeldNode>>,
}

pub struct LinuxCameraFactory {
    approved: CameraApproval,
    inspector: Box<dyn Inspector>,
    state: Mutex<FactoryState>,
}
impl LinuxCameraFactory {
    #[cfg(target_os = "linux")]
    pub fn new(approved: CameraApproval) -> Result<Self, Error> {
        Self::with_approval(approved, Box::new(V4lInspector))
    }
    #[cfg(test)]
    fn with_inspector(approved: Vec<String>, inspector: Box<dyn Inspector>) -> Result<Self, Error> {
        Self::with_approval(CameraApproval::Only(approved), inspector)
    }
    fn with_approval(
        approved: CameraApproval,
        inspector: Box<dyn Inspector>,
    ) -> Result<Self, Error> {
        Self {
            approved,
            inspector,
            state: Mutex::new(FactoryState::default()),
        }
        .validated()
    }
    fn validated(self) -> Result<Self, Error> {
        if let CameraApproval::Only(ids) = &self.approved {
            if ids.len() > 32 || ids.iter().any(|id| !valid_approval(id)) {
                return Err(Error::InvalidSetting("invalid approved V4L2 camera IDs"));
            }
        }
        Ok(self)
    }
    /// Record one enumeration. An approved serial-less identity that is
    /// absent now but was present before is retired: the reappearing node
    /// may be replacement hardware with identical bus, driver and card.
    /// A first appearance is allowed — an approval with no baseline to
    /// compare cannot distinguish the owner's camera from an attacker's,
    /// so refusing it would only wedge the approve-then-plug flow.
    ///
    /// Presence alone is not continuity: a present identity whose held
    /// instance died is a replacement unit behind identical facts, and
    /// is retired even though no enumeration observed the gap. A held
    /// node that cannot be opened is withheld until it can be held, and
    /// a first appearance that cannot be held is not recorded as seen.
    /// Serial-bound identities are never held or retired: the same
    /// serial reappearing is the same instance. With All, no camera was
    /// chosen by name, so any present camera is the one offered.
    fn observe(&self, unique: &[(String, String, String, Option<String>)]) {
        let CameraApproval::Only(approved_ids) = &self.approved else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        for (identity, path, _, serial) in unique {
            if serial.is_some() || !approved_ids.contains(identity) {
                continue;
            }
            if state.invalidated.contains(identity) {
                continue;
            }
            match state.held.get(identity) {
                Some(handle) if handle.alive() => {
                    state.seen.insert(identity.clone());
                }
                Some(_) => {
                    state.held.remove(identity);
                    state.invalidated.insert(identity.clone());
                }
                None => {
                    if let Some(handle) = self.inspector.hold(path) {
                        state.held.insert(identity.clone(), Arc::from(handle));
                        state.seen.insert(identity.clone());
                    }
                }
            }
        }
        for approved in approved_ids {
            let present = unique
                .iter()
                .any(|(identity, _, _, _)| identity == approved);
            if present || !state.seen.contains(approved) {
                continue;
            }
            // Absent after being seen. A live held instance proves a mere
            // enumeration flake; otherwise the removal retires consent.
            let live = state.held.get(approved).is_some_and(|held| held.alive());
            if !live {
                state.held.remove(approved);
                state.invalidated.insert(approved.clone());
            }
        }
    }
    fn held_for_start(
        &self,
        identity: &str,
        serial: &Option<String>,
    ) -> Result<Option<Arc<dyn HeldNode>>, Error> {
        if serial.is_some() || !self.approved.names(identity) {
            return Ok(None);
        }
        let mut state = self.state.lock().unwrap();
        if state.invalidated.contains(identity) {
            return Err(Error::Unauthorized);
        }
        match state.held.get(identity) {
            Some(handle) if handle.alive() => Ok(Some(handle.clone())),
            _ => {
                state.held.remove(identity);
                state.invalidated.insert(identity.to_owned());
                Err(Error::Unauthorized)
            }
        }
    }
    fn retired(&self, identity: &str) -> bool {
        self.state.lock().unwrap().invalidated.contains(identity)
    }
    /// Maps a failed capture start. A dead held instance retires consent
    /// (the device may be gone: fail closed); a live one keeps it and the
    /// original transient error propagates. Serial-bound approvals never
    /// retire here: only the serial distinguishes their instances.
    fn capture_failed(&self, identity: &str, serial: &Option<String>, error: Error) -> Error {
        if serial.is_some() || !self.approved.names(identity) {
            return error;
        }
        let mut state = self.state.lock().unwrap();
        if state.held.get(identity).is_some_and(|held| held.alive()) {
            return error;
        }
        state.held.remove(identity);
        state.invalidated.insert(identity.to_owned());
        Error::Unauthorized
    }
}
impl HostCameraFactory for LinuxCameraFactory {
    fn devices(&self) -> Result<Vec<CameraDevice>, Error> {
        let unique = unique_identities(self.inspector.nodes());
        self.observe(&unique);
        let state = self.state.lock().unwrap();
        let retired = state.invalidated.clone();
        let held: HashSet<String> = state.held.keys().cloned().collect();
        drop(state);
        Ok(unique
            .into_iter()
            .filter(|(identity, _, _, serial)| {
                self.approved.allows(identity)
                    && !retired.contains(identity)
                    && (serial.is_some()
                        || held.contains(identity)
                        || !self.approved.names(identity))
            })
            .map(|(identity, _, name, _)| CameraDevice { id: identity, name })
            .collect())
    }
    fn open(
        &self,
        requested: CameraConfig,
        startup: vnilink::BackendStartup,
    ) -> Result<CameraBackend, Error> {
        startup.check()?;
        requested.validate()?;
        if !self.approved.allows(&requested.device_id) {
            return Err(Error::Unauthorized);
        }
        // Resolve the approved identity to its current node. A replacement
        // camera keeps the path but not the identity, so the old approval
        // finds no node and the open is refused without touching hardware.
        // A serial-less identity that vanished before is refused even when
        // identical facts reappear: consent did not survive the removal.
        let unique = unique_identities(self.inspector.nodes());
        self.observe(&unique);
        if self.retired(&requested.device_id) {
            return Err(Error::Unauthorized);
        }
        let (path, serial) = unique
            .into_iter()
            .find(|(identity, _, _, _)| *identity == requested.device_id)
            .map(|(_, path, _, serial)| (path, serial))
            .ok_or(Error::Unauthorized)?;
        // Serial-less captures open through the held instance, never the
        // recyclable path: a swap between resolve and open serves the
        // approved instance or fails, never the replacement. Serial-bound
        // identities resolve by path and re-verify the serial after open.
        let held = self.held_for_start(&requested.device_id, &serial)?;
        let open_path = held
            .as_ref()
            .map_or_else(|| path.clone(), |handle| handle.open_path());
        let worker_held = held.clone();
        let node = std::path::Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        let source = CameraSource::new();
        let mut backend = CameraBackend::new(requested.clone(), source.clone())?;
        let inner: Result<CameraConfig, Error> = {
            #[cfg(target_os = "linux")]
            {
                let (ready, result) = mpsc::sync_channel(1);
                let worker_startup = startup.clone();
                let worker_requested = requested.clone();
                std::thread::Builder::new()
                    .name("vnilink-camera-v4l".into())
                    .spawn(move || {
                        // Retain the exact approved descriptor until capture has
                        // opened it, even if enumeration retires its factory entry.
                        let _held = worker_held;
                        if capture(
                            worker_requested,
                            open_path,
                            node,
                            source.clone(),
                            &ready,
                            &worker_startup,
                        )
                        .is_err()
                        {
                            let _ = ready.try_send(Err(Error::Unsupported("V4L2 camera")));
                            source.fail();
                        }
                        source.close();
                    })
                    .map_err(|_| Error::Unsupported("camera worker"))?;
                match startup.receive(&result) {
                    Ok(inner) => inner,
                    // The caller cancelled: not a capture loss, so consent
                    // is untouched and the cancellation propagates as-is.
                    Err(cancelled) => return Err(cancelled),
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (&open_path, &node, worker_held);
                Err(Error::Unsupported("camera capture"))
            }
        };
        backend.config =
            inner.map_err(|error| self.capture_failed(&requested.device_id, &serial, error))?;
        if held.as_ref().is_some_and(|handle| !handle.alive()) || self.retired(&requested.device_id)
        {
            backend.source.close();
            return Err(Error::Unauthorized);
        }
        Ok(backend)
    }
}
/// `open_path` addresses the approved instance: the recyclable node
/// path for serial-bound identities (re-verified below), the held
/// `/dev/fd` path for serial-less ones. `node` is the real node name
/// for the sysfs serial lookup, never derived from the held path.
#[cfg(target_os = "linux")]
fn capture(
    request: CameraConfig,
    open_path: String,
    node: Option<String>,
    source: Arc<CameraSource>,
    ready: &mpsc::SyncSender<Result<CameraConfig, Error>>,
    startup: &vnilink::BackendStartup,
) -> Result<(), Error> {
    startup.check()?;
    let device = Device::with_path(&open_path).map_err(|_| Error::Unsupported("camera device"))?;
    // The node may have been swapped between resolve and open. The opened
    // descriptor must still report the approved identity.
    let caps = device
        .query_caps()
        .map_err(|_| Error::Unsupported("camera device"))?;
    let serial = node
        .as_deref()
        .and_then(|node| read_serial(std::path::Path::new("/sys/class/video4linux"), node));
    let opened = NodeCaps {
        path: open_path.clone(),
        driver: caps.driver,
        card: caps.card,
        bus: caps.bus,
        serial,
    };
    if identity_of(&opened) != request.device_id {
        return Err(Error::Unauthorized);
    }
    let supported = device
        .enum_formats()
        .map_err(|_| Error::Unsupported("camera formats"))?;
    let mut selected = None;
    // Compressed USB delivery avoids requiring raw 1080p@30 bandwidth.
    for code in [b"MJPG", b"YUYV", b"RGB3", b"BGR3"] {
        let fourcc = FourCC::new(code);
        if !supported.iter().any(|f| f.fourcc == fourcc) {
            continue;
        }
        if let Ok(format) = device.set_format(&Format::new(
            u32::from(request.width),
            u32::from(request.height),
            fourcc,
        )) {
            if format.fourcc == fourcc {
                selected = Some(format);
                break;
            }
        }
    }
    let format = selected.ok_or(Error::Unsupported("camera pixel formats"))?;
    if format.size as usize > RAW_LIMIT {
        return Err(Error::Unsupported("camera buffer size"));
    }
    let parameters = device
        .set_params(&v4l::video::capture::Parameters::with_fps(u32::from(
            request.fps,
        )))
        .map_err(|_| Error::Unsupported("camera frame rate"))?;
    let interval = parameters.interval;
    let fps = interval
        .denominator
        .checked_div(interval.numerator)
        .and_then(|f| u16::try_from(f).ok())
        .ok_or(Error::Unsupported("camera frame rate"))?;
    let config = CameraConfig {
        device_id: request.device_id,
        width: u16::try_from(format.width).map_err(|_| Error::Malformed)?,
        height: u16::try_from(format.height).map_err(|_| Error::Malformed)?,
        fps,
    };
    config.validate()?;
    let mut encoder = LinuxEncoder::open(
        config.width,
        config.height,
        config.fps as u8,
        BITRATE,
        false,
    )
    .map_err(|_| Error::Unsupported("camera H.264 encoder"))?;
    let mut stream = Stream::with_buffers(&device, Type::VideoCapture, 2)
        .map_err(|_| Error::Unsupported("camera buffers"))?;
    stream.set_timeout(Duration::from_millis(25));
    startup.check()?;
    if source.is_cancelled() {
        return Ok(());
    }
    let start = Instant::now();
    let mut reported = false;
    let mut last = start;
    while !source.is_cancelled() {
        if !reported {
            startup.check()?;
        }
        let (bytes, metadata) = match stream.next() {
            Ok(frame) => frame,
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut && last.elapsed() < IDLE => continue,
            Err(_) => return Err(Error::Unsupported("camera stopped delivering frames")),
        };
        last = Instant::now();
        let used = metadata.bytesused as usize;
        let bytes = bytes
            .get(..used)
            .filter(|b| b.len() <= RAW_LIMIT)
            .ok_or(Error::Malformed)?;
        let frame = decode_frame(
            bytes,
            &format,
            start.elapsed().as_micros().min(u64::MAX as u128) as u64,
        )?;
        let sample = encoder
            .encode(
                &frame,
                &EncodeParams {
                    force_keyframe: source.take_keyframe_request(),
                    bitrate_bps: BITRATE,
                    frame_rate: config.fps as u8,
                },
            )
            .map_err(|_| Error::Unsupported("camera encoding"))?;
        source.publish(vnilink::EncodedFrame::h264(
            sample.keyframe(),
            frame.timestamp_us(),
            sample.bytes().to_vec(),
        )?)?;
        if !reported {
            ready
                .try_send(Ok(config.clone()))
                .map_err(|_| Error::NoSession)?;
            reported = true;
        }
    }
    // Stream Drop issues STREAMOFF and unmaps its two buffers; Device closes.
    Ok(())
}
#[cfg(target_os = "linux")]
fn decode_frame(bytes: &[u8], format: &Format, time: u64) -> Result<RawFrame, Error> {
    let w = u16::try_from(format.width).map_err(|_| Error::Malformed)?;
    let h = u16::try_from(format.height).map_err(|_| Error::Malformed)?;
    if w == 0 || h == 0 || w > 4096 || h > 4096 || w % 2 != 0 || h % 2 != 0 {
        return Err(Error::Malformed);
    }
    let mut bgra = Vec::with_capacity(usize::from(w) * usize::from(h) * 4);
    if format.fourcc == FourCC::new(b"MJPG") {
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
        decoder.set_max_decoding_buffer_size(RAW_LIMIT);
        decoder.read_info().map_err(|_| Error::Malformed)?;
        let info = decoder.info().ok_or(Error::Malformed)?;
        if info.width != w
            || info.height != h
            || info.pixel_format != jpeg_decoder::PixelFormat::RGB24
        {
            return Err(Error::Malformed);
        }
        let rgb = decoder.decode().map_err(|_| Error::Malformed)?;
        if rgb.len() != usize::from(w) * usize::from(h) * 3 {
            return Err(Error::Malformed);
        }
        for p in rgb.chunks_exact(3) {
            bgra.extend([p[2], p[1], p[0], 255]);
        }
    } else {
        let yuyv = format.fourcc == FourCC::new(b"YUYV");
        let row = usize::from(w) * if yuyv { 2 } else { 3 };
        let stride = (format.stride as usize).max(row);
        let needed = stride
            .checked_mul(usize::from(h) - 1)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::Malformed)?;
        if bytes.len() < needed {
            return Err(Error::Malformed);
        }
        for y in 0..usize::from(h) {
            let bytes = &bytes[y * stride..y * stride + row];
            if yuyv {
                for p in bytes.chunks_exact(4) {
                    for luminance in [p[0], p[2]] {
                        bgra.extend(yuv_pixel(luminance, p[1], p[3], format)?);
                    }
                }
            } else if format.fourcc == FourCC::new(b"RGB3") {
                for p in bytes.chunks_exact(3) {
                    bgra.extend([p[2], p[1], p[0], 255]);
                }
            } else if format.fourcc == FourCC::new(b"BGR3") {
                for p in bytes.chunks_exact(3) {
                    bgra.extend([p[0], p[1], p[2], 255]);
                }
            } else {
                return Err(Error::Unsupported("camera pixel format"));
            }
        }
    }
    RawFrame::bgra(w, h, time, bgra).map_err(|_| Error::Malformed)
}
#[cfg(target_os = "linux")]
fn yuv_pixel(y: u8, u: u8, v: u8, format: &Format) -> Result<[u8; 4], Error> {
    use v4l::format::{Colorspace, Quantization};
    let full = matches!(format.quantization, Quantization::FullRange)
        || matches!(format.quantization, Quantization::Default)
            && matches!(format.colorspace, Colorspace::JPEG);
    let rec709 = matches!(format.colorspace, Colorspace::Rec709);
    if !matches!(
        format.colorspace,
        Colorspace::Default
            | Colorspace::SMPTE170M
            | Colorspace::SRGB
            | Colorspace::JPEG
            | Colorspace::Rec709
    ) {
        return Err(Error::Unsupported("camera colour space"));
    }
    let (scale, offset, rv, gu, gv, bu) = match (full, rec709) {
        (false, false) => (298, 16, 409, 100, 208, 516),
        (false, true) => (298, 16, 459, 55, 136, 541),
        (true, false) => (256, 0, 359, 88, 183, 454),
        (true, true) => (256, 0, 403, 48, 120, 475),
    };
    let y = scale * (i32::from(y) - offset);
    let u = i32::from(u) - 128;
    let v = i32::from(v) - 128;
    let c = |n: i32| ((n + 128) >> 8).clamp(0, 255) as u8;
    Ok([c(y + bu * u), c(y - gu * u - gv * v), c(y + rv * v), 255])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(target_os = "linux")]
    fn camera_pixel_conversion_handles_stride_colours_and_invalid_input() {
        let mut fmt = Format::new(2, 2, FourCC::new(b"YUYV"));
        fmt.stride = 6;
        let frame = decode_frame(
            &[16, 128, 235, 128, 9, 9, 235, 128, 16, 128, 9, 9],
            &fmt,
            17,
        )
        .unwrap();
        assert_eq!(
            frame.pixels(),
            &[0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255]
        );
        assert_eq!(frame.timestamp_us(), 17);
        // The final row need not contain padding after its last pixel.
        assert_eq!(
            decode_frame(&[16, 128, 235, 128, 9, 9, 235, 128, 16, 128], &fmt, 17)
                .unwrap()
                .pixels(),
            frame.pixels()
        );
        assert!(decode_frame(&[0; 4], &fmt, 0).is_err());
        let mut rgb = Format::new(2, 2, FourCC::new(b"RGB3"));
        rgb.stride = 8;
        let frame = decode_frame(
            &[255, 0, 0, 0, 0, 255, 8, 8, 0, 255, 0, 255, 255, 255, 8, 8],
            &rgb,
            0,
        )
        .unwrap();
        assert_eq!(
            frame.pixels(),
            &[0, 0, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255]
        );
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn camera_mjpeg_decodes_pixels_and_refuses_wrong_dimensions() {
        let bytes = include_bytes!("testdata/camera-red.jpg");
        let format = Format::new(2, 2, FourCC::new(b"MJPG"));
        let frame = decode_frame(bytes, &format, 91).unwrap();
        assert_eq!(frame.timestamp_us(), 91);
        assert_eq!(frame.pixels().len(), 16);
        for pixel in frame.pixels().chunks_exact(4) {
            for (actual, expected) in pixel.iter().zip([10u8, 20, 240, 255]) {
                assert!(actual.abs_diff(expected) <= 2);
            }
        }
        assert!(decode_frame(bytes, &Format::new(4, 2, FourCC::new(b"MJPG")), 0).is_err());
        assert!(decode_frame(&bytes[..20], &format, 0).is_err());
    }
    #[test]
    fn camera_factory_refuses_unapproved_device_without_opening_it() {
        let (inspector, _) = scripted(vec![node(
            "/dev/video0",
            "uvcvideo",
            "Stranger Cam",
            "usb-0000:00:14.0-9",
            None,
        )]);
        let factory = LinuxCameraFactory::with_inspector(vec![], Box::new(inspector)).unwrap();
        assert!(factory.devices().unwrap().is_empty());
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: "/dev/video0".into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unauthorized)
        ));
    }

    /// Scripted node list with an instance version. Every mutation bumps
    /// the version, killing handles held before it — the faithful model
    /// of unplug/replug, where the old open description dies even when
    /// the replacement reports identical facts.
    struct ScriptState {
        nodes: Vec<NodeCaps>,
        version: u64,
        /// Extra alive() grants after the version check. Tests spend them
        /// to interleave instance death between validation and
        /// capture-loss mapping; `usize::MAX` never exhausts.
        countdown: usize,
    }
    struct ScriptedInspector {
        state: std::sync::Arc<std::sync::Mutex<ScriptState>>,
    }
    /// Test-side mutation handle, cloned before the inspector moves into
    /// the factory.
    struct Script {
        state: std::sync::Arc<std::sync::Mutex<ScriptState>>,
    }
    impl Script {
        fn set_nodes(&self, nodes: Vec<NodeCaps>) {
            let mut state = self.state.lock().unwrap();
            state.nodes = nodes;
            state.version += 1;
        }
        fn set_countdown(&self, countdown: usize) {
            self.state.lock().unwrap().countdown = countdown;
        }
        fn inspector(&self) -> ScriptedInspector {
            ScriptedInspector {
                state: self.state.clone(),
            }
        }
    }
    fn scripted(nodes: Vec<NodeCaps>) -> (ScriptedInspector, Script) {
        let state = std::sync::Arc::new(std::sync::Mutex::new(ScriptState {
            nodes,
            version: 0,
            countdown: usize::MAX,
        }));
        (
            ScriptedInspector {
                state: state.clone(),
            },
            Script { state },
        )
    }
    struct ScriptedHeld {
        version: u64,
        state: std::sync::Arc<std::sync::Mutex<ScriptState>>,
        path: String,
    }
    impl HeldNode for ScriptedHeld {
        fn alive(&self) -> bool {
            let mut state = self.state.lock().unwrap();
            if state.version != self.version {
                return false;
            }
            if state.countdown == 0 {
                return false;
            }
            state.countdown -= 1;
            true
        }
        fn open_path(&self) -> String {
            self.path.clone()
        }
    }
    impl Inspector for ScriptedInspector {
        fn nodes(&self) -> Vec<NodeCaps> {
            self.state.lock().unwrap().nodes.clone()
        }
        fn hold(&self, path: &str) -> Option<Box<dyn HeldNode>> {
            let version = self.state.lock().unwrap().version;
            Some(Box::new(ScriptedHeld {
                version,
                state: self.state.clone(),
                path: path.to_owned(),
            }))
        }
    }
    fn node(path: &str, driver: &str, card: &str, bus: &str, serial: Option<&str>) -> NodeCaps {
        NodeCaps {
            path: path.into(),
            driver: driver.into(),
            card: card.into(),
            bus: bus.into(),
            serial: serial.map(str::to_owned),
        }
    }

    #[test]
    fn all_cameras_lists_each_present_camera_including_one_plugged_in_later() {
        let (inspector, script) = scripted(vec![node(
            "/dev/video0",
            "uvcvideo",
            "Alpha Cam",
            "usb-0000:00:14.0-1",
            None,
        )]);
        let factory =
            LinuxCameraFactory::with_approval(CameraApproval::All, Box::new(inspector)).unwrap();
        let names = |factory: &LinuxCameraFactory| {
            factory
                .devices()
                .unwrap()
                .into_iter()
                .map(|device| device.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&factory), ["Alpha Cam"]);
        script.set_nodes(vec![
            node(
                "/dev/video0",
                "uvcvideo",
                "Alpha Cam",
                "usb-0000:00:14.0-1",
                None,
            ),
            node(
                "/dev/video2",
                "uvcvideo",
                "Beta Cam",
                "usb-0000:00:14.0-2",
                Some("B1"),
            ),
        ]);
        assert_eq!(names(&factory), ["Alpha Cam", "Beta Cam"]);
        // Nothing was chosen by name, so a camera that left and came back
        // is still offered.
        script.set_nodes(vec![]);
        assert!(names(&factory).is_empty());
        script.set_nodes(vec![node(
            "/dev/video0",
            "uvcvideo",
            "Alpha Cam",
            "usb-0000:00:14.0-1",
            None,
        )]);
        assert_eq!(names(&factory), ["Alpha Cam"]);
    }

    #[test]
    fn recycled_node_keeps_path_but_loses_approval() {
        let (inspector, script) = scripted(vec![node(
            "/dev/video0",
            "uvcvideo",
            "Alpha Cam",
            "usb-0000:00:14.0-1",
            None,
        )]);
        let alpha = "v4l2:usb-0000:00:14.0-1:uvcvideo:Alpha Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![alpha.into()], Box::new(inspector)).unwrap();
        let listed = factory.devices().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, alpha);
        assert_eq!(listed[0].name, "Alpha Cam");
        // Camera A is unplugged; camera B takes the same node with
        // different hardware facts. The old approval lists nothing...
        script.set_nodes(vec![node(
            "/dev/video0",
            "uvcvideo",
            "Beta Cam",
            "usb-0000:00:14.0-2",
            None,
        )]);
        assert!(factory.devices().unwrap().is_empty());
        // ...and opens nothing under the old approval.
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: alpha.into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unauthorized)
        ));
        // Fresh approval of the replacement lists it again.
        let beta = "v4l2:usb-0000:00:14.0-2:uvcvideo:Beta Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![beta.into()], Box::new(script.inspector()))
                .unwrap();
        let listed = factory.devices().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, beta);
    }

    #[test]
    fn ambiguous_identities_are_withheld_and_legacy_paths_rejected() {
        // Two nodes, no distinguishing facts: neither is enumerable.
        let (inspector, script) = scripted(vec![
            node("/dev/video0", "skel", "Skeleton", "", None),
            node("/dev/video1", "skel", "Skeleton", "", None),
        ]);
        let factory = LinuxCameraFactory::with_inspector(
            vec!["v4l2::skel:Skeleton".into()],
            Box::new(inspector),
        )
        .unwrap();
        assert!(factory.devices().unwrap().is_empty());
        // One node with the same facts is unambiguous and enumerable.
        script.set_nodes(vec![node("/dev/video0", "skel", "Skeleton", "", None)]);
        let listed = factory.devices().unwrap();
        assert_eq!(listed.len(), 1);
        // Approvals recorded as raw node paths fail closed: the operator
        // must re-approve the hardware identity instead of reusing a path.
        let (legacy_inspector, _) = scripted(vec![]);
        assert!(matches!(
            LinuxCameraFactory::with_inspector(
                vec!["/dev/video0".into()],
                Box::new(legacy_inspector)
            ),
            Err(Error::InvalidSetting(_))
        ));
    }

    #[test]
    fn identical_replacement_after_removal_loses_consent_until_reapproved() {
        // Same model, driver and bus before and after: without a serial
        // the facts cannot tell the replacement unit from the original.
        let twin = || {
            node(
                "/dev/video0",
                "uvcvideo",
                "Twin Cam",
                "usb-0000:00:14.0-1",
                None,
            )
        };
        let (inspector, script) = scripted(vec![twin()]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        // Removal retires the approval even though nothing replaces it yet.
        script.set_nodes(vec![]);
        assert!(factory.devices().unwrap().is_empty());
        // Identical facts reappear: consent did not survive the gap.
        script.set_nodes(vec![twin()]);
        assert!(factory.devices().unwrap().is_empty());
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: approved.into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unauthorized)
        ));
        // Only a fresh approval (a new factory) lists the node again.
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(script.inspector()))
                .unwrap();
        let listed = factory.devices().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, approved);
    }

    fn open_request(device_id: &str) -> (CameraConfig, vnilink::BackendStartup) {
        (
            CameraConfig {
                device_id: device_id.into(),
                width: 640,
                height: 480,
                fps: 30,
            },
            vnilink::BackendStartup::new(Duration::from_secs(5)),
        )
    }

    /// SPEC07: list A, then — with no enumeration in between — replace it
    /// with same-model B on the same port. Identical facts must not
    /// inherit consent: the held instance died in the gap.
    #[test]
    fn identical_replacement_between_polls_loses_consent() {
        let twin = || {
            node(
                "/dev/video0",
                "uvcvideo",
                "Twin Cam",
                "usb-0000:00:14.0-1",
                None,
            )
        };
        let (inspector, script) = scripted(vec![twin()]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        // Unplug A and attach B with no devices()/open() call in between:
        // the version bump is the only record of the gap.
        script.set_nodes(vec![]);
        script.set_nodes(vec![twin()]);
        assert!(factory.devices().unwrap().is_empty());
        let (request, startup) = open_request(approved);
        assert!(matches!(
            factory.open(request, startup),
            Err(Error::Unauthorized)
        ));
        // Only a fresh approval (a new factory) lists the replacement.
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(script.inspector()))
                .unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
    }

    /// SPEC07: the resolve/open race. The approved instance is silently
    /// swapped after the last listing; the open must refuse rather than
    /// serve the replacement that now owns the node.
    #[test]
    fn resolve_open_race_refuses_the_replacement() {
        let twin = || {
            node(
                "/dev/video0",
                "uvcvideo",
                "Twin Cam",
                "usb-0000:00:14.0-1",
                None,
            )
        };
        let (inspector, script) = scripted(vec![twin()]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        // Same facts, new instance: no call observes the swap before open.
        script.set_nodes(vec![twin()]);
        let (request, startup) = open_request(approved);
        assert!(matches!(
            factory.open(request, startup),
            Err(Error::Unauthorized)
        ));
        assert!(factory.devices().unwrap().is_empty());
    }

    /// SPEC07: capture fails after the instance died mid-open. The death
    /// lands between validation and capture-loss mapping (two alive()
    /// grants: observe validation and open validation pass; the mapping
    /// then sees the dead instance): consent is retired and the open
    /// reports Unauthorized, not a transient error.
    #[test]
    fn capture_loss_with_dead_instance_retires_consent() {
        let (inspector, script) = scripted(vec![node(
            "/dev/vnilink-nonexistent0",
            "uvcvideo",
            "Twin Cam",
            "usb-0000:00:14.0-1",
            None,
        )]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        script.set_countdown(2);
        let (request, startup) = open_request(approved);
        assert!(matches!(
            factory.open(request, startup),
            Err(Error::Unauthorized)
        ));
        assert!(factory.devices().unwrap().is_empty());
    }

    /// SPEC07: capture fails while the held instance stays live. The
    /// original error propagates (never Unauthorized) and consent
    /// survives for retry.
    #[test]
    fn transient_capture_failure_keeps_consent() {
        let (inspector, _) = scripted(vec![node(
            "/dev/vnilink-nonexistent0",
            "uvcvideo",
            "Twin Cam",
            "usb-0000:00:14.0-1",
            None,
        )]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        let (request, startup) = open_request(approved);
        assert!(matches!(
            factory.open(request, startup),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(factory.devices().unwrap().len(), 1);
    }

    #[test]
    fn serialised_reappearance_keeps_consent_for_the_same_instance() {
        let unit = |serial: Option<&str>| {
            node(
                "/dev/video0",
                "uvcvideo",
                "Twin Cam",
                "usb-0000:00:14.0-1",
                serial,
            )
        };
        let (inspector, script) = scripted(vec![unit(Some("A1"))]);
        let approved = "v4l2:usb-0000:00:14.0-1:uvcvideo:Twin Cam:A1";
        let factory =
            LinuxCameraFactory::with_inspector(vec![approved.into()], Box::new(inspector)).unwrap();
        assert_eq!(factory.devices().unwrap().len(), 1);
        // Removal hides the node but the serial-bound consent survives.
        script.set_nodes(vec![]);
        assert!(factory.devices().unwrap().is_empty());
        script.set_nodes(vec![unit(Some("A1"))]);
        assert_eq!(factory.devices().unwrap().len(), 1);
        // A different unit of the same model is a different identity.
        script.set_nodes(vec![unit(Some("B2"))]);
        assert!(factory.devices().unwrap().is_empty());
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: approved.into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unauthorized)
        ));
        // The original unit back: listed, and open passes authorization
        // (it fails only on the fixture's fake node path, without hardware).
        script.set_nodes(vec![unit(Some("A1"))]);
        assert_eq!(factory.devices().unwrap().len(), 1);
        assert!(matches!(
            factory.open(
                CameraConfig {
                    device_id: approved.into(),
                    width: 640,
                    height: 480,
                    fps: 30
                },
                vnilink::BackendStartup::new(Duration::from_secs(5))
            ),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn identity_binds_serial_and_rejects_odd_values() {
        let plain = node("p", "d", "c", "b", None);
        assert_eq!(identity_of(&plain), "v4l2:b:d:c");
        let bound = node("p", "d", "c", "b", Some("A1"));
        assert_eq!(identity_of(&bound), "v4l2:b:d:c:A1");
        assert_eq!(clean_serial("  A1\n"), Some("A1".to_owned()));
        assert_eq!(clean_serial(""), None);
        assert_eq!(clean_serial("has:colon"), None);
        assert_eq!(clean_serial("has space"), None);
        assert_eq!(clean_serial(&"x".repeat(65)), None);
    }

    #[test]
    #[cfg(unix)]
    fn read_serial_prefers_interface_then_parent_device() {
        let root = std::env::temp_dir().join(format!("vnilink-cam-serial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let interface = root.join("real").join("2-1").join("2-1:1.0");
        std::fs::create_dir_all(&interface).unwrap();
        std::fs::create_dir_all(root.join("video0")).unwrap();
        std::os::unix::fs::symlink("../real/2-1/2-1:1.0", root.join("video0").join("device"))
            .unwrap();
        // Parent device serial only.
        std::fs::write(root.join("real").join("2-1").join("serial"), "A1\n").unwrap();
        assert_eq!(read_serial(&root, "video0").as_deref(), Some("A1"));
        // Interface serial wins when both exist.
        std::fs::write(interface.join("serial"), "B2").unwrap();
        assert_eq!(read_serial(&root, "video0").as_deref(), Some("B2"));
        // A corrupt interface value falls through to the parent.
        std::fs::write(interface.join("serial"), "has:colon").unwrap();
        assert_eq!(read_serial(&root, "video0").as_deref(), Some("A1"));
        // Nothing exposed anywhere, and an unknown node.
        std::fs::remove_file(interface.join("serial")).unwrap();
        std::fs::remove_file(root.join("real").join("2-1").join("serial")).unwrap();
        assert_eq!(read_serial(&root, "video0"), None);
        assert_eq!(read_serial(&root, "video9"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    #[cfg(unix)]
    fn startup_owns_descriptor_after_factory_retirement() {
        use std::os::fd::AsRawFd;
        struct FileHeld(std::fs::File);
        impl HeldNode for FileHeld {
            fn alive(&self) -> bool {
                true
            }
            fn open_path(&self) -> String {
                format!("/dev/fd/{}", self.0.as_raw_fd())
            }
        }
        let dir = std::env::temp_dir().join(format!("vnilink-camera-pin-{}", rand_id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("approved"), b"approved-A").unwrap();
        std::fs::write(dir.join("replacement"), b"unapproved-B").unwrap();
        let (inspector, _) = scripted(vec![]);
        let factory =
            LinuxCameraFactory::with_inspector(vec!["v4l2:b:d:c".into()], Box::new(inspector))
                .unwrap();
        factory.state.lock().unwrap().held.insert(
            "v4l2:b:d:c".into(),
            Arc::new(FileHeld(std::fs::File::open(dir.join("approved")).unwrap())),
        );
        let startup = factory
            .held_for_start("v4l2:b:d:c", &None)
            .unwrap()
            .unwrap();
        let path = startup.open_path();
        let mut state = factory.state.lock().unwrap();
        state.held.remove("v4l2:b:d:c");
        state.invalidated.insert("v4l2:b:d:c".into());
        drop(state);
        let _replacement = std::fs::File::open(dir.join("replacement")).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"approved-A");
        assert!(factory.held_for_start("v4l2:b:d:c", &None).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    fn rand_id() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
