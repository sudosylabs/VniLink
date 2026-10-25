//! VA-API encode: H.264 (Constrained Baseline or High) and H.265 Main,
//! 8-bit 4:2:0, on one DRM render node the caller chose ([`crate::gpu`]).
//!
//! Low-power slice encode is preferred to slice encode. A node without the
//! codec refuses to open.
//! libva's context is not [`Send`]: this module's thread is its only caller
//! and the encoder handle on the other side is a channel. Pictures arrive as
//! BGRA and the GPU converts them to NV12 (BT.709, limited range) where the
//! node offers video processing; elsewhere they arrive as NV12 converted on
//! the CPU.
//! Each codec's module fills the driver's parameters and writes the
//! parameter sets a driver may leave out of a keyframe. No software encoder
//! is linked.
//!
//! Keyframes come only when asked for: the first picture, and those the
//! host asks for after a loss or a refresh. Rate control holds a picture to
//! one picture's share of the bitrate ([`RateControl`]), and takes a new
//! bitrate or frame rate between pictures without reopening
//! ([`Session::retune`]).

mod convert;
mod h264;
mod hevc;

use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use libva::BufferType;
use libva::Display;
use libva::EncMiscParameter;
use libva::EncMiscParameterFrameRate;
use libva::EncMiscParameterHRD;
use libva::EncMiscParameterRateControl;
use libva::Image;
use libva::MappedCodedBuffer;
use libva::Picture;
use libva::RcFlags;
use libva::Surface;
use libva::UsageHint;
use libva::VAConfigAttrib;
use libva::VAConfigAttribType;
use libva::VAEntrypoint;
use libva::VAProfile;
use libva::VA_FOURCC_NV12;
use libva::VA_RT_FORMAT_YUV420;
use vnilink_media::{EncodedSample, Error, RawFrame};

use self::convert::Converter;
use crate::hevc_parameter_set::Layout;

/// A codec this module encodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VaCodec {
    H264 { high: bool },
    H265,
}

impl VaCodec {
    /// Named in errors, so a failure says which encoder was missing.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::H264 { .. } => "VA-API H.264",
            Self::H265 => "VA-API H.265",
        }
    }

    /// Headers this encoder writes itself when the driver takes them:
    /// none for H.264, whose driver output is tested; for H.265 the
    /// parameter sets and slice headers, so they agree with each other.
    fn packed_headers(self) -> u32 {
        match self {
            Self::H264 { .. } => 0,
            Self::H265 => libva::VA_ENC_PACKED_HEADER_SEQUENCE | libva::VA_ENC_PACKED_HEADER_SLICE,
        }
    }

    fn profile(self) -> VAProfile::Type {
        match self {
            Self::H264 { high: false } => VAProfile::VAProfileH264ConstrainedBaseline,
            Self::H264 { high: true } => VAProfile::VAProfileH264High,
            Self::H265 => VAProfile::VAProfileHEVCMain,
        }
    }
}

/// The I and IDR period the driver's rate control plans for. This encoder
/// decides each picture's type itself and makes a keyframe only when asked,
/// so the driver is told the longest period it is likely to take rather
/// than none: 0 is not read the same way by every driver.
pub(super) const INTRA_PERIOD: u32 = u16::MAX as u32;

/// What one picture's parameters need from the encoder core.
pub(super) struct PictureJob {
    pub(super) surface_id: u32,
    /// The surface of the picture before, which this one predicts from.
    pub(super) reference: Option<u32>,
    pub(super) key: bool,
    pub(super) coded_buf: u32,
    pub(super) visible_width: u32,
    pub(super) visible_height: u32,
    pub(super) coded_width: u32,
    pub(super) coded_height: u32,
    pub(super) frame_rate: u32,
    pub(super) bitrate_bps: u32,
}

enum Syntax {
    H264(h264::Syntax),
    H265(hevc::Syntax),
}

impl Syntax {
    fn buffers(&self, job: &PictureJob) -> Vec<BufferType> {
        match self {
            Self::H264(syntax) => syntax.buffers(job),
            Self::H265(syntax) => syntax.buffers(job),
        }
    }

    fn encoded(&mut self, key: bool) {
        match self {
            Self::H264(syntax) => syntax.encoded(key),
            Self::H265(syntax) => syntax.encoded(key),
        }
    }

