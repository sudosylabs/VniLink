//! One monitor from a portal PipeWire node.
//!
//! [`crate::dbus::open_monitor`] returns the remote file descriptor and stores
//! the node id. This thread connects to that node and copies BGRA or BGRx
//! into a [`vnilink_media::RawFrame`]. Memory buffers are copied directly.
//! DMA-BUF buffers are imported through GBM. A modifier GBM cannot map is
//! refused. The bytes are screen content: do not log them. One connected
//! node is that compositor's permission, not universal unattended access.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa::param::video::VideoFormat;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::Pod;
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
use pw::stream::StreamFlags;
use vnilink_media::{Error, RawFrame};

use crate::dmabuf::{
    Importer, Plane, DRM_FORMAT_ARGB8888, DRM_FORMAT_MOD_INVALID, DRM_FORMAT_MOD_LINEAR,
    DRM_FORMAT_XRGB8888,
};
use crate::pixels::{raw_frame_from_bgra, raw_frame_from_bgrx};

const OPEN_TIMEOUT: Duration = Duration::from_secs(3);
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

struct Shared {
    size: Mutex<Option<(u16, u16)>>,
    frame: Mutex<Option<Vec<u8>>>,
    stride: Mutex<usize>,
    format: Mutex<Option<VideoFormat>>,
    modifier: Mutex<u64>,
    modifier_explicit: AtomicBool,
    generation: AtomicU64,
    delivery: Mutex<Option<PortalDelivery>>,
    failure: Mutex<Option<&'static str>>,
}

struct ListenerState {
    shared: Arc<Shared>,
    importer: Importer,
}

enum Command {
    Frame {
        seen: u64,
        timestamp_us: u64,
        reply: Sender<Result<(RawFrame, u64, PortalDelivery), Error>>,
    },
    Stop,
}

/// Pixel arrangement of one copied portal frame. The picture itself is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortalDelivery {
    pub format: PortalFormat,
    pub buffer: PortalBuffer,
}

/// BGRA or BGRx, matching the bytes [`PortalCapture`] copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalFormat {
    Bgra,
    Bgrx,
}

/// Where the copied bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalBuffer {
    Memory,
    /// GBM produced a linear view of this DMA-BUF.
    DmaBuf {
        modifier: u64,
        explicit: bool,
    },
}

