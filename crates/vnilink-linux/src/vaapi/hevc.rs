//! H.265 Main for the VA-API encoder: one slice, each picture predicted
//! from the one before it. The driver writes the slice header from these
//! numbers; `crate::hevc_parameter_set` writes the VPS, SPS and PPS it may
//! leave out.

use libva::{
    BufferType, EncPackedHeaderParameter, EncPackedHeaderType, EncPictureParameter,
    EncSequenceParameter, EncSliceParameter, PictureHEVC, VA_INVALID_ID, VA_PICTURE_HEVC_INVALID,
};
use vnilink_media::{hevc_sample, hevc_units, EncodedSample, Error};

use super::PictureJob;
use crate::hevc_parameter_set::{
    level_idc, parameter_sets, slice_header, slice_type, Layout, IDR_W_RADL,
    LOG2_MAX_POC_LSB_MINUS4, TRAIL_R,
};

pub(super) const QP: u8 = 26;

pub(super) struct Syntax {
    layout: Layout,
    header: Vec<u8>,
    /// The driver takes this encoder's parameter sets and slice headers.
    packed: bool,
    poc: i32,
    /// Picture order of the picture the next one predicts from.
    previous: Option<i32>,
}

impl Syntax {
    pub(super) fn new(
        layout: Layout,
        packed: bool,
        width: u16,
        height: u16,
    ) -> Result<Self, Error> {
        Ok(Self {
            layout,
            header: parameter_sets(&layout, width, height)?,
            packed,
            poc: 0,
            previous: None,
        })
    }

    pub(super) fn buffers(&self, job: &PictureJob) -> Vec<BufferType> {
        let poc = if job.key { 0 } else { self.poc };
        let reference = job
            .reference
            .zip(self.previous)
            .filter(|_| !job.key)
            .map(|(id, poc)| PictureHEVC::new(id, poc, 0));
        let mut buffers = vec![
            BufferType::EncSequenceParameter(EncSequenceParameter::HEVC(self.sequence(job))),
            BufferType::EncPictureParameter(EncPictureParameter::HEVC(
                self.picture(job, poc, reference),
            )),
        ];
        if self.packed {
            if job.key {
                buffers.extend(packed(EncPackedHeaderType::Sequence, self.header.clone()));
            }
            buffers.extend(packed(
                EncPackedHeaderType::Slice,
                slice_header(&self.layout, job.key, poc as u32),
            ));
        }
        buffers.push(BufferType::EncSliceParameter(EncSliceParameter::HEVC(
            self.slice(job, reference),
        )));
        buffers
    }

    /// Advances picture order once the picture was encoded.
    pub(super) fn encoded(&mut self, key: bool) {
        let poc = if key { 0 } else { self.poc };
        self.previous = Some(poc);
        self.poc = (poc + 1) % (1 << (LOG2_MAX_POC_LSB_MINUS4 + 4));
    }

    /// The driver's output as an access unit: a keyframe without parameter
    /// sets gets this encoder's.
    pub(super) fn sample(&self, bitstream: &[u8]) -> Result<EncodedSample, Error> {
        if bitstream.is_empty() {
            return Err(Error::EncodeRejected("the encoder produced no frame"));
        }
        let units = hevc_units(bitstream)
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        let kind = |nal: &&[u8]| (nal[0] >> 1) & 63;
        let key = units.iter().any(|nal| (16..=21).contains(&kind(nal)));
        let complete = [32, 33, 34]
            .into_iter()
            .all(|set| units.iter().any(|nal| kind(nal) == set));
        let bytes = if key && !complete {
            let mut bytes = self.header.clone();
            bytes.extend(
                units
                    .iter()
                    .filter(|nal| !(32..=34).contains(&kind(nal)))
                    .flat_map(|nal| [&[0, 0, 0, 1][..], nal].concat()),
            );
            bytes
        } else {
            bitstream.to_vec()
        };
        hevc_sample(bytes)
    }

