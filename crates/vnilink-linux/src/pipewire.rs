//! One monitor from a portal PipeWire node.
//!
//! [`crate::dbus::open_monitor`] returns the remote file descriptor and stores
//! the node id. This thread connects to that node and copies BGRA or BGRx
//! into a [`vnilink_media::RawFrame`]. Memory buffers are copied directly.
//! The live portal negotiates MemFd/MemPtr buffers. DMA-BUF import is not used:
//! a Mesa GBM unmap abort was observed on a real Wayland desktop.
//! The bytes are screen content: do not log them. One connected node is that
//! compositor's permission, not universal unattended access.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixDatagram;
use std::sync::atomic::{AtomicBool, Ordering};
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
use vnilink_media::{ChannelSlot, Error, FrameChannel, RawFrame};

use crate::pixels::{raw_frame_from_bgra, raw_frame_from_bgrx};

// drm_fourcc.h: sentinel retained for delivery metadata. No GBM calls in this path.
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const OPEN_TIMEOUT: Duration = Duration::from_secs(3);
const FRAME_TIMEOUT: Duration = Duration::from_millis(100);

struct Shared {
    cursor: Mutex<Option<vnilink::CursorState>>,
    size: Mutex<Option<(u16, u16)>>,
    channel: FrameChannel,
    format: Mutex<Option<VideoFormat>>,
    modifier: Mutex<u64>,
    modifier_explicit: AtomicBool,
    delivery: Mutex<Option<PortalDelivery>>,
}

struct ListenerState {
    shared: Arc<Shared>,
}

