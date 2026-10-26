//! Linux H.264 decode.
//!
//! The hardware path is VA-API variable-length decode on the first DRM
//! render node that offers it, in [`crate::gpu::ranked`] order. A keyframe carries SPS, PPS, and an IDR;
//! those parameter sets open the session, and later predicted frames reuse
//! it. Output is BGRA for the UI to paint. Bitstreams are screen content:
//! do not log them.
//!
//! The accepted subset is Constrained Baseline, Main, and High progressive
//! video: one frame per access unit, P and IDR slices, `pic_order_cnt_type`
//! 0, sliding-window reference marking, and 4:2:0 eight-bit pictures. The
//! syntax reader refuses anything outside that subset with an explicit
//! error instead of misdecoding it.
// trace: records every ignored failure

use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use libva::BufferType;
use libva::Display;
use libva::H264PicFields;
use libva::H264SeqFields;
use libva::IQMatrix;
use libva::IQMatrixBufferH264;
use libva::Picture;
use libva::PictureH264;
use libva::PictureParameter;
use libva::PictureParameterBufferH264;
use libva::SliceParameter;
use libva::SliceParameterBufferH264;
use libva::Surface;
use libva::VAConfigAttrib;
use libva::VAConfigAttribType;
use libva::VAEntrypoint;
use libva::VAProfile;
use libva::VA_RT_FORMAT_YUV420;
use vnilink::EncodedFrame;
use vnilink_media::{
    h264_video_signal, nv12_to_bgra, parse_annex_b, Error, RawFrame, VideoDecoder, VideoSignal,
    UNDECLARED_SIGNAL,
};

use crate::gpu::ranked_render_nodes;
use crate::syntax::{Marking, Pps, Slice, SliceKind, Sps};

/// Decoded BGRA picture with its size, moved back to the caller thread.
struct DecodedPicture {
    width: u16,
    height: u16,
    bgra: Vec<u8>,
}

enum Job {
    Decode {
        payload: Vec<u8>,
        keyframe: bool,
        timestamp_us: u64,
        reply: Sender<Result<DecodedPicture, Error>>,
    },
    Stop,
}

/// One decoded reference picture kept for later pictures.
struct Reference {
    surface: Surface<()>,
    frame_num: u32,
    poc: i32,
}

/// Picture-order state for `pic_order_cnt_type` 0.
#[derive(Default)]
struct PocState {
    prev_msb: i32,
    prev_lsb: u32,
}

struct OpenDecoder {
    context: Rc<libva::Context>,
    format: libva::VAImageFormat,
    coded_width: u32,
    coded_height: u32,
    visible_width: u16,
    visible_height: u16,
    max_refs: usize,
    parameter_key: Vec<u8>,
    /// The colour the SPS says, BT.709 limited range when it says none.
    signal: VideoSignal,
    sps: Sps,
    pps: Pps,
    free: Vec<Surface<()>>,
    refs: Vec<Reference>,
    poc: PocState,
}

pub struct LinuxDecoder {
    software: vnilink_media::SoftwareDecoderSlot,
    worker: Option<Worker>,
}

struct Worker {
    tx: Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl LinuxDecoder {
    pub fn new() -> Self {
        Self {
            worker: None,
            software: Default::default(),
        }
    }

    fn ensure_worker(&mut self) -> Result<&Worker, Error> {
        if self.worker.is_none() {
            let (tx, rx) = mpsc::channel();
            let thread = thread::Builder::new()
                .name("vnilink-vaapi-decode".to_string())
                .spawn(move || worker(rx))
                .map_err(|_| Error::Unsupported("VA-API H.264"))?;
            self.worker = Some(Worker {
                tx,
                thread: Some(thread),
            });
        }
        Ok(self.worker.as_ref().expect("worker is set"))
    }
}

impl Default for LinuxDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LinuxDecoder {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            vnilink::ignored!(
                vnilink::trace::DECODE,
                "a decoder thread already gone needs no stop",
                worker.tx.send(Job::Stop)
            );
            if let Some(thread) = worker.thread.take() {
                if thread.join().is_err() {
                    tracing::warn!(target: vnilink::trace::DECODE, "the VA-API decoder thread panicked");
                }
            }
        }
    }
}