    fn sample(&self, bitstream: &[u8]) -> Result<EncodedSample, Error> {
        match self {
            Self::H264(syntax) => syntax.sample(bitstream),
            Self::H265(syntax) => syntax.sample(bitstream),
        }
    }

    /// Initial, smallest and largest quantizer for rate control.
    fn quantizers(&self) -> (u32, u32, u32) {
        match self {
            Self::H264(_) => (u32::from(h264::QP), 0, 51),
            // Zero leaves each to the driver, as FFmpeg does.
            Self::H265(_) => (0, 0, 0),
        }
    }
}

/// A picture for the encoder: NV12 planes, or BGRA the GPU converts.
pub(crate) enum Input {
    Nv12 { y: Vec<u8>, uv: Vec<u8> },
    Bgra(RawFrame),
}

enum Job {
    Encode {
        input: Input,
        force_keyframe: bool,
        reply: Sender<Result<EncodedSample, Error>>,
    },
    Retune {
        bitrate_bps: u32,
        frame_rate: u8,
        reply: Sender<()>,
    },
    Stop,
}

pub(crate) struct Session {
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
    converts: bool,
}

impl Session {
    /// A session on the render node at `node`.
    pub(crate) fn open(
        node: &Path,
        codec: VaCodec,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let node = node.to_path_buf();
        let thread = thread::Builder::new()
            .name("vnilink-vaapi".to_string())
            .spawn(move || {
                worker(
                    &node,
                    codec,
                    width,
                    height,
                    frame_rate,
                    bitrate_bps,
                    rx,
                    ready_tx,
                )
            })
            .map_err(|_| Error::Unsupported(codec.name()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(converts)) => Ok(Self {
                tx,
                thread: Some(thread),
                converts,
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Unsupported(codec.name()))
            }
        }
    }

    /// Whether this session takes BGRA ([`Input::Bgra`]) and converts it on
    /// the GPU; otherwise it takes NV12.
    pub(crate) fn converts(&self) -> bool {
        self.converts
    }

    pub(crate) fn encode(
        &self,
        input: Input,
        force_keyframe: bool,
    ) -> Result<EncodedSample, Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Job::Encode {
                input,
                force_keyframe,
                reply: reply_tx,
            })
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        reply_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?
    }

    /// Takes `bitrate_bps` and `frame_rate` from the next picture on, with
    /// no keyframe: rate control's parameters go with every picture.
    pub(crate) fn retune(&self, bitrate_bps: u32, frame_rate: u8) -> Result<(), Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Job::Retune {
                bitrate_bps,
                frame_rate,
                reply: reply_tx,
            })
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        reply_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    node: &Path,
    codec: VaCodec,
    width: u16,
    height: u16,
    frame_rate: u8,
    bitrate_bps: u32,
    jobs: Receiver<Job>,
    ready: Sender<Result<bool, Error>>,
) {
    let mut encoder = match Encoder::open(node, codec, width, height, frame_rate, bitrate_bps) {
        Ok(encoder) => {
            let _ = ready.send(Ok(encoder.converter.is_some()));
            encoder
        }
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Stop => break,
            Job::Encode {
                input,
                force_keyframe,
                reply,
            } => {
                let result = encoder.encode(input, force_keyframe);
                let _ = reply.send(result);
            }
            Job::Retune {
                bitrate_bps,
                frame_rate,
                reply,
            } => {
                encoder.rate.retune(bitrate_bps, frame_rate);
                let _ = reply.send(());
            }
        }
    }
}

struct Encoder {
    context: Rc<libva::Context>,
    surfaces: [Option<Surface<()>>; 2],
    format: libva::VAImageFormat,
    converter: Option<Converter>,
    syntax: Syntax,
    visible_width: u32,
    visible_height: u32,
    coded_width: u32,
    coded_height: u32,
    rate: RateControl,
    cursor: usize,
    /// The surface holding the picture before, once there is one.
    previous: Option<usize>,
    poisoned: bool,
}

/// The bitrate and frame rate the next picture is encoded at, and whether
/// rate control starts over at them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RateControl {
    pub(crate) bitrate_bps: u32,
    pub(crate) frame_rate: u32,
    /// Set by a change and cleared by the picture that carries it.
    pub(crate) reset: bool,
}

