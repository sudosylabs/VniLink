//! VA-API Constrained Baseline encode.
//!
//! Render nodes under `/dev/dri` are opened in name order. The first node
//! that offers Constrained Baseline is kept: low-power slice encode, then
//! slice encode. A node that opens without that entrypoint is skipped.
//! libva's context is not [`Send`]. This thread is the only caller. The
//! encoder handle on the other side is a channel. Pictures are NV12. A
//! keyframe is Annex-B with SPS, PPS, and an IDR. No software encoder is
//! linked.

use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use libva::BufferType;
use libva::Display;
use libva::EncMiscParameter;
use libva::EncMiscParameterFrameRate;
use libva::EncMiscParameterRateControl;
use libva::EncPictureParameter;
use libva::EncSequenceParameter;
use libva::EncSliceParameter;
use libva::Image;
use libva::MappedCodedBuffer;
use libva::Picture;
use libva::PictureH264;
use libva::RcFlags;
use libva::Surface;
use libva::UsageHint;
use libva::VAConfigAttrib;
use libva::VAConfigAttribType;
use libva::VAEntrypoint;
use libva::VAProfile;
use libva::VA_FOURCC_NV12;
use libva::VA_INVALID_ID;
use libva::VA_INVALID_SURFACE;
use libva::VA_PICTURE_H264_SHORT_TERM_REFERENCE;
use libva::VA_RT_FORMAT_YUV420;
use vnilink_media::{parse_annex_b, EncodedSample, Error};

use crate::parameter_set::{
    level_idc, parameter_sets, LOG2_MAX_FRAME_NUM_MINUS4, LOG2_MAX_POC_LSB_MINUS4, MAX_FRAME_NUM,
    MAX_POC_LSB,
};

const QP: u8 = 26;

enum Job {
    Encode {
        y: Vec<u8>,
        uv: Vec<u8>,
        force_keyframe: bool,
        reply: Sender<Result<EncodedSample, Error>>,
    },
    Stop,
}

pub(crate) struct Session {
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    pub(crate) fn open(
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
    ) -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("vnilink-vaapi".to_string())
            .spawn(move || worker(width, height, frame_rate, bitrate_bps, rx, ready_tx))
            .map_err(|_| Error::Unsupported("VA-API H.264"))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                tx,
                thread: Some(thread),
            }),
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Unsupported("VA-API H.264"))
            }
        }
    }

    pub(crate) fn encode(
        &self,
        y: Vec<u8>,
        uv: Vec<u8>,
        force_keyframe: bool,
    ) -> Result<EncodedSample, Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(Job::Encode {
                y,
                uv,
                force_keyframe,
                reply: reply_tx,
            })
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        reply_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?
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

fn worker(
    width: u16,
    height: u16,
    frame_rate: u8,
    bitrate_bps: u32,
    jobs: Receiver<Job>,
    ready: Sender<Result<(), Error>>,
) {
    let mut encoder = match Encoder::open(width, height, frame_rate, bitrate_bps) {
        Ok(encoder) => {
            let _ = ready.send(Ok(()));
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
                y,
                uv,
                force_keyframe,
                reply,
            } => {
                let result = encoder.encode(&y, &uv, force_keyframe);
                let _ = reply.send(result);
            }
        }
    }
}

struct Encoder {
    context: Rc<libva::Context>,
    surfaces: [Option<Surface<()>>; 2],
    format: libva::VAImageFormat,
    header: Vec<u8>,
    visible_width: u32,
    visible_height: u32,
    coded_width: u32,
    coded_height: u32,
    frame_rate: u32,
    bitrate_bps: u32,
    cursor: usize,
    frame_num: u32,
    poc: i32,
    idr_id: u16,
    previous: Option<Reference>,
    poisoned: bool,
}

struct Reference {
    index: usize,
    frame_num: u32,
    poc: i32,
}