impl VideoDecoder for LinuxDecoder {
    fn decode(&mut self, frame: &EncodedFrame) -> Result<RawFrame, Error> {
        if let Some(result) = self.software.decode(frame) {
            return result;
        }
        let worker = self.ensure_worker()?;
        let (reply_tx, reply_rx) = mpsc::channel();
        worker
            .tx
            .send(Job::Decode {
                payload: frame.payload.to_vec(),
                keyframe: frame.keyframe,
                timestamp_us: frame.timestamp_us,
                reply: reply_tx,
            })
            .map_err(|_| Error::DecodeRejected("VA-API rejected the frame"))?;
        let picture = reply_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| Error::DecodeRejected("VA-API rejected the frame"))??;
        RawFrame::bgra(
            picture.width,
            picture.height,
            frame.timestamp_us,
            picture.bgra,
        )
    }
}

fn worker(jobs: Receiver<Job>) {
    let mut open: Option<OpenDecoder> = None;
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Stop => break,
            Job::Decode {
                payload,
                keyframe,
                timestamp_us,
                reply,
            } => {
                let result = decode_picture(&mut open, &payload, keyframe, timestamp_us);
                vnilink::ignored!(
                    vnilink::trace::DECODE,
                    "the caller stopped waiting for its picture",
                    reply.send(result)
                );
            }
        }
    }
}

fn decode_picture(
    open: &mut Option<OpenDecoder>,
    payload: &[u8],
    keyframe: bool,
    timestamp_us: u64,
) -> Result<DecodedPicture, Error> {
    if payload.is_empty() {
        return Err(Error::InvalidFrame("encoded frame is empty"));
    }
    let parsed = parse_annex_b(payload)?;
    let mut parameter_sets = Vec::new();
    let mut slices = Vec::new();
    let mut has_idr = false;
    let mut uniform_idr = None;
    for unit in parsed.units() {
        let header = unit.bytes().first().copied().unwrap_or(0);
        if header & 0x80 != 0 {
            return Err(Error::InvalidFrame("H.264 NAL header is invalid"));
        }
        match unit.nal_type {
            7 | 8 => parameter_sets.push(unit.bytes().to_vec()),
            1 | 5 => {
                let is_idr = unit.nal_type == 5;
                has_idr |= is_idr;
                match uniform_idr {
                    Some(previous) if previous != is_idr => {
                        return Err(Error::DecodeRejected(
                            "a coded picture mixes IDR and non-IDR slices",
                        ));
                    }
                    _ => uniform_idr = Some(is_idr),
                }
                slices.push(unit.bytes().to_vec());
            }
            // Data partitions carry no slice header of their own.
            2 | 3 | 4 => {
                return Err(Error::Unsupported(
                    "partitioned H.264 slices are not supported",
                ));
            }
            // Aud units, SEI, and filler carry no picture data.
            6 | 9 | 12 => {}
            _ => {
                return Err(Error::DecodeRejected("VA-API rejected the frame"));
            }
        }
    }
    if slices.is_empty() {
        return Err(Error::InvalidFrame("encoded frame has no picture"));
    }
    if keyframe || has_idr {
        if !parsed.has_sps() || !parsed.has_pps() || !has_idr {
            return Err(Error::DecodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ));
        }
    }
    // The last parameter set of each kind wins; our transport sends one of
    // each on a keyframe. Two passes, so a PPS never depends on SPS order
    // inside the access unit.
    let mut fresh_sps = None;
    let mut signal = UNDECLARED_SIGNAL;
    for set in &parameter_sets {
        if set.first().map(|header| header & 0x1F) == Some(7) {
            fresh_sps = Some(Sps::parse(set)?);
            signal = h264_video_signal(set)?.unwrap_or(UNDECLARED_SIGNAL);
        }
    }
    let mut fresh_pps = None;
    for set in &parameter_sets {
        if set.first().map(|header| header & 0x1F) == Some(8) {
            let sps = fresh_sps.clone().ok_or(Error::DecodeRejected(
                "a keyframe needs SPS, PPS, and an IDR",
            ))?;
            fresh_pps = Some(Pps::parse(set, &sps)?);
        }
    }
    // Slices without parameter sets reuse the running session.
    if open.is_none() {
        if parameter_sets.is_empty() {
            return Err(Error::DecodeRejected("the decoder has not seen a keyframe"));
        }
        let (sps, pps) = match (fresh_sps, fresh_pps) {
            (Some(sps), Some(pps)) => (sps, pps),
            _ => {
                return Err(Error::DecodeRejected(
                    "a keyframe needs SPS, PPS, and an IDR",
                ));
            }
        };
        let key = parameter_sets.concat();
        *open = Some(OpenDecoder::open(sps, pps, key, signal)?);
    } else if !parameter_sets.is_empty() {
        let (sps, pps) = match (fresh_sps, fresh_pps) {
            (Some(sps), Some(pps)) => (sps, pps),
            _ => {
                return Err(Error::DecodeRejected("parameter set is incomplete"));
            }
        };
        let key = parameter_sets.concat();
        let changed = key != open.as_ref().expect("decoder is open").parameter_key;
        if changed {
            *open = Some(OpenDecoder::open(sps, pps, key, signal)?);
        }
    }
    let open = open.as_mut().expect("decoder is open");
    open.decode_slices(&slices, timestamp_us)
}