/// A PipeWire capture started from a portal remote.
pub struct PortalCapture {
    width: u16,
    height: u16,
    seen: AtomicU64,
    delivery: Mutex<Option<PortalDelivery>>,
    tx: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl PortalCapture {
    /// Connect `remote` to `node_id` and wait until the node names its size.
    ///
    /// PipeWire closes `remote`. The call fails when the node does not offer
    /// BGRA or BGRx, or when no format arrives before the timeout. A DMA-BUF
    /// whose modifier GBM cannot map fails when the first frame is read.
    pub fn open(remote: OwnedFd, node_id: u32) -> Result<Self, Error> {
        let shared = Arc::new(Shared {
            size: Mutex::new(None),
            frame: Mutex::new(None),
            stride: Mutex::new(0),
            format: Mutex::new(None),
            modifier: Mutex::new(0),
            modifier_explicit: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            delivery: Mutex::new(None),
            failure: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("vnilink-pipewire".to_string())
            .spawn(move || capture_thread(remote, node_id, shared, rx, ready_tx))
            .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
        match ready_rx.recv_timeout(OPEN_TIMEOUT + Duration::from_secs(5)) {
            Ok(Ok((width, height))) => Ok(Self {
                width,
                height,
                seen: AtomicU64::new(0),
                delivery: Mutex::new(None),
                tx,
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::CaptureRejected(
                    "the screen cast portal did not start",
                ))
            }
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Arrangement of the frame most recently returned by [`Self::next_frame`].
    pub fn last_delivery(&self) -> Option<PortalDelivery> {
        *self.delivery.lock().expect("portal delivery")
    }

    /// Block until a frame newer than the last one is copied.
    pub fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::Frame {
                seen: self.seen.load(Ordering::Relaxed),
                timestamp_us,
                reply: reply_tx,
            })
            .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
        match reply_rx.recv_timeout(FRAME_TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok((frame, generation, delivery))) => {
                self.seen.store(generation, Ordering::Relaxed);
                *self.delivery.lock().expect("portal delivery") = Some(delivery);
                Ok(frame)
            }
            Ok(Err(err)) => Err(err),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => Err(
                Error::CaptureRejected("the screen cast portal did not start"),
            ),
        }
    }
}

impl Drop for PortalCapture {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn capture_thread(
    remote: OwnedFd,
    node_id: u32,
    shared: Arc<Shared>,
    commands: Receiver<Command>,
    ready: Sender<Result<(u16, u16), Error>>,
) {
    pw::init();
    if let Err(err) = run(remote, node_id, &shared, &commands, &ready) {
        let _ = ready.send(Err(err));
    }
}

fn run(
    remote: OwnedFd,
    node_id: u32,
    shared: &Arc<Shared>,
    commands: &Receiver<Command>,
    ready: &Sender<Result<(u16, u16), Error>>,
) -> Result<(), Error> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    let core = context
        .connect_fd(remote, None)
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "vnilink-portal",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;

    let mut importer = Importer::new();
    let modifiers = importer.modifiers();
    let blobs = enum_formats(&modifiers)?;
    let mut params = Vec::with_capacity(blobs.len());
    for blob in &blobs {
        params.push(Pod::from_bytes(blob).ok_or(Error::CaptureRejected(
            "the screen cast portal did not start",
        ))?);
    }
    let listener_state = ListenerState {
        shared: Arc::clone(shared),
        importer,
    };
    let _listener = stream
        .add_local_listener_with_user_data(listener_state)
        .state_changed(|_, state, _old, new| {
            if matches!(new, pw::stream::StreamState::Error(_)) {
                reject(&state.shared, "this PipeWire pixel format");
            }
        })
        .param_changed(|_, state, id, param| remember_format(&state.shared, id, param))
        .process(|stream, state| copy_buffer(stream, &state.shared, &mut state.importer))
        .register()
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            params.as_mut_slice(),
        )
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;

    let started = Instant::now();
    let size = loop {
        if let Some(reason) = rejected(shared) {
            return Err(Error::Unsupported(reason));
        }
        if let Some(size) = *shared.size.lock().expect("portal size") {
            break size;
        }
        if started.elapsed() > OPEN_TIMEOUT {
            return Err(Error::CaptureRejected(
                "the screen cast portal did not start",
            ));
        }
        let _ = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
    };
    let _ = ready.send(Ok(size));

    loop {
        match commands.try_recv() {
            Ok(Command::Stop) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                let _ = mainloop
                    .loop_()
                    .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
            }
            Ok(Command::Frame {
                seen,
                timestamp_us,
                reply,
            }) => {
                let _ = reply.send(wait_frame(&mainloop, shared, seen, timestamp_us));
            }
        }
    }
    Ok(())
}

fn wait_frame(
    mainloop: &pw::main_loop::MainLoopRc,
    shared: &Shared,
    seen: u64,
    timestamp_us: u64,
) -> Result<(RawFrame, u64, PortalDelivery), Error> {
    let deadline = Instant::now() + FRAME_TIMEOUT;
    loop {
        if let Some(reason) = rejected(shared) {
            return Err(Error::Unsupported(reason));
        }
        let generation = shared.generation.load(Ordering::Relaxed);
        if generation > seen {
            let frame = take_frame(shared, timestamp_us)?;
            let delivery = shared
                .delivery
                .lock()
                .expect("portal delivery")
                .ok_or(Error::Unsupported("this PipeWire pixel format"))?;
            return Ok((frame, generation, delivery));
        }
        if Instant::now() >= deadline {
            return Err(Error::CaptureRejected(
                "the screen cast portal did not start",
            ));
        }
        let _ = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
    }
}

fn take_frame(shared: &Shared, timestamp_us: u64) -> Result<RawFrame, Error> {
    let (width, height) =
        shared
            .size
            .lock()
            .expect("portal size")
            .ok_or(Error::CaptureRejected(
                "the screen cast portal did not start",
            ))?;
    let bytes =
        shared
            .frame
            .lock()
            .expect("portal frame")
            .clone()
            .ok_or(Error::CaptureRejected(
                "the screen cast portal did not start",
            ))?;
    let stride = *shared.stride.lock().expect("portal stride");
    let format = shared
        .format
        .lock()
        .expect("portal format")
        .ok_or(Error::Unsupported("this PipeWire pixel format"))?;
    frame_from_bytes(format, width, height, stride, timestamp_us, &bytes)
}

