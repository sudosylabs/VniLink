//! BGRA to NV12 on the GPU, for the encoder on the same node.
//!
//! The video processor writes the stream's colour (BT.709, limited range:
//! [`vnilink_media::colour::STREAM_SIGNAL`]) straight into the encoder's
//! picture, so the host does not convert on the CPU. Capture is sRGB, full
//! range. A node without a video processor, or one that refuses BGRX, keeps
//! the CPU conversion.
//!
//! The source has the encoder's coded size, padded black, so the processor
//! maps whole surface to whole surface. Regions are left out: libva 0.1.4
//! frees a pipeline buffer's rectangles once the buffer is created, before
//! the driver reads them.

use std::rc::Rc;
use std::sync::Arc;

use libva::{
    BufferType, Display, Image, Picture, ProcColorProperties, ProcPipelineParameterBuffer, Surface,
    VAEntrypoint, VAProfile,
};
use vnilink_media::{Error, RawFrame};

/// `VAProcColorStandardType` in `va_vpp.h`.
const STANDARD_BT709: u8 = libva::_VAProcColorStandardType_VAProcColorStandardBT709 as u8;
const STANDARD_SRGB: u8 = libva::_VAProcColorStandardType_VAProcColorStandardSRGB as u8;
/// H.273 code points: BT.709 primaries, the sRGB and BT.709 transfers and
/// the BT.709 matrix. RGB has the identity matrix.
const PRIMARIES_BT709: u8 = 1;
const TRANSFER_SRGB: u8 = 13;
const TRANSFER_BT709: u8 = 1;
const MATRIX_RGB: u8 = 0;
const MATRIX_BT709: u8 = 1;

pub(super) struct Converter {
    context: Rc<libva::Context>,
    /// The captured picture, uploaded for the video processor to read.
    source: Surface<()>,
    format: libva::VAImageFormat,
    width: u32,
    height: u32,
    coded: (u32, u32),
}

impl Converter {
    /// A converter writing into `targets`, the encoder's pictures, or
    /// `None` when the node cannot.
    pub(super) fn open(
        display: &Arc<Display>,
        width: u32,
        height: u32,
        coded: (u32, u32),
        targets: &Vec<Surface<()>>,
    ) -> Option<Self> {
        let entrypoints = display
            .query_config_entrypoints(VAProfile::VAProfileNone)
            .ok()?;
        if !entrypoints.contains(&VAEntrypoint::VAEntrypointVideoProc) {
            return None;
        }
        let format = display
            .query_image_formats()
            .ok()?
            .into_iter()
            .find(|format| format.fourcc == libva::VA_FOURCC_BGRX)?;
        let config = display
            .create_config(
                vec![],
                VAProfile::VAProfileNone,
                VAEntrypoint::VAEntrypointVideoProc,
            )
            .ok()?;
        let context = display
            .create_context(&config, coded.0, coded.1, Some(targets), true)
            .ok()?;
        let source = display
            .create_surfaces(
                libva::VA_RT_FORMAT_RGB32,
                Some(libva::VA_FOURCC_BGRX),
                coded.0,
                coded.1,
                Some(libva::UsageHint::USAGE_HINT_VPP_READ),
                vec![()],
            )
            .ok()?
            .pop()?;
        Some(Self {
            context,
            source,
            format,
            width,
            height,
            coded,
        })
    }

    /// Converts `frame` into `target` and hands it back. On failure the
    /// target is lost with the error.
    pub(super) fn convert(
        &self,
        frame: &RawFrame,
        target: Surface<()>,
    ) -> Result<Surface<()>, Error> {
        let rejected = |_| Error::EncodeRejected("VA-API rejected the frame");
        if (u32::from(frame.width()), u32::from(frame.height())) != (self.width, self.height) {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        self.upload(frame)?;
        let parameters = ProcPipelineParameterBuffer::new(
            self.source.id(),
            None,
            STANDARD_SRGB,
            None,
            0xff00_0000,
            STANDARD_BT709,
            0,
            0,
            None,
            None,
            None,
            0,
            None,
            0,
            None,
            0,
            0,
            ProcColorProperties::new(
                0,
                libva::VA_SOURCE_RANGE_FULL as u8,
                PRIMARIES_BT709,
                TRANSFER_SRGB,
                MATRIX_RGB,
            ),
            ProcColorProperties::new(
                0,
                libva::VA_SOURCE_RANGE_REDUCED as u8,
                PRIMARIES_BT709,
                TRANSFER_BT709,
                MATRIX_BT709,
            ),
            0,
            None,
        );
        let buffer = self
            .context
            .create_buffer(BufferType::ProcPipelineParameter(parameters))
            .map_err(rejected)?;
        let mut picture = Picture::new(0, Rc::clone(&self.context), target);
        picture.add_buffer(buffer);
        let picture = picture.begin().map_err(rejected)?;
        let picture = picture.render().map_err(rejected)?;
        let picture = picture.end().map_err(rejected)?;
        let picture = picture
            .sync()
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
        picture
            .take_surface()
            .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))
    }

    /// Copies `frame` into the source surface, black past its edges: in
    /// place where the driver maps it, else through an image it copies back.
    fn upload(&self, frame: &RawFrame) -> Result<(), Error> {
        let rejected = |_| Error::EncodeRejected("VA-API rejected the frame");
        let mut image = match Image::derive_from(&self.source, self.coded) {
            Ok(image) => image,
            Err(_) => Image::create_from(&self.source, self.format, self.coded, self.coded)
                .map_err(rejected)?,
        };
        let descriptor = *image.image();
        let offset = descriptor.offsets[0] as usize;
        let pitch = descriptor.pitches[0] as usize;
        let row = self.width as usize * 4;
        let coded_row = self.coded.0 as usize * 4;
        if pitch < coded_row {
            return Err(Error::EncodeRejected("VA-API rejected the frame"));
        }
        let dest = image.as_mut();
        for y in 0..self.coded.1 as usize {
            let start = offset + y * pitch;
            let line = dest
                .get_mut(start..start + coded_row)
                .ok_or(Error::EncodeRejected("VA-API rejected the frame"))?;
            if y < self.height as usize {
                line[..row].copy_from_slice(frame.row(y as u16));
                line[row..].fill(0);
            } else {
                line.fill(0);
            }
        }
        Ok(())
    }
}