impl RateControl {
    pub(crate) fn new(bitrate_bps: u32, frame_rate: u8) -> Self {
        Self {
            bitrate_bps,
            frame_rate: u32::from(frame_rate.max(1)),
            reset: false,
        }
    }

    pub(crate) fn retune(&mut self, bitrate_bps: u32, frame_rate: u8) {
        let next = Self::new(bitrate_bps, frame_rate);
        if (next.bitrate_bps, next.frame_rate) != (self.bitrate_bps, self.frame_rate) {
            *self = Self {
                reset: true,
                ..next
            };
        }
    }

    /// One picture's share of the bitrate, in bits: the HRD buffer, as
    /// NVENC's VBV and Media Foundation's buffer are sized, so that no
    /// picture, a keyframe included, takes the link longer than a picture's
    /// interval to send.
    pub(crate) fn buffer_bits(&self) -> u32 {
        (self.bitrate_bps / self.frame_rate).max(1)
    }

    /// The window rate control holds the bitrate over, in milliseconds:
    /// the time the buffer takes to drain at the bitrate, one picture's
    /// interval.
    pub(crate) fn window_ms(&self) -> u32 {
        (1000 / self.frame_rate).max(1)
    }
}

impl Encoder {
    fn open(
        node: &Path,
        codec: VaCodec,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::InvalidFrame("width and height must be even"));
        }
        try_open(node, codec, width, height, frame_rate, bitrate_bps)
            .ok_or(Error::Unsupported(codec.name()))
    }

    fn encode(&mut self, input: Input, force_keyframe: bool) -> Result<EncodedSample, Error> {
        if self.poisoned {
            return Err(Error::EncodeRejected("VA-API rejected the frame"));
        }
        // No periodic keyframe: the viewer asks for one after a loss, and
        // until then every picture predicts from the one before.
        let key = force_keyframe || self.previous.is_none();
        let index = self.cursor;
        let surface = self.surfaces[index]
            .take()
            .ok_or(Error::EncodeRejected("VA-API rejected the frame"))?;
        let prepared = match input {
            Input::Nv12 { y, uv } => upload_nv12(
                &surface,
                self.format,
                self.visible_width,
                self.visible_height,
                self.coded_width,
                self.coded_height,
                &y,
                &uv,
            )
            .map(|()| surface),
            Input::Bgra(frame) => match &self.converter {
                Some(converter) => converter.convert(&frame, surface),
                None => Err(Error::EncodeRejected("VA-API converts no BGRA here")),
            },
        };
        let surface = match prepared {
            Ok(surface) => surface,
            Err(err) => {
                // The surface went with the failure: the pool is short now.
                self.poisoned = true;
                return Err(err);
            }
        };
        let reference = self
            .previous
            .and_then(|previous| self.surfaces[previous].as_ref())
            .map(|stored| stored.id());
        let outcome = self.encode_surface(surface, reference, key);
        let (surface, sample) = match outcome {
            Ok(pair) => pair,
            Err(err) => {
                self.poisoned = true;
                return Err(err);
            }
        };
        self.surfaces[index] = Some(surface);
        self.previous = Some(index);
        self.syntax.encoded(key);
        self.rate.reset = false;
        self.cursor ^= 1;
        Ok(sample)
    }

    fn encode_surface(
        &self,
        surface: Surface<()>,
        reference: Option<u32>,
        key: bool,
    ) -> Result<(Surface<()>, EncodedSample), Error> {
        let rejected = |_| Error::EncodeRejected("VA-API rejected the frame");
        let context = &self.context;
        let coded = context
            .create_enc_coded((self.coded_width * self.coded_height) as usize)
            .map_err(rejected)?;
        let job = PictureJob {
            surface_id: surface.id(),
            reference,
            key,
            coded_buf: coded.id(),
            visible_width: self.visible_width,
            visible_height: self.visible_height,
            coded_width: self.coded_width,
            coded_height: self.coded_height,
            frame_rate: self.rate.frame_rate,
            bitrate_bps: self.rate.bitrate_bps,
        };
        let (initial_qp, min_qp, max_qp) = self.syntax.quantizers();
        let buffer_bits = self.rate.buffer_bits();
        let mut buffers = Vec::new();
        for buffer in self.syntax.buffers(&job).into_iter().chain([
            BufferType::EncMiscParameter(EncMiscParameter::FrameRate(
                EncMiscParameterFrameRate::new(self.rate.frame_rate, 0),
            )),
            // The buffer starts full, as NVENC's initial delay is set.
            BufferType::EncMiscParameter(EncMiscParameter::HRD(EncMiscParameterHRD::new(
                buffer_bits,
                buffer_bits,
            ))),
            // Pictures are never skipped, as NVENC and Media Foundation do
            // not skip them: a skipped picture would leave the newest
            // screen unsent, and the one after it predicting from a
            // picture the driver did not make. The quantiser rises instead.
            BufferType::EncMiscParameter(EncMiscParameter::RateControl(
                EncMiscParameterRateControl::new(
                    self.rate.bitrate_bps,
                    100,
                    self.rate.window_ms(),
                    initial_qp,
                    min_qp,
                    0,
                    RcFlags::new(u32::from(self.rate.reset), 1, 0, 0, 0, 0, 0, 0, 0),
                    0,
                    max_qp,
                    0,
                    0,
                ),
            )),
        ]) {
            buffers.push(context.create_buffer(buffer).map_err(rejected)?);
        }

        let mut picture = Picture::new(0, Rc::clone(context), surface);
        for buffer in buffers {
            picture.add_buffer(buffer);
        }
        let picture = picture.begin().map_err(rejected)?;
        let picture = picture.render().map_err(rejected)?;
        let picture = picture.end().map_err(rejected)?;
        let picture = match picture.sync() {
            Ok(picture) => picture,
            // PictureEnd cannot give the surface back. Dropping it poisons the pool.
            Err((_, _picture)) => return Err(Error::EncodeRejected("VA-API rejected the frame")),
        };
        let bytes = {
            let mapped = MappedCodedBuffer::new(&coded).map_err(rejected)?;
            let mut bytes = Vec::new();
            for segment in mapped.segments() {
                if segment.bit_offset % 8 != 0 {
                    return Err(Error::EncodeRejected("VA-API rejected the frame"));
                }
                push_bitstream(&mut bytes, segment.buf);
            }
            bytes
        };
        let surface = picture
            .take_surface()
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        let sample = self.syntax.sample(&bytes)?;
        Ok((surface, sample))
    }
}