enum Command {
    Frame {
        timestamp_us: u64,
        reply: Sender<Result<Option<(RawFrame, PortalDelivery)>, Error>>,
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
    delivery: Mutex<Option<PortalDelivery>>,
    tx: Sender<Command>,
    wake: UnixDatagram,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl PortalCapture {
    /// Connect `remote` to `node_id` and wait until the node names its size.
    ///
    /// PipeWire closes `remote`. The call fails when the node does not offer
    /// BGRA or BGRx, or when no format arrives before the timeout. Unexpected
    /// GPU buffers are refused before import.
    pub fn open(
        remote: OwnedFd,
        node_id: u32,
        startup: vnilink::BackendStartup,
    ) -> Result<Self, Error> {
        startup
            .check()
            .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))?;
        let shared = Arc::new(Shared {
            cursor: Mutex::new(None),
            size: Mutex::new(None),
            channel: FrameChannel::new(),
            format: Mutex::new(None),
            modifier: Mutex::new(0),
            modifier_explicit: AtomicBool::new(false),
            delivery: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        let (wake, wake_rx) = command_wakeup()?;
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("vnilink-pipewire".to_string())
            .spawn({
                let startup = startup.clone();
                move || {
                    capture_thread(
                        remote,
                        node_id,
                        worker_shared,
                        rx,
                        wake_rx,
                        ready_tx,
                        startup,
                    )
                }
            })
            .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
        match startup.receive(&ready_rx) {
            Ok(Ok((width, height))) => {
                let capture = Self {
                    width,
                    height,
                    delivery: Mutex::new(None),
                    tx,
                    wake,
                    thread: Some(thread),
                    shared,
                };
                startup.check().map_err(|_| {
                    Error::CaptureRejected("desktop capture startup cancelled or expired")
                })?;
                Ok(capture)
            }
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

    pub fn cursor(&self) -> Option<vnilink::CursorState> {
        self.shared
            .cursor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// When PipeWire last pushed a picture. Idle polls do not move this.
    pub fn attested_at(&self) -> Option<std::time::Instant> {
        self.shared.channel.pushed_at()
    }

    /// Arrangement of the frame most recently returned by [`Self::poll_frame`].
    pub fn last_delivery(&self) -> Option<PortalDelivery> {
        *self.delivery.lock().expect("portal delivery")
    }

    /// Return the latest picture, retaining it while the desktop is idle.
    pub fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Command::Frame {
                timestamp_us,
                reply: reply_tx,
            })
            .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
        let _ = self.wake.send(&[1]);
        match reply_rx.recv_timeout(FRAME_TIMEOUT) {
            Ok(Ok(Some((frame, delivery)))) => {
                *self.delivery.lock().expect("portal delivery") = Some(delivery);
                Ok(Some(frame))
            }
            Ok(Ok(None)) => Ok(None),
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
        let _ = self.wake.send(&[1]);
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
    wake: UnixDatagram,
    ready: Sender<Result<(u16, u16), Error>>,
    startup: vnilink::BackendStartup,
) {
    pw::init();
    if let Err(err) = run(remote, node_id, &shared, &commands, wake, &ready, &startup) {
        let _ = ready.send(Err(err));
    }
}

fn run(
    remote: OwnedFd,
    node_id: u32,
    shared: &Arc<Shared>,
    commands: &Receiver<Command>,
    wake: UnixDatagram,
    ready: &Sender<Result<(u16, u16), Error>>,
    startup: &vnilink::BackendStartup,
) -> Result<(), Error> {
    let check = || {
        startup
            .check()
            .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))
    };
    check()?;
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

    // Do not probe EGL/GBM or offer modifiers on the production portal path.
    let blobs = enum_formats(&[])?;
    let mut params = Vec::with_capacity(blobs.len());
    for blob in &blobs {
        params.push(Pod::from_bytes(blob).ok_or(Error::CaptureRejected(
            "the screen cast portal did not start",
        ))?);
    }
    let listener_state = ListenerState {
        shared: Arc::clone(shared),
    };
    let _listener = stream
        .add_local_listener_with_user_data(listener_state)
        .state_changed(|_, state, _old, new| {
            if matches!(new, pw::stream::StreamState::Error(_)) {
                state.shared.channel.fail("this PipeWire pixel format");
            }
        })
        .param_changed(|stream, state, id, param| {
            remember_format(&state.shared, id, param);
            if param.is_some() && id == pw::spa::param::ParamType::Format.as_raw() {
                let updated = (|| {
                    let bytes = serialize_object(memory_buffer_object())?;
                    let cursor_bytes = serialize_object(cursor_meta_object())?;
                    let pod = Pod::from_bytes(&bytes)
                        .ok_or(Error::Unsupported("memory buffer negotiation"))?;
                    stream
                        .update_params(&mut [
                            pod,
                            Pod::from_bytes(&cursor_bytes)
                                .ok_or(Error::Unsupported("cursor metadata negotiation"))?,
                        ])
                        .map_err(|_| Error::Unsupported("memory buffer negotiation"))
                })();
                if updated.is_err() {
                    state
                        .shared
                        .channel
                        .fail("PipeWire shared-memory buffer negotiation failed");
                }
            }
        })
        .process(|stream, state| copy_buffer(stream, &state.shared))
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
        check()?;
        if let Some(reason) = shared.channel.failure() {
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
    check()?;
    ready
        .send(Ok(size))
        .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))?;

    dispatch_commands(&mainloop, shared, commands, wake)
}

fn command_wakeup() -> Result<(UnixDatagram, UnixDatagram), Error> {
    let (sender, receiver) = UnixDatagram::pair()
        .map_err(|_| Error::CaptureRejected("the capture command wakeup did not start"))?;
    sender
        .set_nonblocking(true)
        .and_then(|()| receiver.set_nonblocking(true))
        .map_err(|_| Error::CaptureRejected("the capture command wakeup did not start"))?;
    Ok((sender, receiver))
}

fn dispatch_commands(
    mainloop: &pw::main_loop::MainLoopRc,
    shared: &Shared,
    commands: &Receiver<Command>,
    wake: UnixDatagram,
) -> Result<(), Error> {
    // Wake an idle PipeWire poll as soon as capture or Stop is requested.
    // The socket only carries notifications; commands and pixels stay separate.
    let _wake_source =
        mainloop
            .loop_()
            .add_io(wake, pw::spa::support::system::IoFlags::IN, |socket| {
                let mut bytes = [0; 64];
                while socket.recv(&mut bytes).is_ok() {}
            });
    loop {
        match commands.try_recv() {
            Ok(Command::Stop) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                let _ = mainloop
                    .loop_()
                    .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
            }
            Ok(Command::Frame {
                timestamp_us,
                reply,
            }) => {
                let _ = reply.send(wait_frame(&mainloop, shared, timestamp_us));
            }
        }
    }
    Ok(())
}

fn wait_frame(
    mainloop: &pw::main_loop::MainLoopRc,
    shared: &Shared,
    timestamp_us: u64,
) -> Result<Option<(RawFrame, PortalDelivery)>, Error> {
    // Drain ready compositor events without sleeping before using cached pixels.
    // Waiting 50ms here and again in dispatch caps even idle capture at ~10fps.
    let _ = mainloop
        .loop_()
        .iterate(pw::loop_::Timeout::Finite(Duration::ZERO));
    let slot = match shared.channel.try_latest()? {
        Some(slot) => slot,
        None => return Ok(None),
    };
    let delivery = shared
        .delivery
        .lock()
        .expect("portal delivery")
        .ok_or(Error::Unsupported("this PipeWire pixel format"))?;
    let format = shared
        .format
        .lock()
        .expect("portal format")
        .ok_or(Error::Unsupported("this PipeWire pixel format"))?;
    let frame = frame_from_bytes(
        format,
        slot.width,
        slot.height,
        slot.stride,
        timestamp_us,
        &slot.bytes,
    )?;
    Ok(Some((frame, delivery)))
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
        shared.channel.fail("this PipeWire pixel format");
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
        shared.channel.fail("this PipeWire pixel format");
        return;
    }
    let format = info.format();
    if !matches!(format, VideoFormat::BGRA | VideoFormat::BGRx) {
        shared.channel.fail("this PipeWire pixel format");
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

pub(crate) fn enum_formats(modifiers: &[u64]) -> Result<Vec<Vec<u8>>, Error> {
    format_objects(modifiers)
        .into_iter()
        .map(serialize_object)
        .collect()
}

pub(crate) fn format_objects(_modifiers: &[u64]) -> Vec<pw::spa::pod::Object> {
    vec![object_from(video_properties())]
}

pub(crate) fn memory_buffer_object() -> pw::spa::pod::Object {
    let mask = (1 << pw::spa::buffer::DataType::MemPtr.as_raw())
        | (1 << pw::spa::buffer::DataType::MemFd.as_raw());
    pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: pw::spa::param::ParamType::Buffers.as_raw(),
        properties: vec![pw::spa::pod::Property {
            key: pw::spa::sys::SPA_PARAM_BUFFERS_dataType,
            flags: pw::spa::pod::PropertyFlags::MANDATORY,
            value: pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: mask,
                    flags: vec![],
                },
            ))),
        }],
    }
}