fn profile_for(sps: &Sps) -> Result<VAProfile::Type, Error> {
    match sps.profile {
        66 if sps.constraints & 0x40 != 0 => Ok(VAProfile::VAProfileH264ConstrainedBaseline),
        66 => Ok(VAProfile::VAProfileH264Baseline),
        77 => Ok(VAProfile::VAProfileH264Main),
        100 => Ok(VAProfile::VAProfileH264High),
        _ => Err(Error::Unsupported("this H.264 profile is not supported")),
    }
}

fn try_open(
    path: &std::path::Path,
    sps: &Sps,
    surfaces: usize,
) -> Option<(Rc<libva::Context>, Vec<Surface<()>>, libva::VAImageFormat)> {
    let display = Display::open_drm_display(path).ok()?;
    let profile = profile_for(sps).ok()?;
    let entrypoints = display.query_config_entrypoints(profile).ok()?;
    if !entrypoints
        .iter()
        .any(|entry| *entry == VAEntrypoint::VAEntrypointVLD)
    {
        return None;
    }
    let mut attrs = vec![VAConfigAttrib {
        type_: VAConfigAttribType::VAConfigAttribRTFormat,
        value: 0,
    }];
    display
        .get_config_attributes(profile, VAEntrypoint::VAEntrypointVLD, &mut attrs)
        .ok()?;
    if attrs[0].value & VA_RT_FORMAT_YUV420 == 0 {
        return None;
    }
    attrs[0].value = VA_RT_FORMAT_YUV420;
    let config = display
        .create_config(attrs, profile, VAEntrypoint::VAEntrypointVLD)
        .ok()?;
    let coded_width = u32::from(sps.coded.0);
    let coded_height = u32::from(sps.coded.1);
    let owned: Vec<()> = (0..surfaces).map(|_| ()).collect();
    let created = display
        .create_surfaces(
            VA_RT_FORMAT_YUV420,
            None,
            coded_width,
            coded_height,
            Some(libva::UsageHint::USAGE_HINT_DECODER),
            owned,
        )
        .ok()?;
    let context = display
        .create_context(&config, coded_width, coded_height, Some(&created), true)
        .ok()?;
    let format = display
        .query_image_formats()
        .ok()?
        .into_iter()
        .find(|format| format.fourcc == libva::VA_FOURCC_NV12)?;
    Some((context, created, format))
}

