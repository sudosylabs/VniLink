//! One monitor from a portal PipeWire node.
//!
//! [`crate::dbus::open_monitor`] returns the remote file descriptor and stores
//! the node id. This thread connects to that node and copies BGRA or BGRx
//! into a [`vnilink_media::RawFrame`]. Memory buffers are copied directly.
//! Asked to ([`PortalCapture::open_for_encoder`]), the capture instead
//! negotiates DMA-BUFs and keeps each picture on the GPU
//! ([`crate::gpu_picture`]): the buffer is imported into a VA-API surface
//! (DRM PRIME) and never mapped, since mapping one through GBM aborted in
//! Mesa on a real Wayland desktop. A compositor that offers no DMA-BUF
//! modifier this side imports, or an import the GPU refuses, leaves the
//! capture on shared memory, as does every capture not asked to.
//! The bytes are screen content: do not log them. One connected node is that
//! compositor's permission, not universal unattended access.

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
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
use vnilink_media::{ChannelSlot, Error, FrameChannel, RawFrame};

use crate::gpu_picture::{
    choose_modifier, damage_is_empty, Importer, Prime, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888,
    PREFERRED_MODIFIERS,
};
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
    /// Set by the process callback when the GPU path cannot go on: the
    /// capture then starts again on shared memory.
    gpu_failed: AtomicBool,
}

struct ListenerState {
    shared: Arc<Shared>,
    gpu: Option<GpuState>,
}

/// The GPU path of one stream: where pictures are imported, and what the
/// compositor and this side agreed on.
struct GpuState {
    node: PathBuf,
    modifier: Option<u64>,
    importer: Option<Importer>,
    /// Whether the compositor has ever filled in a buffer's damage: one
    /// that leaves the list zeroed would otherwise read as a screen that
    /// never changes.
    damage_filled: bool,
    /// What buffers dropped since the last picture kept changed: the next
    /// one kept shows their changes too.
    dropped: Option<vnilink_media::Changes>,
}