fn try_open(
    path: &Path,
    codec: VaCodec,
    width: u16,
    height: u16,
    frame_rate: u8,
    bitrate_bps: u32,
) -> Option<Encoder> {
    let display = Display::open_drm_display(path).ok()?;
    let profile = codec.profile();
    let entrypoints = display.query_config_entrypoints(profile).ok()?;
    let entrypoint = choose_entrypoint(&entrypoints)?;
    let (config, packed) = encode_config(&display, profile, entrypoint, codec.packed_headers())?;
    let syntax = match codec {
        VaCodec::H264 { high } => {
            Syntax::H264(h264::Syntax::new(high, width, height, frame_rate).ok()?)
        }
        VaCodec::H265 => Syntax::H265(
            hevc::Syntax::new(
                hevc_layout(&display, profile, entrypoint),
                packed != 0,
                width,
                height,
            )
            .ok()?,
        ),
    };
    let coded_width = u32::from(crate::parameter_set::round_up_16(width));
    let coded_height = u32::from(crate::parameter_set::round_up_16(height));
    let surfaces = display
        .create_surfaces(
            VA_RT_FORMAT_YUV420,
            Some(VA_FOURCC_NV12),
            coded_width,
            coded_height,
            // The video processor writes them when it converts the capture.
            Some(UsageHint::USAGE_HINT_ENCODER | UsageHint::USAGE_HINT_VPP_WRITE),
            vec![(), ()],
        )
        .ok()?;
    let context = display
        .create_context(&config, coded_width, coded_height, Some(&surfaces), true)
        .ok()?;
    let format = display
        .query_image_formats()
        .ok()?
        .into_iter()
        .find(|format| format.fourcc == VA_FOURCC_NV12)?;
    let converter = Converter::open(
        &display,
        path,
        u32::from(width),
        u32::from(height),
        (coded_width, coded_height),
        &surfaces,
    );
    let mut stored = [None, None];
    for (slot, surface) in stored.iter_mut().zip(surfaces) {
        *slot = Some(surface);
    }
    Some(Encoder {
        context,
        surfaces: stored,
        format,
        converter,
        syntax,
        visible_width: u32::from(width),
        visible_height: u32::from(height),
        coded_width,
        coded_height,
        rate: RateControl::new(bitrate_bps, frame_rate),
        cursor: 0,
        previous: None,
        poisoned: false,
    })
}