fn reject(shared: &Shared, reason: &'static str) {
    let mut failure = shared.failure.lock().expect("portal format");
    if failure.is_none() {
        *failure = Some(reason);
    }
}

fn rejected(shared: &Shared) -> Option<&'static str> {
    *shared.failure.lock().expect("portal format")
}

fn remember_format(shared: &Shared, id: u32, param: Option<&Pod>) {
    let Some(param) = param else {
        return;
    };
    if id != pw::spa::param::ParamType::Format.as_raw() {
        return;
    }
    let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param) else {
        return;
    };
    if media_type != pw::spa::param::format::MediaType::Video
        || media_subtype != pw::spa::param::format::MediaSubtype::Raw
    {
        return;
    }
    let mut info = pw::spa::param::video::VideoInfoRaw::default();
    if info.parse(param).is_err() {
        reject(shared, "this PipeWire pixel format");
        return;
    }
    if info
        .flags()
        .contains(pw::spa::param::video::VideoFlags::MODIFIER_FIXATION_REQUIRED)
    {
        return;
    }
    let width = u16::try_from(info.size().width).unwrap_or(0);
    let height = u16::try_from(info.size().height).unwrap_or(0);
    if width == 0 || height == 0 {
        reject(shared, "this PipeWire pixel format");
        return;
    }
    let format = info.format();
    if !matches!(format, VideoFormat::BGRA | VideoFormat::BGRx) {
        reject(shared, "this PipeWire pixel format");
        return;
    }
    let explicit = info
        .flags()
        .contains(pw::spa::param::video::VideoFlags::MODIFIER);
    shared.modifier_explicit.store(explicit, Ordering::Relaxed);
    *shared.modifier.lock().expect("portal format") = info.modifier();
    *shared.format.lock().expect("portal format") = Some(format);
    *shared.size.lock().expect("portal size") = Some((width, height));
}

pub(crate) fn delivery_of(
    format: VideoFormat,
    dma_buf: bool,
    modifier: u64,
    explicit: bool,
) -> Option<PortalDelivery> {
    let format = match format {
        VideoFormat::BGRA => PortalFormat::Bgra,
        VideoFormat::BGRx => PortalFormat::Bgrx,
        _ => return None,
    };
    let buffer = if dma_buf {
        PortalBuffer::DmaBuf { modifier, explicit }
    } else {
        PortalBuffer::Memory
    };
    Some(PortalDelivery { format, buffer })
}

fn record_delivery(shared: &Shared, dma_buf: bool) {
    let Some(format) = *shared.format.lock().expect("portal format") else {
        return;
    };
    let modifier = *shared.modifier.lock().expect("portal format");
    let explicit = shared.modifier_explicit.load(Ordering::Relaxed);
    *shared.delivery.lock().expect("portal delivery") =
        delivery_of(format, dma_buf, modifier, explicit);
}

fn drm_format(format: VideoFormat) -> Option<u32> {
    match format {
        VideoFormat::BGRA => Some(DRM_FORMAT_ARGB8888),
        VideoFormat::BGRx => Some(DRM_FORMAT_XRGB8888),
        _ => None,
    }
}

pub(crate) fn enum_formats(modifiers: &[u64]) -> Result<Vec<Vec<u8>>, Error> {
    format_objects(modifiers)
        .into_iter()
        .map(serialize_object)
        .collect()
}

pub(crate) fn format_objects(modifiers: &[u64]) -> Vec<pw::spa::pod::Object> {
    let mut offered: Vec<i64> = modifiers
        .iter()
        .take(126)
        .map(|modifier| *modifier as i64)
        .collect();
    let linear = DRM_FORMAT_MOD_LINEAR as i64;
    let invalid = DRM_FORMAT_MOD_INVALID as i64;
    if !offered.contains(&linear) {
        offered.push(linear);
    }
    if !offered.contains(&invalid) {
        offered.push(invalid);
    }
    let mut with_modifier = video_properties();
    with_modifier.push(pw::spa::pod::Property {
        key: pw::spa::param::format::FormatProperties::VideoModifier.as_raw(),
        flags: pw::spa::pod::PropertyFlags::MANDATORY | pw::spa::pod::PropertyFlags::DONT_FIXATE,
        value: pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Long(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: offered[0],
                alternatives: offered,
            },
        ))),
    });
    vec![object_from(with_modifier), object_from(video_properties())]
}