pub(crate) fn accepts_memory_type(kind: pw::spa::buffer::DataType) -> bool {
    matches!(
        kind,
        pw::spa::buffer::DataType::MemPtr | pw::spa::buffer::DataType::MemFd
    )
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

/// Owns one dequeued buffer until every borrowed pixel/metadata slice is gone.
/// The high-level wrapper exposes only fixed-size metadata, whereas compositor
/// cursor allocations vary. Read the actual SPA metadata size before parsing.
struct CaptureBuffer<'a> {
    raw: std::ptr::NonNull<pw::sys::pw_buffer>,
    stream: &'a pw::stream::Stream,
}
impl<'a> CaptureBuffer<'a> {
    fn dequeue(stream: &'a pw::stream::Stream) -> Option<Self> {
        // SAFETY: dequeue is called only from this stream's process callback.
        std::ptr::NonNull::new(unsafe { stream.dequeue_raw_buffer() })
            .map(|raw| Self { raw, stream })
    }
    fn cursor_bytes(&self) -> Option<&[u8]> {
        // SAFETY: the dequeued buffer and its metadata remain owned until Drop.
        let buffer = unsafe { self.raw.as_ref().buffer.as_ref()? };
        unsafe { cursor_metadata(buffer) }
    }
    fn datas_mut(&mut self) -> &mut [pw::spa::buffer::Data] {
        // SAFETY: like pipewire::Buffer::datas_mut, borrow the stream-owned SPA
        // array exclusively; Data is repr(transparent) over spa_data.
        let Some(buffer) = (unsafe { self.raw.as_ref().buffer.as_ref() }) else {
            return &mut [];
        };
        if buffer.n_datas == 0 || buffer.datas.is_null() {
            return &mut [];
        }
        unsafe { std::slice::from_raw_parts_mut(buffer.datas.cast(), buffer.n_datas as usize) }
    }
}
impl Drop for CaptureBuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: this exact pointer was dequeued from this stream; return once.
        unsafe {
            self.stream.queue_raw_buffer(self.raw.as_ptr());
        }
    }
}