/// The H.265 block sizes and tools this driver takes, or the common ones
/// when it does not say.
fn hevc_layout(
    display: &std::sync::Arc<Display>,
    profile: VAProfile::Type,
    entrypoint: VAEntrypoint::Type,
) -> Layout {
    let mut attrs = vec![
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribEncHEVCFeatures,
            value: 0,
        },
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribEncHEVCBlockSizes,
            value: 0,
        },
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribPredictionDirection,
            value: 0,
        },
    ];
    if display
        .get_config_attributes(profile, entrypoint, &mut attrs)
        .is_err()
    {
        return Layout::from_attributes(None, None);
    }
    let known = |value: u32| (value != libva::VA_ATTRIB_NOT_SUPPORTED).then_some(value);
    let mut layout = Layout::from_attributes(known(attrs[0].value), known(attrs[1].value));
    layout.generalized_b = known(attrs[2].value)
        .is_some_and(|value| value & libva::VA_PREDICTION_DIRECTION_BI_NOT_EMPTY != 0);
    layout
}

fn bitrate_config(
    display: &std::sync::Arc<Display>,
    profile: libva::VAProfile::Type,
    entrypoint: libva::VAEntrypoint::Type,
) -> Option<libva::Config> {
    encode_config(display, profile, entrypoint, 0).map(|(config, _)| config)
}

/// A 4:2:0 encode configuration with bitrate control, and of the packed
/// headers `wanted`, those the driver takes.
fn encode_config(
    display: &std::sync::Arc<Display>,
    profile: libva::VAProfile::Type,
    entrypoint: libva::VAEntrypoint::Type,
    wanted: u32,
) -> Option<(libva::Config, u32)> {
    let mut attrs = vec![
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribRTFormat,
            value: 0,
        },
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribRateControl,
            value: 0,
        },
        VAConfigAttrib {
            type_: VAConfigAttribType::VAConfigAttribEncPackedHeaders,
            value: 0,
        },
    ];
    display
        .get_config_attributes(profile, entrypoint, &mut attrs)
        .ok()?;
    if attrs[0].value & VA_RT_FORMAT_YUV420 == 0 {
        return None;
    }
    let mode = choose_bitrate_control(attrs[1].value)?;
    // All or none: a parameter set the driver also writes would disagree.
    let packed =
        if attrs[2].value != libva::VA_ATTRIB_NOT_SUPPORTED && attrs[2].value & wanted == wanted {
            wanted
        } else {
            0
        };
    attrs[0].value = VA_RT_FORMAT_YUV420;
    attrs[1].value = mode;
    attrs[2].value = packed;
    if packed == 0 {
        attrs.pop();
    }
    let config = display.create_config(attrs, profile, entrypoint).ok()?;
    Some((config, packed))
}

pub(crate) fn choose_bitrate_control(supported: u32) -> Option<u32> {
    if supported == libva::VA_ATTRIB_NOT_SUPPORTED {
        return None;
    }
    // Leaving this attribute unset selects CQP on Intel: bitrate settings
    // then have no effect and every picture uses the fixed initial quantizer.
    [libva::VA_RC_VBR, libva::VA_RC_CBR]
        .into_iter()
        .find(|mode| supported & mode != 0)
}

/// The render nodes under `/dev/dri`, in name order.
pub(crate) fn render_nodes() -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if is_render_node_name(name) {
            paths.push(entry.path());
        }
    }
    paths.sort();
    paths
}

/// The first render node, in [`crate::gpu::ranked`] order, that offers
/// slice encode of `codec` with bitrate control. No encoder session is
/// opened.
#[cfg(test)]
pub(crate) fn offering(codec: VaCodec) -> Option<std::path::PathBuf> {
    crate::gpu::ranked_render_nodes()
        .into_iter()
        .find(|node| offers_on(node, codec))
}