fn video_properties() -> Vec<pw::spa::pod::Property> {
    vec![
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaType,
            Id,
            pw::spa::param::format::MediaType::Video
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::MediaSubtype,
            Id,
            pw::spa::param::format::MediaSubtype::Raw
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRA,
            VideoFormat::BGRA,
            VideoFormat::BGRx,
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pw::spa::utils::Rectangle {
                width: 1920,
                height: 1080
            },
            pw::spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            pw::spa::utils::Rectangle {
                width: 7680,
                height: 7680
            }
        ),
        pw::spa::pod::property!(
            pw::spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pw::spa::utils::Fraction { num: 30, denom: 1 },
            pw::spa::utils::Fraction { num: 0, denom: 1 },
            pw::spa::utils::Fraction { num: 60, denom: 1 }
        ),
    ]
}

fn object_from(properties: Vec<pw::spa::pod::Property>) -> pw::spa::pod::Object {
    pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties,
    }
}

fn serialize_object(value: pw::spa::pod::Object) -> Result<Vec<u8>, Error> {
    PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(value),
    )
    .map(|serialized| serialized.0.into_inner())
    .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))
}

fn frame_from_bytes(
    format: VideoFormat,
    width: u16,
    height: u16,
    stride: usize,
    timestamp_us: u64,
    bytes: &[u8],
) -> Result<RawFrame, Error> {
    match format {
        VideoFormat::BGRA => raw_frame_from_bgra(width, height, stride, timestamp_us, bytes),
        VideoFormat::BGRx => raw_frame_from_bgrx(width, height, stride, timestamp_us, bytes),
        _ => Err(Error::Unsupported("this PipeWire pixel format")),
    }
}

fn copy_buffer(stream: &pw::stream::Stream, shared: &Shared, importer: &mut Importer) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    if datas[0].type_() == pw::spa::buffer::DataType::DmaBuf {
        copy_dmabuf(datas, shared, importer);
        return;
    }
    let data = &mut datas[0];
    let (stride, offset, size) = {
        let chunk = data.chunk();
        (
            usize::try_from(chunk.stride()).unwrap_or(0),
            usize::try_from(chunk.offset()).unwrap_or(usize::MAX),
            usize::try_from(chunk.size()).unwrap_or(0),
        )
    };
    let Some(bytes) = data.data() else {
        reject(shared, "this PipeWire pixel format");
        return;
    };
    let end = offset.saturating_add(size);
    if size == 0 || end > bytes.len() {
        return;
    }
    *shared.stride.lock().expect("portal stride") = if stride == 0 {
        bytes[offset..end].len()
    } else {
        stride
    };
    *shared.frame.lock().expect("portal frame") = Some(bytes[offset..end].to_vec());
    record_delivery(shared, false);
    shared.generation.fetch_add(1, Ordering::Relaxed);
}

fn copy_dmabuf(datas: &mut [pw::spa::buffer::Data], shared: &Shared, importer: &mut Importer) {
    let mut planes = Vec::new();
    for data in datas.iter() {
        if data.type_() != pw::spa::buffer::DataType::DmaBuf {
            break;
        }
        let chunk = data.chunk();
        let Ok(offset) = i32::try_from(chunk.offset()) else {
            reject(shared, "this DMA-BUF modifier");
            return;
        };
        planes.push(Plane {
            fd: data.fd(),
            stride: chunk.stride(),
            offset,
        });
    }
    let Some((width, height)) = *shared.size.lock().expect("portal size") else {
        return;
    };
    let Some(format) = *shared.format.lock().expect("portal format") else {
        reject(shared, "this PipeWire pixel format");
        return;
    };
    let Some(drm_format) = drm_format(format) else {
        reject(shared, "this PipeWire pixel format");
        return;
    };
    let modifier = *shared.modifier.lock().expect("portal format");
    let explicit = shared.modifier_explicit.load(Ordering::Relaxed);
    let Some((bytes, stride)) = importer.map(
        u32::from(width),
        u32::from(height),
        drm_format,
        modifier,
        explicit,
        &planes,
    ) else {
        reject(shared, "this DMA-BUF modifier");
        return;
    };
    *shared.stride.lock().expect("portal stride") = stride;
    *shared.frame.lock().expect("portal frame") = Some(bytes);
    record_delivery(shared, true);
    shared.generation.fetch_add(1, Ordering::Relaxed);
}