/// Safety: the SPA descriptor arrays and every advertised allocation must stay
/// readable for the borrowed buffer's lifetime, as guaranteed while dequeued.
unsafe fn cursor_metadata(buffer: &pw::spa::sys::spa_buffer) -> Option<&[u8]> {
    if buffer.n_metas == 0 || buffer.metas.is_null() {
        return None;
    }
    let metas = unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
    let meta = metas
        .iter()
        .find(|meta| meta.type_ == pw::spa::sys::SPA_META_Cursor)?;
    if meta.data.is_null() || !(28..=crate::cursor::META_SIZE).contains(&(meta.size as usize)) {
        return None;
    }
    // SAFETY: SPA owns `size` readable bytes. The parser checks every offset
    // against this allocation, including the trailing bitmap and stride.
    Some(unsafe { std::slice::from_raw_parts(meta.data.cast::<u8>(), meta.size as usize) })
}

fn copy_buffer(stream: &pw::stream::Stream, shared: &Shared) {
    let Some(mut buffer) = CaptureBuffer::dequeue(stream) else {
        return;
    };
    if let Some(meta) = buffer.cursor_bytes() {
        let mut cursor = shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(update) = crate::cursor::parse(
            meta,
            cursor.as_ref(),
            VideoFormat::BGRA.as_raw(),
            VideoFormat::RGBA.as_raw(),
        ) {
            *cursor = Some(update);
        }
    }
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    if !accepts_memory_type(datas[0].type_()) {
        shared
            .channel
            .fail("PipeWire delivered a GPU buffer instead of shared memory");
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
        shared.channel.fail("this PipeWire pixel format");
        return;
    };
    let end = offset.saturating_add(size);
    if size == 0 || end > bytes.len() {
        return;
    }
    let Some((width, height)) = *shared.size.lock().expect("portal size") else {
        return;
    };
    let stride = if stride == 0 {
        bytes[offset..end].len()
    } else {
        stride
    };
    shared.channel.push(ChannelSlot {
        width,
        height,
        stride,
        bytes: Arc::new(bytes[offset..end].to_vec()),
        captured_at: None,
    });
    record_delivery(shared, false);
}