impl OpenDecoder {
    fn open(sps: Sps, pps: Pps, key: Vec<u8>, signal: VideoSignal) -> Result<Self, Error> {
        if sps.visible.0 % 2 != 0 || sps.visible.1 % 2 != 0 {
            return Err(Error::Unsupported(
                "odd H.264 picture sizes are not supported",
            ));
        }
        if sps.visible.0 == 0 || sps.visible.1 == 0 {
            return Err(Error::InvalidFrame("H.264 SPS picture size is empty"));
        }
        let surfaces = sps.num_ref_frames as usize + 2;
        for path in ranked_render_nodes() {
            if let Some((context, created, format)) = try_open(&path, &sps, surfaces.max(2)) {
                return Ok(Self {
                    context,
                    format,
                    coded_width: u32::from(sps.coded.0),
                    coded_height: u32::from(sps.coded.1),
                    visible_width: sps.visible.0,
                    visible_height: sps.visible.1,
                    max_refs: (sps.num_ref_frames as usize).max(1),
                    parameter_key: key,
                    signal,
                    sps,
                    pps,
                    free: created,
                    refs: Vec::new(),
                    poc: PocState::default(),
                });
            }
        }
        Err(Error::Unsupported("VA-API H.264"))
    }

    fn decode_slices(
        &mut self,
        slices: &[Vec<u8>],
        timestamp_us: u64,
    ) -> Result<DecodedPicture, Error> {
        // Parse every slice header before touching the DPB.
        let mut parsed = Vec::with_capacity(slices.len());
        let mut uniform_marking = None;
        for nal in slices {
            let header = nal.first().copied().unwrap_or(0);
            let nal_type = header & 0x1F;
            let nal_ref_idc = (header >> 5) & 3;
            let is_idr = nal_type == 5;
            let slice = Slice::parse(nal, &self.sps, &self.pps, is_idr, nal_ref_idc)?;
            match &uniform_marking {
                Some(previous) if *previous != slice.marking => {
                    return Err(Error::DecodeRejected(
                        "slices of one picture disagree on reference marking",
                    ));
                }
                _ => uniform_marking = Some(slice.marking.clone()),
            }
            parsed.push((slice, nal_ref_idc, is_idr, nal));
        }
        let is_idr = parsed.iter().any(|(_, _, is_idr, _)| *is_idr);
        // One picture shares its frame number, order count, and reference
        // bit across slices.
        let (frame_num, poc_lsb, nal_ref_idc) = {
            let first = &parsed[0];
            (first.0.frame_num, first.0.poc_lsb, first.1)
        };
        for (slice, ref_idc, _, _) in &parsed {
            if slice.frame_num != frame_num || slice.poc_lsb != poc_lsb || *ref_idc != nal_ref_idc {
                return Err(Error::DecodeRejected(
                    "slices of one picture disagree on its identity",
                ));
            }
        }
        let store_current = nal_ref_idc != 0 && self.sps.num_ref_frames > 0;
        // Picture order first: the DPB slots below are stamped with it.
        if is_idr {
            self.poc = PocState::default();
        }
        let poc = self.picture_order(&parsed[0].0)?;
        // Reference marking, then room for the picture being decoded.
        match &parsed[0].0.marking {
            Marking::Sliding => {
                if is_idr {
                    self.flush_refs();
                }
            }
            Marking::FlushAll => self.flush_refs(),
            Marking::MarkNonRef { diffs } => self.mark_non_ref(&parsed[0].0, diffs)?,
        }
        if store_current {
            while self.refs.len() >= self.max_refs {
                self.evict_oldest();
            }
        }
        let current = self
            .free
            .pop()
            .ok_or(Error::DecodeRejected("VA-API rejected the frame"))?;
        let outcome = self.submit(&parsed, current, poc, store_current, timestamp_us);
        let (surface, picture) = match outcome {
            Ok(pair) => pair,
            Err(error) => {
                // The DPB references stay intact; only the failed picture is
                // dropped, and the next keyframe restarts cleanly.
                return Err(error);
            }
        };
        if store_current {
            self.refs.push(Reference {
                surface,
                frame_num: parsed[0].0.frame_num,
                poc,
            });
        } else {
            self.free.push(surface);
        }
        self.poc.prev_msb = poc - parsed[0].0.poc_lsb as i32;
        self.poc.prev_lsb = parsed[0].0.poc_lsb;
        Ok(picture)
    }