/// How one stream ended.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Stopped,
    /// The GPU path failed: connect again without it.
    GpuFailed,
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
    /// The channel's generation when the picture last polled was read.
    seen: AtomicU64,
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
        Self::open_on(remote, node_id, startup, None)
    }

    /// Like [`Self::open`], with pictures kept on the GPU when the encoder
    /// this machine would open reads them there: VA-API on the GPU the
    /// compositor draws on. NVENC, a machine without a GPU encoder and a
    /// compositor that cannot share its buffers get shared memory, as
    /// [`Self::open`] does.
    pub fn open_for_encoder(
        remote: OwnedFd,
        node_id: u32,
        startup: vnilink::BackendStartup,
    ) -> Result<Self, Error> {
        Self::open_on(remote, node_id, startup, crate::gpu_picture::target_node())
    }

    fn open_on(
        remote: OwnedFd,
        node_id: u32,
        startup: vnilink::BackendStartup,
        gpu: Option<PathBuf>,
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
            gpu_failed: AtomicBool::new(false),
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
                        gpu,
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
                    seen: AtomicU64::new(0),
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

    /// When PipeWire last delivered a changed picture. Idle polls, and the
    /// same picture delivered again, do not move this.
    pub fn attested_at(&self) -> Option<std::time::Instant> {
        self.shared.channel.pushed_at()
    }

    /// Arrangement of the frame most recently returned by [`Self::poll_frame`].
    pub fn last_delivery(&self) -> Option<PortalDelivery> {
        *self.delivery.lock().expect("portal delivery")
    }

    /// Return the latest picture, retaining it while the desktop is idle.
    ///
    /// The capture thread stores each picture in the channel as PipeWire
    /// delivers it and keeps iterating its loop, so a stored picture is read
    /// here without asking that thread: pictures are shared, never reused
    /// buffers, and a GPU picture is a handle the channel owns. Only a
    /// capture without a picture yet, or whose thread has ended, takes the
    /// round trip, which reports the failure.
    pub fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        // Before the read, so a picture stored during it still counts as new.
        let generation = self.shared.channel.generation();
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
        {
            if let Some(slot) = self.shared.channel.try_latest()? {
                self.seen.store(generation, Ordering::Release);
                let (frame, delivery) = frame_from_slot(&self.shared, slot, timestamp_us)?;
                let frame = frame.with_changed(self.shared.channel.take_changed());
                *self.delivery.lock().expect("portal delivery") = Some(delivery);
                return Ok(Some(frame));
            }
        }
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
                self.seen.store(generation, Ordering::Release);
                *self.delivery.lock().expect("portal delivery") = Some(delivery);
                Ok(Some(frame.with_changed(self.shared.channel.take_changed())))
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(err)) => Err(err),
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => Err(
                Error::CaptureRejected("the screen cast portal did not start"),
            ),
        }
    }

    /// Waits for a picture newer than the last poll's, until `deadline`. The
    /// PipeWire thread wakes it as it stores the picture.
    pub fn wait_frame(&self, deadline: Instant) -> bool {
        let seen = self.seen.load(Ordering::Acquire);
        self.shared.channel.wait_newer(seen, deadline) != seen
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
    gpu: Option<PathBuf>,
) {
    pw::init();
    if let Err(err) = run(
        remote, node_id, &shared, &commands, wake, &ready, &startup, gpu,
    ) {
        let _ = ready.send(Err(err));
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    remote: OwnedFd,
    node_id: u32,
    shared: &Arc<Shared>,
    commands: &Receiver<Command>,
    wake: UnixDatagram,
    ready: &Sender<Result<(u16, u16), Error>>,
    startup: &vnilink::BackendStartup,
    mut gpu: Option<PathBuf>,
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
    let mut started = false;
    loop {
        let outcome = serve(
            &mainloop,
            &core,
            node_id,
            shared,
            commands,
            &wake,
            ready,
            &check,
            gpu.take(),
            &mut started,
        )?;
        match outcome {
            Outcome::Stopped => return Ok(()),
            // The GPU path was the optimisation: carry on without it.
            Outcome::GpuFailed => shared.gpu_failed.store(false, Ordering::Relaxed),
        }
    }
}

/// One stream on `node_id` until it stops or its GPU path fails. `ready` is
/// told the picture size the first time one is known.
#[allow(clippy::too_many_arguments)]
fn serve(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::Core,
    node_id: u32,
    shared: &Arc<Shared>,
    commands: &Receiver<Command>,
    wake: &UnixDatagram,
    ready: &Sender<Result<(u16, u16), Error>>,
    check: &dyn Fn() -> Result<(), Error>,
    gpu: Option<PathBuf>,
    started: &mut bool,
) -> Result<Outcome, Error> {
    let stream = pw::stream::StreamBox::new(
        core,
        "vnilink-portal",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;

    // Modifiers are offered only to a capture that keeps pictures on the GPU.
    let offered: &[u64] = if gpu.is_some() {
        &PREFERRED_MODIFIERS
    } else {
        &[]
    };
    let blobs = enum_formats(offered)?;
    let mut params = Vec::with_capacity(blobs.len());
    for blob in &blobs {
        params.push(Pod::from_bytes(blob).ok_or(Error::CaptureRejected(
            "the screen cast portal did not start",
        ))?);
    }
    let listener_state = ListenerState {
        shared: Arc::clone(shared),
        gpu: gpu.map(|node| GpuState {
            node,
            modifier: None,
            importer: None,
            damage_filled: false,
            dropped: Some(vnilink_media::Changes::none()),
        }),
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
            if let (Some(param), true) = (param, id == pw::spa::param::ParamType::Format.as_raw()) {
                if negotiate(stream, state, param).is_err() {
                    state
                        .shared
                        .channel
                        .fail("PipeWire buffer negotiation failed");
                }
            }
        })
        .process(|stream, state| copy_buffer(stream, state))
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

    let begun = Instant::now();
    let size = loop {
        check()?;
        if shared.gpu_failed.load(Ordering::Relaxed) {
            return Ok(Outcome::GpuFailed);
        }
        if let Some(reason) = shared.channel.failure() {
            return Err(Error::Unsupported(reason));
        }
        if let Some(size) = *shared.size.lock().expect("portal size") {
            break size;
        }
        if begun.elapsed() > OPEN_TIMEOUT {
            return Err(Error::CaptureRejected(
                "the screen cast portal did not start",
            ));
        }
        let _ = mainloop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(Duration::from_millis(50)));
    };
    check()?;
    if !*started {
        ready
            .send(Ok(size))
            .map_err(|_| Error::CaptureRejected("desktop capture startup cancelled or expired"))?;
        *started = true;
    }

    // Every stream this thread connects is woken by the same socket.
    let wake = wake
        .try_clone()
        .map_err(|_| Error::CaptureRejected("the capture command wakeup did not start"))?;
    dispatch_commands(mainloop, shared, commands, wake)
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
) -> Result<Outcome, Error> {
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
        if shared.gpu_failed.load(Ordering::Relaxed) {
            return Ok(Outcome::GpuFailed);
        }
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
    Ok(Outcome::Stopped)
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
    match shared.channel.try_latest()? {
        Some(slot) => frame_from_slot(shared, slot, timestamp_us).map(Some),
        None => Ok(None),
    }
}