#[cfg(test)]
#[path = "tests/pipewire_idle.rs"]
mod idle_tests;

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn cancelled_startup_closes_remote_fd_without_opening_pipewire() {
        use std::io::Read;
        let (remote, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let startup = vnilink::BackendStartup::new(Duration::ZERO);
        assert!(matches!(
            PortalCapture::open(remote.into(), 42, startup),
            Err(Error::CaptureRejected(
                "desktop capture startup cancelled or expired"
            ))
        ));
        assert_eq!(
            peer.read(&mut [0]).unwrap(),
            0,
            "cancelled capture must close the owned remote fd"
        );
    }

    #[test]
    fn cached_capture_commands_meet_a_60fps_frame_budget() {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (wake, wake_rx) = command_wakeup().unwrap();
        let worker = thread::spawn(move || {
            pw::init();
            let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
            let shared = Arc::new(Shared {
                cursor: Mutex::new(None),
                size: Mutex::new(Some((8, 8))),
                channel: FrameChannel::new(),
                format: Mutex::new(Some(VideoFormat::BGRA)),
                modifier: Mutex::new(0),
                modifier_explicit: AtomicBool::new(false),
                delivery: Mutex::new(Some(PortalDelivery {
                    format: PortalFormat::Bgra,
                    buffer: PortalBuffer::Memory,
                })),
            });
            ready_tx.send(Arc::clone(&shared)).unwrap();
            dispatch_commands(&mainloop, &shared, &rx, wake_rx).unwrap();
        });
        let shared = ready_rx.recv().unwrap();
        for timestamp_us in 0..20 {
            let (reply, result) = mpsc::channel();
            tx.send(Command::Frame {
                timestamp_us,
                reply,
            })
            .unwrap();
            wake.send(&[1]).unwrap();
            assert!(result
                .recv_timeout(Duration::from_millis(100))
                .unwrap()
                .unwrap()
                .is_none());
        }
        shared.channel.push(ChannelSlot {
            width: 8,
            height: 8,
            stride: 32,
            bytes: Arc::new(vec![255; 256]),
            captured_at: None,
        });
        let started = Instant::now();
        for timestamp_us in 0..20 {
            let (reply, result) = mpsc::channel();
            tx.send(Command::Frame {
                timestamp_us,
                reply,
            })
            .unwrap();
            wake.send(&[1]).unwrap();
            let (frame, _) = result
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!((frame.width(), frame.height()), (8, 8));
        }
        let elapsed = started.elapsed();
        tx.send(Command::Stop).unwrap();
        wake.send(&[1]).unwrap();
        worker.join().unwrap();
        assert!(
            elapsed < Duration::from_micros(20 * 16_667),
            "20 cached capture requests took {elapsed:?}; blocking event polling caps frame delivery below 60fps"
        );
    }
}

fn cursor_meta_object() -> pw::spa::pod::Object {
    pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: pw::spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            pw::spa::pod::Property {
                key: pw::spa::sys::SPA_PARAM_META_type,
                flags: pw::spa::pod::PropertyFlags::empty(),
                value: pw::spa::pod::Value::Id(pw::spa::utils::Id(pw::spa::sys::SPA_META_Cursor)),
            },
            pw::spa::pod::Property {
                key: pw::spa::sys::SPA_PARAM_META_size,
                flags: pw::spa::pod::PropertyFlags::empty(),
                value: pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: 28 + 20 + 64 * 64 * 4,
                        min: 28,
                        max: crate::cursor::META_SIZE as i32,
                    },
                ))),
            },
        ],
    }
}

#[cfg(test)]
mod cursor_buffer_tests {
    use super::*;
    #[test]
    fn metadata_uses_actual_allocation_instead_of_requiring_the_maximum() {
        let mut bytes = [0u8; 64];
        let mut meta = pw::spa::sys::spa_meta {
            type_: pw::spa::sys::SPA_META_Cursor,
            size: bytes.len() as u32,
            data: bytes.as_mut_ptr().cast(),
        };
        let mut buffer = pw::spa::sys::spa_buffer {
            n_metas: 1,
            n_datas: 0,
            metas: &mut meta,
            datas: std::ptr::null_mut(),
        };
        // Each advertised valid size is within `bytes`; rejected sizes must
        // never create a slice or touch the pointer.
        assert_eq!(unsafe { cursor_metadata(&buffer) }.unwrap().len(), 64);
        meta.size = 28;
        assert_eq!(unsafe { cursor_metadata(&buffer) }.unwrap().len(), 28);
        for size in [0, 27, crate::cursor::META_SIZE as u32 + 1, u32::MAX] {
            meta.size = size;
            assert!(unsafe { cursor_metadata(&buffer) }.is_none());
        }
        meta.size = 64;
        meta.data = std::ptr::null_mut();
        assert!(unsafe { cursor_metadata(&buffer) }.is_none());
        buffer.metas = std::ptr::null_mut();
        assert!(unsafe { cursor_metadata(&buffer) }.is_none());
    }
}