    /// Picture order count for `pic_order_cnt_type` 0.
    fn picture_order(&self, slice: &Slice) -> Result<i32, Error> {
        let max_lsb = 1i64 << (u32::from(self.sps.log2_max_poc_lsb_minus4) + 4);
        let lsb = i64::from(slice.poc_lsb);
        let prev_msb = i64::from(self.poc.prev_msb);
        let prev_lsb = i64::from(self.poc.prev_lsb);
        let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
            prev_msb + max_lsb
        } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
            prev_msb - max_lsb
        } else {
            prev_msb
        };
        i32::try_from(msb + lsb)
            .map_err(|_| Error::InvalidFrame("H.264 picture order is out of range"))
    }

    fn flush_refs(&mut self) {
        for slot in self.refs.drain(..) {
            self.free.push(slot.surface);
        }
    }

    fn evict_oldest(&mut self) {
        if self.refs.is_empty() {
            return;
        }
        let mut oldest = 0;
        for (index, slot) in self.refs.iter().enumerate() {
            if slot.poc < self.refs[oldest].poc {
                oldest = index;
            }
        }
        let slot = self.refs.remove(oldest);
        self.free.push(slot.surface);
    }

    fn mark_non_ref(&mut self, slice: &Slice, diffs: &[u32]) -> Result<(), Error> {
        let max_frame_num = 1u32 << (self.sps.log2_max_frame_num_minus4 + 4);
        for &diff in diffs {
            let pic_num =
                (slice.frame_num + max_frame_num - (diff + 1) % max_frame_num) % max_frame_num;
            if let Some(index) = self.refs.iter().position(|slot| slot.frame_num == pic_num) {
                let slot = self.refs.remove(index);
                self.free.push(slot.surface);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        parsed: &[(Slice, u8, bool, &Vec<u8>)],
        current: Surface<()>,
        poc: i32,
        store_current: bool,
        timestamp_us: u64,
    ) -> Result<(Surface<()>, DecodedPicture), Error> {
        let fail = |_: libva::VaError| Error::DecodeRejected("VA-API rejected the frame");
        let sps = &self.sps;
        let pps = &self.pps;
        let reference_frames: [PictureH264; 16] = std::array::from_fn(|index| {
            self.refs.get(index).map_or_else(
                || {
                    PictureH264::new(
                        libva::VA_INVALID_ID,
                        0,
                        libva::VA_PICTURE_H264_INVALID,
                        0,
                        0,
                    )
                },
                |slot| {
                    PictureH264::new(
                        slot.surface.id(),
                        slot.frame_num,
                        libva::VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                        slot.poc,
                        slot.poc,
                    )
                },
            )
        });
        // Default reference list: short-term pictures by descending order count.
        let mut ordered: Vec<&Reference> = self.refs.iter().collect();
        ordered.sort_by(|left, right| right.poc.cmp(&left.poc));
        let invalid = || {
            PictureH264::new(
                libva::VA_INVALID_ID,
                0,
                libva::VA_PICTURE_H264_INVALID,
                0,
                0,
            )
        };
        let ref_picture = |slot: &Reference| {
            PictureH264::new(
                slot.surface.id(),
                slot.frame_num,
                libva::VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                slot.poc,
                slot.poc,
            )
        };
        let seq_fields = H264SeqFields::new(
            1,
            0,
            0,
            1,
            0,
            u32::from(sps.direct_8x8),
            u32::from(sps.level >= 31),
            sps.log2_max_frame_num_minus4,
            sps.poc_type,
            sps.log2_max_poc_lsb_minus4,
            0,
        );
        let pic_fields = H264PicFields::new(
            u32::from(pps.entropy_cabac),
            u32::from(pps.weighted_pred),
            u32::from(pps.weighted_bipred),
            u32::from(pps.transform_8x8),
            0,
            u32::from(pps.constrained_intra),
            u32::from(pps.bottom_field_order_present),
            u32::from(pps.deblocking_present),
            u32::from(pps.redundant_present),
            u32::from(store_current),
        );
        let current_id = current.id();
        let current_flags = if store_current {
            libva::VA_PICTURE_H264_SHORT_TERM_REFERENCE
        } else {
            0
        };
        let picture = PictureParameterBufferH264::new(
            PictureH264::new(current_id, parsed[0].0.frame_num, current_flags, poc, poc),
            reference_frames,
            sps.pic_width_mbs_minus1 as u16,
            sps.pic_height_map_minus1 as u16,
            0,
            0,
            sps.num_ref_frames as u8,
            &seq_fields,
            0,
            0,
            0,
            int8(pps.pic_init_qp - 26)?,
            int8(pps.pic_init_qs - 26)?,
            int8(pps.chroma_qp_offset[0])?,
            int8(pps.chroma_qp_offset[1])?,
            &pic_fields,
            parsed[0].0.frame_num as u16,
        );
        let iq = IQMatrixBufferH264::new(pps.scaling4, pps.scaling8);
        let mut slice_params = SliceParameterBufferH264::new_array();
        let mut slice_data = Vec::new();
        for (slice, _, _, nal) in parsed {
            let weights = ExpandedWeights::of(slice);
            let ref_list_0: [PictureH264; 32] = std::array::from_fn(|index| {
                ordered
                    .get(index)
                    .map_or_else(invalid, |slot| ref_picture(slot))
            });
            let ref_list_1: [PictureH264; 32] = std::array::from_fn(|_| invalid());
            slice_params.add_slice_parameter(
                nal.len() as u32,
                slice_data.len() as u32,
                libva::VA_SLICE_DATA_FLAG_ALL,
                slice.header_bits,
                slice.first_mb,
                match slice.kind {
                    SliceKind::P => 0,
                    SliceKind::I => 2,
                },
                0,
                slice.num_ref_l0.saturating_sub(1) as u8,
                0,
                slice.cabac_init,
                slice.qp_delta as i8,
                slice.deblocking_idc,
                slice.alpha_offset as i8,
                slice.beta_offset as i8,
                ref_list_0,
                ref_list_1,
                weights.luma_denom,
                weights.chroma_denom,
                weights.luma_flag,
                weights.luma_weight,
                weights.luma_offset,
                weights.chroma_flag,
                weights.chroma_weight,
                weights.chroma_offset,
                0,
                [0; 32],
                [0; 32],
                0,
                [[0; 2]; 32],
                [[0; 2]; 32],
            );
            slice_data.extend_from_slice(nal);
        }
        let context = Rc::clone(&self.context);
        let pic_param = context
            .create_buffer(BufferType::PictureParameter(PictureParameter::H264(
                picture,
            )))
            .map_err(fail)?;
        let iq_param = context
            .create_buffer(BufferType::IQMatrix(IQMatrix::H264(iq)))
            .map_err(fail)?;
        let slice_param = context
            .create_buffer(BufferType::SliceParameter(SliceParameter::H264(
                slice_params,
            )))
            .map_err(fail)?;
        let data_param = context
            .create_buffer(BufferType::SliceData(slice_data))
            .map_err(fail)?;
        let mut picture = Picture::new(timestamp_us, context, current);
        picture.add_buffer(pic_param);
        picture.add_buffer(iq_param);
        picture.add_buffer(slice_param);
        picture.add_buffer(data_param);
        let picture = picture.begin().map_err(fail)?;
        let picture = picture.render().map_err(fail)?;
        let picture = picture.end().map_err(fail)?;
        let picture = match picture.sync() {
            Ok(picture) => picture,
            Err(_) => return Err(Error::DecodeRejected("VA-API rejected the frame")),
        };
        let decoded = download(&picture, self)?;
        let surface = picture
            .take_surface()
            .map_err(|_| Error::DecodeRejected("VA-API rejected the frame"))?;
        Ok((surface, decoded))
    }
}

fn int8(value: i32) -> Result<i8, Error> {
    i8::try_from(value).map_err(|_| Error::InvalidFrame("H.264 parameter is out of range"))
}

/// Weight table expanded to the VA-API arrays, with plain defaults where the
/// stream carries no table.
struct ExpandedWeights {
    luma_denom: u8,
    chroma_denom: u8,
    luma_flag: u8,
    luma_weight: [i16; 32],
    luma_offset: [i16; 32],
    chroma_flag: u8,
    chroma_weight: [[i16; 2]; 32],
    chroma_offset: [[i16; 2]; 32],
}

impl ExpandedWeights {
    fn of(slice: &Slice) -> Self {
        let mut expanded = Self {
            luma_denom: 0,
            chroma_denom: 0,
            luma_flag: 0,
            luma_weight: [0; 32],
            luma_offset: [0; 32],
            chroma_flag: 0,
            chroma_weight: [[0; 2]; 32],
            chroma_offset: [[0; 2]; 32],
        };
        let Some(table) = &slice.weights else {
            return expanded;
        };
        expanded.luma_denom = table.luma_denom;
        expanded.chroma_denom = table.chroma_denom;
        for (index, entry) in table.entries.iter().enumerate().take(32) {
            if entry.luma_flag {
                expanded.luma_flag = 1;
                expanded.luma_weight[index] = entry.luma_weight;
                expanded.luma_offset[index] = entry.luma_offset;
            } else {
                expanded.luma_weight[index] = 1 << table.luma_denom;
            }
            if entry.chroma_flag {
                expanded.chroma_flag = 1;
                expanded.chroma_weight[index] = entry.chroma_weight;
                expanded.chroma_offset[index] = entry.chroma_offset;
            } else {
                expanded.chroma_weight[index] = [1 << table.chroma_denom; 2];
            }
        }
        expanded
    }
}

fn download(
    picture: &Picture<libva::PictureSync, Surface<()>>,
    open: &OpenDecoder,
) -> Result<DecodedPicture, Error> {
    let image = picture
        .create_image(
            open.format,
            (open.coded_width, open.coded_height),
            (open.coded_width, open.coded_height),
        )
        .map_err(|_| Error::DecodeRejected("VA-API rejected the frame"))?;
    let descriptor = *image.image();
    if descriptor.num_planes < 2 {
        return Err(Error::DecodeRejected("VA-API rejected the frame"));
    }
    let data: &[u8] = image.as_ref();
    let width = usize::from(open.visible_width);
    let height = usize::from(open.visible_height);
    let mut y = vec![0u8; width * height];
    let mut uv = vec![0u8; width * height / 2];
    copy_visible(
        data,
        &mut y,
        descriptor.offsets[0] as usize,
        descriptor.pitches[0] as usize,
        width,
        height,
    )?;
    copy_visible(
        data,
        &mut uv,
        descriptor.offsets[1] as usize,
        descriptor.pitches[1] as usize,
        width,
        height / 2,
    )?;
    let frame = nv12_to_bgra(
        open.visible_width,
        open.visible_height,
        0,
        &y,
        &uv,
        open.signal,
    )?;
    Ok(DecodedPicture {
        width: open.visible_width,
        height: open.visible_height,
        bgra: frame_bytes(&frame),
    })
}

fn copy_visible(
    data: &[u8],
    dest: &mut [u8],
    offset: usize,
    pitch: usize,
    width: usize,
    height: usize,
) -> Result<(), Error> {
    if pitch < width {
        return Err(Error::DecodeRejected("VA-API rejected the frame"));
    }
    for row in 0..height {
        let start = offset + row * pitch;
        let end = start + width;
        if end > data.len() || (row + 1) * width > dest.len() {
            return Err(Error::DecodeRejected("VA-API rejected the frame"));
        }
        dest[row * width..(row + 1) * width].copy_from_slice(&data[start..end]);
    }
    Ok(())
}

fn frame_bytes(frame: &RawFrame) -> Vec<u8> {
    let mut bgra = Vec::with_capacity(usize::from(frame.width()) * usize::from(frame.height()) * 4);
    for row in 0..frame.height() {
        bgra.extend_from_slice(frame.row(row));
    }
    bgra
}
