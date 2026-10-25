//! BGRA to NV12 on the GPU, for the encoder on the same node.
//!
//! The video processor writes the stream's colour (the session's matrix,
//! limited range: [`vnilink_media::VideoSignal::stream`]) straight into the
//! encoder's picture, so the host does not convert on the CPU. Capture is sRGB, full
//! range. A node without a video processor, or one that refuses BGRX, keeps
//! the CPU conversion.
//!
//! A picture from memory is uploaded padded black to the encoder's coded
//! size, so the processor maps whole surface to whole surface. A screen
//! picture that stays on the GPU ([`GpuPicture`]) is imported as it is and
//! scaled into the encoder's picture, a region of its coded size, with the
//! processor's high-quality filter, black around it; that needs the pipeline
//! buffer's rectangles to outlive its creation, which vendor/libva ensures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use libva::{
    BufferType, Display, Image, Picture, ProcColorProperties, ProcPipelineParameterBuffer, Surface,
    VAEntrypoint, VAProfile, VARectangle,
};
use vnilink::YuvMatrix;
use vnilink_media::{scale_bgra, Error, RawFrame};

use crate::gpu_picture::{self, GpuPicture, Prime};

/// `VA_FILTER_SCALING_HQ`: the processor's high-quality scaler.
const FILTER_SCALING_HQ: u32 = 0x0000_0200;

/// `VAProcColorStandardType` in `va_vpp.h`.
const STANDARD_BT709: u8 = libva::_VAProcColorStandardType_VAProcColorStandardBT709 as u8;
const STANDARD_BT601: u8 = libva::_VAProcColorStandardType_VAProcColorStandardBT601 as u8;
const STANDARD_SRGB: u8 = libva::_VAProcColorStandardType_VAProcColorStandardSRGB as u8;
/// H.273 code points: BT.709 primaries, the sRGB and BT.709 transfers. RGB
/// has the identity matrix; the output matrix is the session's
/// ([`YuvMatrix::code`]).
const PRIMARIES_BT709: u8 = 1;
const TRANSFER_SRGB: u8 = 13;
const TRANSFER_BT709: u8 = 1;
const MATRIX_RGB: u8 = 0;

pub(super) struct Converter {
    display: Arc<Display>,
    node: PathBuf,
    /// Screen pictures imported so far, by the picture's identity.
    imported: RefCell<HashMap<u64, Surface<Prime>>>,
    context: Rc<libva::Context>,
    /// The captured picture, uploaded for the video processor to read.
    source: Surface<()>,
    format: libva::VAImageFormat,
    width: u32,
    height: u32,
    coded: (u32, u32),
    /// The matrix written into the encoder's pictures.
    matrix: YuvMatrix,
}

impl Converter {
    /// A converter writing into `targets`, the encoder's pictures, or
    /// `None` when the node cannot.
    pub(super) fn open(
        display: &Arc<Display>,
        node: &Path,
        width: u32,
        height: u32,
        coded: (u32, u32),
        targets: &Vec<Surface<()>>,
        matrix: YuvMatrix,
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
            display: Arc::clone(display),
            node: node.to_path_buf(),
            imported: RefCell::new(HashMap::new()),
            context,
            source,
            format,
            width,
            height,
            coded,
            matrix,
        })
    }

    /// Converts `frame` into `target` and hands it back. On failure the
    /// target is lost with the error.
    pub(super) fn convert(
        &self,
        frame: &RawFrame,
        target: Surface<()>,
    ) -> Result<Surface<()>, Error> {
        if let Some(picture) = frame
            .native_picture()
            .and_then(|native| native.as_any().downcast_ref::<GpuPicture>())
        {
            if picture.node() == self.node.as_path() {
                return self.convert_picture(picture, target);
            }
        }
        let scaled;
        let frame = if frame.native_picture().is_some()
            && (u32::from(frame.width()), u32::from(frame.height())) != (self.width, self.height)
        {
            // A picture the pump left at its own size for a processor that
            // is not on its GPU: read it back and scale it here.
            scaled = scale_bgra(frame, self.width as u16, self.height as u16)?;
            &scaled
        } else {
            frame
        };
        if (u32::from(frame.width()), u32::from(frame.height())) != (self.width, self.height) {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        self.upload(frame)?;
        self.process(self.source.id(), target, None, 0)
    }

    /// Scales `picture`, in the GPU memory it already is in, into `target`'s
    /// visible part.
    fn convert_picture(
        &self,
        picture: &GpuPicture,
        target: Surface<()>,
    ) -> Result<Surface<()>, Error> {
        let (width, height) = vnilink_media::NativePicture::size(picture);
        let source = {
            let mut imported = self.imported.borrow_mut();
            if !imported.contains_key(&picture.id()) {
                // Keep to the pictures the capture holds now.
                if imported.len() >= 16 {
                    imported.clear();
                }
                let surface = gpu_picture::import(
                    &self.display,
                    picture.share()?,
                    u32::from(width),
                    u32::from(height),
                )
                .map_err(|_| Error::EncodeRejected("VA-API rejected the frame"))?;
                imported.insert(picture.id(), surface);
            }
            imported
                .get(&picture.id())
                .map(Surface::id)
                .ok_or(Error::EncodeRejected("VA-API rejected the frame"))?
        };
        let region = VARectangle {
            x: 0,
            y: 0,
            width: self.width as u16,
            height: self.height as u16,
        };
        let filter = if (u32::from(width), u32::from(height)) == (self.width, self.height) {
            0
        } else {
            FILTER_SCALING_HQ
        };
        self.process(source, target, Some(region), filter)
    }

    /// The processor's conversion of `source` into `target`: sRGB, full range
    /// in; the session's matrix, limited range out, into `region` when there
    /// is one.
    fn process(
        &self,
        source: libva::VASurfaceID,
        target: Surface<()>,
        region: Option<VARectangle>,
        filter_flags: u32,
    ) -> Result<Surface<()>, Error> {
        let rejected = |_| Error::EncodeRejected("VA-API rejected the frame");
        let parameters = ProcPipelineParameterBuffer::new(
            source,
            None,
            STANDARD_SRGB,
            region,
            0xff00_0000,
            match self.matrix {
                YuvMatrix::Bt709 => STANDARD_BT709,
                YuvMatrix::Bt601 => STANDARD_BT601,
            },
            0,
            filter_flags,
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
                self.matrix.code(),
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