impl Encoder {
    fn open(width: u16, height: u16, frame_rate: u8, bitrate_bps: u32) -> Result<Self, Error> {
        let header = parameter_sets(width, height, frame_rate)?;
        for path in render_nodes() {
            if let Some(encoder) = try_open(&path, width, height, frame_rate, bitrate_bps, &header)
            {
                return Ok(encoder);
            }
        }
        Err(Error::Unsupported("VA-API H.264"))
    }

    fn encode(
        &mut self,
        y: &[u8],
        uv: &[u8],
        force_keyframe: bool,
    ) -> Result<EncodedSample, Error> {
        if self.poisoned {
            return Err(Error::EncodeRejected("VA-API rejected the frame"));
        }
        let idr = force_keyframe || self.previous.is_none();
        let index = self.cursor;
        let surface = self.surfaces[index]
            .as_ref()
            .ok_or(Error::EncodeRejected("VA-API rejected the frame"))?;
        if let Err(err) = upload_nv12(
            surface,
            self.format,
            self.visible_width,
            self.visible_height,
            self.coded_width,
            self.coded_height,
            y,
            uv,
        ) {
            self.poisoned = true;
            return Err(err);
        }
        let surface = self.surfaces[index]
            .take()
            .ok_or(Error::EncodeRejected("VA-API rejected the frame"))?;
        let surface_id = surface.id();
        let reference = self.previous.as_ref().and_then(|previous| {
            self.surfaces[previous.index]
                .as_ref()
                .map(|stored| (stored.id(), previous.frame_num, previous.poc))
        });
        let frame_num = if idr { 0 } else { self.frame_num };
        let poc = if idr { 0 } else { self.poc };
        let outcome = encode_surface(
            &self.context,
            surface,
            &EncodeJob {
                surface_id,
                reference,
                frame_num,
                poc,
                idr,
                idr_id: self.idr_id,
                visible_width: self.visible_width,
                visible_height: self.visible_height,
                coded_width: self.coded_width,
                coded_height: self.coded_height,
                frame_rate: self.frame_rate,
                bitrate_bps: self.bitrate_bps,
                header: &self.header,
            },
        );
        let (surface, sample) = match outcome {
            Ok(pair) => pair,
            Err(err) => {
                self.poisoned = true;
                return Err(err);
            }
        };
        self.surfaces[index] = Some(surface);
        self.previous = Some(Reference {
            index,
            frame_num,
            poc,
        });
        if idr {
            self.frame_num = 1 % MAX_FRAME_NUM;
            self.poc = 2 % MAX_POC_LSB;
            self.idr_id = self.idr_id.wrapping_add(1);
        } else {
            self.frame_num = (frame_num + 1) % MAX_FRAME_NUM;
            self.poc = (poc + 2) % MAX_POC_LSB;
        }
        self.cursor ^= 1;
        Ok(sample)
    }
}

struct EncodeJob<'a> {
    surface_id: u32,
    reference: Option<(u32, u32, i32)>,
    frame_num: u32,
    poc: i32,
    idr: bool,
    idr_id: u16,
    visible_width: u32,
    visible_height: u32,
    coded_width: u32,
    coded_height: u32,
    frame_rate: u32,
    bitrate_bps: u32,
    header: &'a [u8],
}