/// Whether the render node at `node` offers slice encode of `codec` with
/// bitrate control.
pub(crate) fn offers_on(node: &Path, codec: VaCodec) -> bool {
    let profile = codec.profile();
    let Ok(display) = Display::open_drm_display(node) else {
        return false;
    };
    let Ok(entrypoints) = display.query_config_entrypoints(profile) else {
        return false;
    };
    choose_entrypoint(&entrypoints)
        .is_some_and(|entrypoint| bitrate_config(&display, profile, entrypoint).is_some())
}

/// True when a DRM render node offers VA-API H.264 VLD decode. No decode
/// session is opened. A machine without this entrypoint reports unsupported
/// instead of silently showing no video.
pub(crate) fn offers_h264_decode() -> bool {
    const PROFILES: [libva::VAProfile::Type; 3] = [
        libva::VAProfile::VAProfileH264ConstrainedBaseline,
        libva::VAProfile::VAProfileH264Main,
        libva::VAProfile::VAProfileH264High,
    ];
    render_nodes().iter().any(|path| {
        let Ok(display) = Display::open_drm_display(path) else {
            return false;
        };
        PROFILES.into_iter().any(|profile| {
            display
                .query_config_entrypoints(profile)
                .is_ok_and(|entrypoints| entrypoints.contains(&VAEntrypoint::VAEntrypointVLD))
        })
    })
}

pub(crate) fn is_render_node_name(name: &str) -> bool {
    let Some(index) = name.strip_prefix("renderD") else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn choose_entrypoint(entrypoints: &[VAEntrypoint::Type]) -> Option<VAEntrypoint::Type> {
    if entrypoints
        .iter()
        .any(|entry| *entry == VAEntrypoint::VAEntrypointEncSliceLP)
    {
        Some(VAEntrypoint::VAEntrypointEncSliceLP)
    } else if entrypoints
        .iter()
        .any(|entry| *entry == VAEntrypoint::VAEntrypointEncSlice)
    {
        Some(VAEntrypoint::VAEntrypointEncSlice)
    } else {
        None
    }
}

fn upload_nv12(
    surface: &Surface<()>,
    format: libva::VAImageFormat,
    visible_width: u32,
    visible_height: u32,
    coded_width: u32,
    coded_height: u32,
    y: &[u8],
    uv: &[u8],
) -> Result<(), Error> {
    let mut image = Image::create_from(
        surface,
        format,
        (coded_width, coded_height),
        (visible_width, visible_height),
    )
    .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let descriptor = *image.image();
    if descriptor.num_planes < 2 {
        return Err(Error::EncodeRejected("VA-API rejected the frame"));
    }
    let dest = image.as_mut();
    copy_plane(
        dest,
        descriptor.offsets[0] as usize,
        descriptor.pitches[0] as usize,
        y,
        visible_width as usize,
        visible_height as usize,
        coded_width as usize,
        coded_height as usize,
        16,
    )?;
    copy_plane(
        dest,
        descriptor.offsets[1] as usize,
        descriptor.pitches[1] as usize,
        uv,
        visible_width as usize,
        visible_height as usize / 2,
        coded_width as usize,
        coded_height as usize / 2,
        128,
    )?;
    Ok(())
}

fn copy_plane(
    dest: &mut [u8],
    offset: usize,
    pitch: usize,
    src: &[u8],
    visible_width: usize,
    visible_height: usize,
    coded_width: usize,
    coded_height: usize,
    pad: u8,
) -> Result<(), Error> {
    if pitch < coded_width {
        return Err(Error::EncodeRejected("VA-API rejected the frame"));
    }
    for row in 0..coded_height {
        let start = offset + row * pitch;
        let end = start + coded_width;
        if end > dest.len() {
            return Err(Error::EncodeRejected("VA-API rejected the frame"));
        }
        if row < visible_height {
            let src_row = &src[row * visible_width..(row + 1) * visible_width];
            dest[start..start + visible_width].copy_from_slice(src_row);
            dest[start + visible_width..end].fill(pad);
        } else {
            dest[start..end].fill(pad);
        }
    }
    Ok(())
}

fn push_bitstream(out: &mut Vec<u8>, chunk: &[u8]) {
    if chunk.is_empty() {
        return;
    }
    if chunk.starts_with(&[0, 0, 1]) || chunk.starts_with(&[0, 0, 0, 1]) {
        out.extend_from_slice(chunk);
    } else {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(chunk);
    }
}
