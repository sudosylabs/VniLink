//! H.264 for the VA-API encoder: Constrained Baseline or High, one slice,
//! each picture predicted from the one before it. The driver writes the
//! slice header from these numbers; `crate::parameter_set` writes the SPS
//! and PPS it may leave out.

use libva::{
    BufferType, EncPictureParameter, EncSequenceParameter, EncSliceParameter, PictureH264,
    VA_INVALID_ID, VA_INVALID_SURFACE, VA_PICTURE_H264_SHORT_TERM_REFERENCE,
};
use vnilink::YuvMatrix;
use vnilink_media::{parse_annex_b, EncodedSample, Error};

use super::PictureJob;
use crate::parameter_set::{
    level_idc, parameter_sets, EncodeProfile, LOG2_MAX_FRAME_NUM_MINUS4, LOG2_MAX_POC_LSB_MINUS4,
    MAX_FRAME_NUM, MAX_POC_LSB,
};

pub(super) const QP: u8 = 26;

pub(super) struct Syntax {
    profile: EncodeProfile,
    header: Vec<u8>,
    frame_num: u32,
    poc: i32,
    idr_id: u16,
    /// `frame_num` and picture order of the picture the next one predicts from.
    previous: Option<(u32, i32)>,
    /// The matrix every SPS this encoder hands out says.
    matrix: YuvMatrix,
}

impl Syntax {
    pub(super) fn new(
        high: bool,
        width: u16,
        height: u16,
        frame_rate: u8,
        matrix: YuvMatrix,
    ) -> Result<Self, Error> {
        let profile = if high {
            EncodeProfile::High
        } else {
            EncodeProfile::Baseline
        };
        Ok(Self {
            profile,
            header: parameter_sets(profile, width, height, frame_rate)?,
            frame_num: 0,
            poc: 0,
            idr_id: 0,
            previous: None,
            matrix,
        })
    }

    pub(super) fn buffers(&self, job: &PictureJob) -> Vec<BufferType> {
        let frame_num = if job.key { 0 } else { self.frame_num };
        let poc = if job.key { 0 } else { self.poc };
        // Surface id, `frame_num` and picture order of the reference.
        let reference = job
            .reference
            .zip(self.previous)
            .filter(|_| !job.key)
            .map(|(id, (frame_num, poc))| (id, frame_num, poc));
        vec![
            BufferType::EncSequenceParameter(EncSequenceParameter::H264(sequence(job))),
            BufferType::EncPictureParameter(EncPictureParameter::H264(picture(
                job,
                self.profile,
                frame_num,
                poc,
                reference,
            ))),
            BufferType::EncSliceParameter(EncSliceParameter::H264(slice(
                job,
                self.idr_id,
                poc,
                reference,
            ))),
        ]
    }

    /// Advances the counters once the picture was encoded.
    pub(super) fn encoded(&mut self, key: bool) {
        let frame_num = if key { 0 } else { self.frame_num };
        let poc = if key { 0 } else { self.poc };
        self.previous = Some((frame_num, poc));
        if key {
            self.frame_num = 1 % MAX_FRAME_NUM;
            self.poc = 2 % MAX_POC_LSB;
            self.idr_id = self.idr_id.wrapping_add(1);
        } else {
            self.frame_num = (frame_num + 1) % MAX_FRAME_NUM;
            self.poc = (poc + 2) % MAX_POC_LSB;
        }
    }

    /// The driver's output as an access unit: a keyframe without parameter
    /// sets gets this encoder's.
    pub(super) fn sample(&self, bitstream: &[u8]) -> Result<EncodedSample, Error> {
        if bitstream.is_empty() {
            return Err(Error::EncodeRejected("the encoder produced no frame"));
        }
        let parsed = parse_annex_b(bitstream)
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        let bytes = if parsed.has_idr() && !(parsed.has_sps() && parsed.has_pps()) {
            let mut bytes = self.header.clone();
            bytes.extend_from_slice(bitstream);
            bytes
        } else {
            bitstream.to_vec()
        };
        EncodedSample::annex_b(parsed.has_idr(), bytes, self.matrix)
            .map_err(|_| Error::EncodeRejected("a keyframe needs SPS, PPS, and an IDR"))
    }
}

fn invalid_pictures<const N: usize>() -> [PictureH264; N] {
    std::array::from_fn(|_| PictureH264::new(VA_INVALID_ID, 0, VA_INVALID_SURFACE, 0, 0))
}

fn reference_picture((id, frame_num, poc): (u32, u32, i32)) -> PictureH264 {
    PictureH264::new(
        id,
        frame_num,
        VA_PICTURE_H264_SHORT_TERM_REFERENCE,
        poc,
        poc,
    )
}

fn sequence(job: &PictureJob) -> libva::EncSequenceParameterBufferH264 {
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
        super::INTRA_PERIOD,
        super::INTRA_PERIOD,
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

fn picture(
    job: &PictureJob,
    profile: EncodeProfile,
    frame_num: u32,
    poc: i32,
    reference: Option<(u32, u32, i32)>,
) -> libva::EncPictureParameterBufferH264 {
    let current = PictureH264::new(job.surface_id, frame_num, 0, poc, poc);
    let mut references = invalid_pictures::<16>();
    if let Some(reference) = reference {
        references[0] = reference_picture(reference);
    }
    let fields = libva::H264EncPicFields::new(
        u32::from(job.key),
        1,
        u32::from(profile == EncodeProfile::High),
        0,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
    );
    libva::EncPictureParameterBufferH264::new(
        current,
        references,
        job.coded_buf,
        0,
        0,
        0,
        frame_num as u16,
        QP,
        0,
        0,
        0,
        0,
        &fields,
    )
}

fn slice(
    job: &PictureJob,
    idr_id: u16,
    poc: i32,
    reference: Option<(u32, u32, i32)>,
) -> libva::EncSliceParameterBufferH264 {
    let mut list0 = invalid_pictures::<32>();
    let list1 = invalid_pictures::<32>();
    if let Some(reference) = reference {
        list0[0] = reference_picture(reference);
    }
    let macroblocks = (job.coded_width / 16) * (job.coded_height / 16);
    libva::EncSliceParameterBufferH264::new(
        0,
        macroblocks,
        VA_INVALID_ID,
        if job.key { 2 } else { 0 },
        0,
        idr_id,
        poc as u16,
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