/// `slot` as a frame in the arrangement the stream negotiated.
fn frame_from_slot(
    shared: &Shared,
    slot: ChannelSlot,
    timestamp_us: u64,
) -> Result<(RawFrame, PortalDelivery), Error> {
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
    let frame = if slot.bytes.native_picture().is_some() {
        // A picture on the GPU: its pixels are read only if something asks.
        RawFrame::shared(
            slot.width,
            slot.height,
            slot.stride,
            timestamp_us,
            slot.bytes.clone(),
        )?
    } else {
        frame_from_bytes(
            format,
            slot.width,
            slot.height,
            slot.stride,
            timestamp_us,
            &slot.bytes,
        )?
    }
    .with_captured_at(slot.captured_at);
    Ok((frame, delivery))
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

/// What the format object asks of the buffer's layout.
enum Layout<'a> {
    /// Shared memory: no modifier.
    Memory,
    /// DMA-BUFs in any of these layouts, the compositor to choose.
    Offered(&'a [u64]),
    /// DMA-BUFs in exactly this layout, as the format is fixated.
    Fixed(u64),
}

/// The formats this side accepts, most wanted first: with `modifiers`, the
/// same format in each of those layouts and then in shared memory.
pub(crate) fn format_objects(modifiers: &[u64]) -> Vec<pw::spa::pod::Object> {
    let mut objects = Vec::new();
    if !modifiers.is_empty() {
        objects.push(object_from(video_properties(Layout::Offered(modifiers))));
    }
    objects.push(object_from(video_properties(Layout::Memory)));
    objects
}

/// The format `format` in the one layout `modifier`, to answer a compositor
/// that offered several.
pub(crate) fn fixated_format(format: VideoFormat, modifier: u64) -> pw::spa::pod::Object {
    let mut properties = video_properties(Layout::Fixed(modifier));
    let key = pw::spa::param::format::FormatProperties::VideoFormat.as_raw();
    for property in &mut properties {
        if property.key == key {
            property.value = pw::spa::pod::Value::Id(pw::spa::utils::Id(format.as_raw()));
        }
    }
    object_from(properties)
}

/// Buffers that are DMA-BUFs and nothing else.
pub(crate) fn dma_buf_object() -> pw::spa::pod::Object {
    let mask = 1 << pw::spa::buffer::DataType::DmaBuf.as_raw();
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

/// Room for the regions of a picture that changed: the compositor says
/// which part of the screen it redrew, and an empty list says nothing did.
const DAMAGE_REGIONS: i32 = 16;

pub(crate) fn damage_meta_object() -> pw::spa::pod::Object {
    pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: pw::spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            pw::spa::pod::Property {
                key: pw::spa::sys::SPA_PARAM_META_type,
                flags: pw::spa::pod::PropertyFlags::empty(),
                value: pw::spa::pod::Value::Id(pw::spa::utils::Id(
                    pw::spa::sys::SPA_META_VideoDamage,
                )),
            },
            pw::spa::pod::Property {
                key: pw::spa::sys::SPA_PARAM_META_size,
                flags: pw::spa::pod::PropertyFlags::empty(),
                value: pw::spa::pod::Value::Int(
                    DAMAGE_REGIONS * std::mem::size_of::<pw::spa::sys::spa_meta_region>() as i32,
                ),
            },
        ],
    }
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

fn video_properties(layout: Layout) -> Vec<pw::spa::pod::Property> {
    let mut properties = vec![
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
    ];
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    let modifier = match layout {
        Layout::Memory => None,
        // Left unfixed, so the compositor and this side settle on one
        // layout in a second round (PipeWire's modifier fixation).
        Layout::Offered(modifiers) => Some(pw::spa::pod::Property {
            key,
            flags: pw::spa::pod::PropertyFlags::MANDATORY
                | pw::spa::pod::PropertyFlags::DONT_FIXATE,
            value: pw::spa::pod::Value::Choice(pw::spa::pod::ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: modifiers[0] as i64,
                    alternatives: modifiers.iter().map(|m| *m as i64).collect(),
                },
            ))),
        }),
        Layout::Fixed(modifier) => Some(pw::spa::pod::Property {
            key,
            flags: pw::spa::pod::PropertyFlags::MANDATORY,
            value: pw::spa::pod::Value::Long(modifier as i64),
        }),
    };
    // After the pixel format, which the modifier belongs to.
    properties.splice(3..3, modifier);
    properties
}