fn encode_surface(
    context: &Rc<libva::Context>,
    surface: Surface<()>,
    job: &EncodeJob<'_>,
) -> Result<(Surface<()>, EncodedSample), Error> {
    let coded = context
        .create_enc_coded((job.coded_width * job.coded_height) as usize)
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let seq = context
        .create_buffer(BufferType::EncSequenceParameter(
            EncSequenceParameter::H264(sequence_buffer(job)),
        ))
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let pic = context
        .create_buffer(BufferType::EncPictureParameter(EncPictureParameter::H264(
            picture_buffer(job, coded.id()),
        )))
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let slice = context
        .create_buffer(BufferType::EncSliceParameter(EncSliceParameter::H264(
            slice_buffer(job),
        )))
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let rate = context
        .create_buffer(BufferType::EncMiscParameter(EncMiscParameter::FrameRate(
            EncMiscParameterFrameRate::new(job.frame_rate, 0),
        )))
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let control = context
        .create_buffer(BufferType::EncMiscParameter(EncMiscParameter::RateControl(
            EncMiscParameterRateControl::new(
                job.bitrate_bps,
                100,
                1_000,
                u32::from(QP),
                10,
                0,
                RcFlags::new(0, 1, 0, 0, 0, 0, 0, 0, 0),
                0,
                51,
                0,
                0,
            ),
        )))
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;

    let mut picture = Picture::new(0, Rc::clone(context), surface);
    picture.add_buffer(seq);
    picture.add_buffer(pic);
    picture.add_buffer(slice);
    picture.add_buffer(rate);
    picture.add_buffer(control);
    let picture = picture
        .begin()
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let picture = picture
        .render()
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let picture = picture
        .end()
        .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let picture = match picture.sync() {
        Ok(picture) => picture,
        // PictureEnd cannot give the surface back. Dropping it poisons the pool.
        Err((_, _picture)) => return Err(Error::EncodeRejected("VA-API rejected the frame")),
    };
    let bytes = {
        let mapped = MappedCodedBuffer::new(&coded)
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
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
    let sample = access_unit(&bytes, job.header)?;
    Ok((surface, sample))
}

fn try_open(
    path: &Path,
    width: u16,
    height: u16,
    frame_rate: u8,
    bitrate_bps: u32,
    header: &[u8],
) -> Option<Encoder> {
    let display = Display::open_drm_display(path).ok()?;
    let profile = VAProfile::VAProfileH264ConstrainedBaseline;
    let entrypoints = display.query_config_entrypoints(profile).ok()?;
    let entrypoint = choose_h264_entrypoint(&entrypoints)?;
    let mut attrs = vec![VAConfigAttrib {
        type_: VAConfigAttribType::VAConfigAttribRTFormat,
        value: 0,
    }];
    display
        .get_config_attributes(profile, entrypoint, &mut attrs)
        .ok()?;
    if attrs[0].value & VA_RT_FORMAT_YUV420 == 0 {
        return None;
    }
    attrs[0].value = VA_RT_FORMAT_YUV420;
    let config = display.create_config(attrs, profile, entrypoint).ok()?;
    let coded_width = u32::from(crate::parameter_set::round_up_16(width));
    let coded_height = u32::from(crate::parameter_set::round_up_16(height));
    let surfaces = display
        .create_surfaces(
            VA_RT_FORMAT_YUV420,
            Some(VA_FOURCC_NV12),
            coded_width,
            coded_height,
            Some(UsageHint::USAGE_HINT_ENCODER),
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
    let mut stored = [None, None];
    for (slot, surface) in stored.iter_mut().zip(surfaces) {
        *slot = Some(surface);
    }
    Some(Encoder {
        context,
        surfaces: stored,
        format,
        header: header.to_vec(),
        visible_width: u32::from(width),
        visible_height: u32::from(height),
        coded_width,
        coded_height,
        frame_rate: u32::from(frame_rate),
        bitrate_bps,
        cursor: 0,
        frame_num: 0,
        poc: 0,
        idr_id: 0,
        previous: None,
        poisoned: false,
    })
}

fn invalid_pictures<const N: usize>() -> [PictureH264; N] {
    std::array::from_fn(|_| PictureH264::new(VA_INVALID_ID, 0, VA_INVALID_SURFACE, 0, 0))
}

fn render_nodes() -> Vec<std::path::PathBuf> {
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

pub(crate) fn is_render_node_name(name: &str) -> bool {
    let Some(index) = name.strip_prefix("renderD") else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn choose_h264_entrypoint(
    entrypoints: &[VAEntrypoint::Type],
) -> Option<VAEntrypoint::Type> {
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

fn sequence_buffer(job: &EncodeJob<'_>) -> libva::EncSequenceParameterBufferH264 {
    let fields = libva::H264EncSeqFields::new(
        1,
        1,
        0,
        0,
        1,
        LOG2_MAX_FRAME_NUM_MINUS4,
        0,
        LOG2_MAX_POC_LSB_MINUS4,
        0,
    );
    let crop_right = (job.coded_width - job.visible_width) / 2;
    let crop_bottom = (job.coded_height - job.visible_height) / 2;
    let crop = (crop_right != 0 || crop_bottom != 0)
        .then(|| libva::H264EncFrameCropOffsets::new(0, crop_right, 0, crop_bottom));
    libva::EncSequenceParameterBufferH264::new(
        0,
        level_idc(job.coded_width / 16 * (job.coded_height / 16)),
        300,
        300,
        1,
        job.bitrate_bps,
        1,
        (job.coded_width / 16) as u16,
        (job.coded_height / 16) as u16,
        &fields,
        0,
        0,
        0,
        0,
        0,
        [0; 256],
        crop,
        Some(libva::H264VuiFields::new(0, 1, 1, 16, 16, 1, 0, 1)),
        255,
        1,
        1,
        1,
        job.frame_rate * 2,
    )
}

fn picture_buffer(job: &EncodeJob<'_>, coded_buf: u32) -> libva::EncPictureParameterBufferH264 {
    let current = PictureH264::new(job.surface_id, job.frame_num, 0, job.poc, job.poc);
    let mut references = invalid_pictures::<16>();
    if let Some((id, frame_num, poc)) = job.reference.filter(|_| !job.idr) {
        references[0] = PictureH264::new(
            id,
            frame_num,
            VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            poc,
            poc,
        );
    }
    let fields = libva::H264EncPicFields::new(u32::from(job.idr), 1, 0, 0, 0, 0, 0, 1, 0, 0, 0);
    libva::EncPictureParameterBufferH264::new(
        current,
        references,
        coded_buf,
        0,
        0,
        0,
        job.frame_num as u16,
        QP,
        0,
        0,
        0,
        0,
        &fields,
    )
}

fn slice_buffer(job: &EncodeJob<'_>) -> libva::EncSliceParameterBufferH264 {
    let mut list0 = invalid_pictures::<32>();
    let list1 = invalid_pictures::<32>();
    if let Some((id, frame_num, poc)) = job.reference.filter(|_| !job.idr) {
        list0[0] = PictureH264::new(
            id,
            frame_num,
            VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            poc,
            poc,
        );
    }
    let macroblocks = (job.coded_width / 16) * (job.coded_height / 16);
    libva::EncSliceParameterBufferH264::new(
        0,
        macroblocks,
        VA_INVALID_ID,
        if job.idr { 2 } else { 0 },
        0,
        job.idr_id,
        job.poc as u16,
        0,
        [0, 0],
        1,
        0,
        0,
        0,
        list0,
        list1,
        0,
        0,
        0,
        [0; 32],
        [0; 32],
        0,
        [[0; 2]; 32],
        [[0; 2]; 32],
        0,
        [0; 32],
        [0; 32],
        0,
        [[0; 2]; 32],
        [[0; 2]; 32],
        0,
        0,
        0,
        0,
        0,
    )
}

fn access_unit(bitstream: &[u8], header: &[u8]) -> Result<EncodedSample, Error> {
    if bitstream.is_empty() {
        return Err(Error::EncodeRejected("the encoder produced no frame"));
    }
    let parsed =
        parse_annex_b(bitstream).map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
    let bytes = if parsed.has_idr() && !(parsed.has_sps() && parsed.has_pps()) {
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(bitstream);
        bytes
    } else {
        bitstream.to_vec()
    };
    EncodedSample::annex_b(parsed.has_idr(), bytes)
        .map_err(|_| Error::EncodeRejected("a keyframe needs SPS, PPS, and an IDR"))
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