    fn sequence(&self, job: &PictureJob) -> libva::EncSequenceParameterBufferHEVC {
        let layout = &self.layout;
        let fields = libva::HEVCEncSeqFields::new(
            1,
            0,
            0,
            0,
            0,
            u32::from(layout.strong_intra_smoothing),
            u32::from(layout.amp),
            u32::from(layout.sao),
            0,
            0,
            u32::from(layout.temporal_mvp),
            1,
            0,
        );
        libva::EncSequenceParameterBufferHEVC::new(
            1,
            level_idc(job.coded_width * job.coded_height),
            0,
            super::INTRA_PERIOD,
            super::INTRA_PERIOD,
            1,
            job.bitrate_bps,
            job.coded_width as u16,
            job.coded_height as u16,
            &fields,
            (layout.log2_min_cb - 3) as u8,
            (layout.log2_ctb - layout.log2_min_cb) as u8,
            (layout.log2_min_tb - 2) as u8,
            (layout.log2_max_tb - layout.log2_min_tb) as u8,
            layout.depth_inter as u8,
            layout.depth_intra as u8,
            0,
            0,
            0,
            0,
            None,
            0,
            0,
            0,
            1,
            job.frame_rate,
            0,
            0,
            0,
            &libva::HevcEncSeqSccFields::new(0),
        )
    }

    fn picture(
        &self,
        job: &PictureJob,
        poc: i32,
        reference: Option<PictureHEVC>,
    ) -> libva::EncPictureParameterBufferHEVC {
        let layout = &self.layout;
        let mut references = invalid_pictures();
        if let Some(reference) = reference {
            references[0] = reference;
        }
        let fields = libva::HEVCEncPicFields::new(
            u32::from(job.key),
            if job.key { 1 } else { 2 },
            1,
            0,
            0,
            0,
            u32::from(layout.transform_skip),
            u32::from(layout.cu_qp_delta),
            0,
            0,
            0,
            0,
            0,
            0,
            u32::from(layout.loop_filter_across_slices),
            0,
            0,
            0,
            0,
        );
        libva::EncPictureParameterBufferHEVC::new(
            PictureHEVC::new(job.surface_id, poc, 0),
            references,
            job.coded_buf,
            if layout.temporal_mvp { 0 } else { 0xFF },
            0,
            QP,
            layout.cu_qp_delta_depth() as u8,
            0,
            0,
            0,
            0,
            [0; 19],
            [0; 21],
            0,
            0,
            0,
            0,
            0,
            if job.key { IDR_W_RADL } else { TRAIL_R },
            &fields,
            0,
            0,
            &libva::HevcEncPicSccFields::new(0),
        )
    }

    fn slice(
        &self,
        job: &PictureJob,
        reference: Option<PictureHEVC>,
    ) -> libva::EncSliceParameterBufferHEVC {
        let layout = &self.layout;
        let mut list0 = invalid_pictures();
        let mut list1 = invalid_pictures();
        if let Some(reference) = reference {
            list0[0] = reference;
            if layout.generalized_b {
                list1[0] = reference;
            }
        }
        let fields = libva::HevcEncSliceFields::new(
            1,
            0,
            0,
            u32::from(layout.temporal_mvp && reference.is_some()),
            u32::from(layout.sao),
            u32::from(layout.sao),
            0,
            0,
            0,
            0,
            0,
            // P slices take their collocated picture from list 0.
            u32::from(!job.key),
        );
        libva::EncSliceParameterBufferHEVC::new(
            0,
            layout.ctbs((job.coded_width, job.coded_height)),
            slice_type(layout, job.key),
            0,
            0,
            0,
            list0,
            list1,
            0,
            0,
            [0; 15],
            [0; 15],
            [[0; 2]; 15],
            [[0; 2]; 15],
            [0; 15],
            [0; 15],
            [[0; 2]; 15],
            [[0; 2]; 15],
            5,
            0,
            0,
            0,
            0,
            0,
            &fields,
            0,
            0,
        )
    }
}

/// One header the driver writes as given, emulation prevention included.
fn packed(kind: EncPackedHeaderType, header: Vec<u8>) -> [BufferType; 2] {
    [
        BufferType::EncPackedHeaderParameter(EncPackedHeaderParameter::new(
            kind,
            header.len() as u32 * 8,
            true,
        )),
        BufferType::EncPackedHeaderData(header),
    ]
}

fn invalid_pictures() -> [PictureHEVC; 15] {
    [PictureHEVC::new(VA_INVALID_ID, 0, VA_PICTURE_HEVC_INVALID); 15]
}