fn object_from(properties: Vec<pw::spa::pod::Property>) -> pw::spa::pod::Object {
    pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties,
    }
}

pub(crate) fn serialize_object(value: pw::spa::pod::Object) -> Result<Vec<u8>, Error> {
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
    /// The regions of the screen the compositor redrew for this buffer, when
    /// it says: an empty list names none.
    fn damage(&self) -> Option<Vec<[i32; 4]>> {
        // SAFETY: the dequeued buffer and its metadata remain owned until Drop.
        let buffer = unsafe { self.raw.as_ref().buffer.as_ref()? };
        if buffer.n_metas == 0 || buffer.metas.is_null() {
            return None;
        }
        let metas = unsafe { std::slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
        let meta = metas
            .iter()
            .find(|meta| meta.type_ == pw::spa::sys::SPA_META_VideoDamage)?;
        let region = std::mem::size_of::<pw::spa::sys::spa_meta_region>();
        if meta.data.is_null() || meta.size as usize % region != 0 {
            return None;
        }
        // SAFETY: SPA owns `size` readable bytes, a whole number of regions.
        let regions = unsafe {
            std::slice::from_raw_parts(
                meta.data.cast::<pw::spa::sys::spa_meta_region>(),
                meta.size as usize / region,
            )
        };
        Some(
            regions
                .iter()
                .map(|region| {
                    [
                        region.region.position.x,
                        region.region.position.y,
                        region.region.size.width as i32,
                        region.region.size.height as i32,
                    ]
                })
                .collect(),
        )
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

/// Answers the compositor's format: the buffers this side takes, and for
/// the GPU path the layout it settles on.
fn negotiate(
    stream: &pw::stream::Stream,
    state: &mut ListenerState,
    param: &Pod,
) -> Result<(), Error> {
    let mut info = pw::spa::param::video::VideoInfoRaw::default();
    info.parse(param)
        .map_err(|_| Error::Unsupported("this PipeWire pixel format"))?;
    let flags = info.flags();
    let shared = Arc::clone(&state.shared);
    let gpu_wanted = state.gpu.is_some();
    if gpu_wanted && flags.contains(pw::spa::param::video::VideoFlags::MODIFIER) {
        if flags.contains(pw::spa::param::video::VideoFlags::MODIFIER_FIXATION_REQUIRED) {
            // The compositor lists the layouts it can draw in: take the
            // first this side can import.
            let Some(modifier) = choose_modifier(&offered_modifiers(param)) else {
                shared.gpu_failed.store(true, Ordering::Relaxed);
                return Ok(());
            };
            let bytes = serialize_object(fixated_format(info.format(), modifier))?;
            let pod = Pod::from_bytes(&bytes).ok_or(Error::Unsupported("modifier negotiation"))?;
            return stream
                .update_params(&mut [pod])
                .map_err(|_| Error::Unsupported("modifier negotiation"));
        }
        let gpu = state.gpu.as_mut().expect("checked above");
        gpu.modifier = Some(info.modifier());
        let size = (
            u16::try_from(info.size().width).unwrap_or(0),
            u16::try_from(info.size().height).unwrap_or(0),
        );
        gpu.importer = match Importer::open(&gpu.node, size) {
            Ok(importer) => Some(importer),
            Err(_) => {
                shared.gpu_failed.store(true, Ordering::Relaxed);
                return Ok(());
            }
        };
        let buffers = serialize_object(dma_buf_object())?;
        let cursor = serialize_object(cursor_meta_object())?;
        let damage = serialize_object(damage_meta_object())?;
        return stream
            .update_params(&mut [
                Pod::from_bytes(&buffers).ok_or(Error::Unsupported("buffer negotiation"))?,
                Pod::from_bytes(&cursor)
                    .ok_or(Error::Unsupported("cursor metadata negotiation"))?,
                Pod::from_bytes(&damage)
                    .ok_or(Error::Unsupported("damage metadata negotiation"))?,
            ])
            .map_err(|_| Error::Unsupported("buffer negotiation"));
    }
    // Shared memory: the compositor chose it, or this stream never offered
    // anything else.
    state.gpu = None;
    let buffers = serialize_object(memory_buffer_object())?;
    let cursor = serialize_object(cursor_meta_object())?;
    stream
        .update_params(&mut [
            Pod::from_bytes(&buffers).ok_or(Error::Unsupported("memory buffer negotiation"))?,
            Pod::from_bytes(&cursor).ok_or(Error::Unsupported("cursor metadata negotiation"))?,
        ])
        .map_err(|_| Error::Unsupported("memory buffer negotiation"))
}

/// The layouts a format the compositor sent says it can use.
pub(crate) fn offered_modifiers(param: &Pod) -> Vec<u64> {
    use pw::spa::pod::deserialize::PodDeserializer;
    use pw::spa::pod::{ChoiceValue, Value};
    let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(param.as_bytes())
    else {
        return Vec::new();
    };
    let key = pw::spa::param::format::FormatProperties::VideoModifier.as_raw();
    object
        .properties
        .into_iter()
        .find(|property| property.key == key)
        .map(|property| match property.value {
            Value::Long(modifier) => vec![modifier as u64],
            Value::Choice(ChoiceValue::Long(Choice(
                _,
                ChoiceEnum::Enum {
                    default,
                    alternatives,
                },
            ))) => std::iter::once(default)
                .chain(alternatives)
                .map(|modifier| modifier as u64)
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

fn copy_buffer(stream: &pw::stream::Stream, state: &mut ListenerState) {
    let shared = Arc::clone(&state.shared);
    let shared = shared.as_ref();
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
    let damage = buffer.damage();
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    if datas[0].type_() == pw::spa::buffer::DataType::DmaBuf {
        match state.gpu.as_mut() {
            Some(gpu) => push_dma_buf(shared, gpu, damage.as_deref(), datas),
            None => shared
                .channel
                .fail("PipeWire delivered a GPU buffer instead of shared memory"),
        }
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
    // Mutter hands the whole screen over again at the frame rate while
    // anything on it moves, so an unchanged picture comes too: it is no
    // change, and the host must still see the screen as still.
    shared.channel.push_if_changed(ChannelSlot {
        width,
        height,
        stride,
        bytes: bytes[offset..end].to_vec().into(),
        // When PipeWire handed the picture over: the compositor's own
        // presentation time is not read.
        captured_at: Some(Instant::now()),
        changed: damage
            .as_deref()
            .map(|damage| damage_changes(damage, width, height)),
    });
    record_delivery(shared, false);
}

/// Where a buffer's damage says the screen changed. SPA ends the list at
/// the first empty region.
fn damage_changes(damage: &[[i32; 4]], width: u16, height: u16) -> vnilink_media::Changes {
    vnilink_media::Changes::of(
        damage
            .iter()
            .take_while(|region| region[2] > 0 && region[3] > 0)
            .map(|region| region.map(i64::from)),
        u32::from(width),
        u32::from(height),
    )
}

/// Imports a DMA-BUF the compositor drew and keeps the GPU's copy of it as
/// the latest picture. A buffer the compositor says it left unchanged is no
/// picture, as for shared memory; one the pool has no room for is dropped,
/// the next replacing it.
fn push_dma_buf(
    shared: &Shared,
    gpu: &mut GpuState,
    damage: Option<&[[i32; 4]]>,
    datas: &[pw::spa::buffer::Data],
) {
    if let Some(damage) = damage {
        if !damage_is_empty(damage) {
            gpu.damage_filled = true;
        } else if gpu.damage_filled && shared.channel.pushed_at().is_some() {
            return;
        }
    }
    // This buffer's changes after any dropped since the last picture kept;
    // taken once, by the picture kept or by the next drop.
    let mut dropped = gpu.dropped.replace(vnilink_media::Changes::none());
    let (Some(importer), Some(modifier), Some((width, height)), Some(format)) = (
        gpu.importer.as_mut(),
        gpu.modifier,
        *shared.size.lock().expect("portal size"),
        *shared.format.lock().expect("portal format"),
    ) else {
        shared.gpu_failed.store(true, Ordering::Relaxed);
        return;
    };
    let fourcc = match format {
        VideoFormat::BGRA => DRM_FORMAT_ARGB8888,
        _ => DRM_FORMAT_XRGB8888,
    };
    // SAFETY: the descriptor is the PipeWire buffer's, open while it is dequeued.
    let key = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(datas[0].fd()) })
        .metadata()
        .map(|metadata| metadata.ino())
        .unwrap_or_default();
    let planes: Vec<_> = datas
        .iter()
        .map(|data| {
            (
                data.fd(),
                data.as_raw().maxsize,
                data.chunk().offset(),
                data.chunk().stride().max(0) as u32,
            )
        })
        .collect();
    let mut changed = || {
        dropped
            .take()
            .zip(damage.map(|damage| damage_changes(damage, width, height)))
            .map(|(before, after)| before.join(after))
    };
    let copied = importer.copy(key, || {
        Prime::from_planes(
            &planes,
            u32::from(width),
            u32::from(height),
            fourcc,
            modifier,
        )
    });
    match copied {
        Ok(Some(picture)) => {
            shared.channel.push(ChannelSlot {
                width,
                height,
                stride: usize::from(width) * 4,
                bytes: vnilink_media::Pixels::native(Box::new(picture)),
                captured_at: Some(Instant::now()),
                changed: changed(),
            });
            record_delivery(shared, true);
        }
        Ok(None) => {
            tracing::debug!(
                target: vnilink::trace::CAPTURE,
                "picture dropped: no GPU copy free; its changes go with the next"
            );
            gpu.dropped = changed();
        }
        Err(_) => shared.gpu_failed.store(true, Ordering::Relaxed),
    }
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
                gpu_failed: AtomicBool::new(false),
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
        let delivered = Instant::now();
        shared.channel.push(ChannelSlot {
            width: 8,
            height: 8,
            stride: 32,
            bytes: vec![255; 256].into(),
            captured_at: Some(delivered),
            changed: None,
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
            // A cached picture keeps the time PipeWire delivered it.
            assert_eq!(frame.captured_at(), Some(delivered));
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
