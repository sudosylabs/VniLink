// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A portal screen picture that stays on the GPU.
//!
//! PipeWire hands the compositor's buffers over as DMA-BUFs. [`Importer`]
//! imports each one into a VA-API surface (DRM PRIME, no GBM and no CPU
//! mapping) and has the video processor copy it into a picture of its own,
//! so the compositor can draw the next frame into its buffer at once. The
//! copy is a [`GpuPicture`], a [`NativePicture`]: the VA-API encoder
//! recognises it and imports the same memory into its own display, scaling
//! and converting it to NV12 on the GPU. Anything else that asks for its
//! pixels gets them read back once, packed.
//!
//! The pool is small. A picture counts as in use until the last frame
//! holding it is dropped, and a capture with every picture in use drops the
//! compositor's frame instead of waiting. The bytes are screen content: do
//! not log them.

use std::any::Any;
use std::collections::HashMap;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use libva::{
    BufferType, Display, ExternalBufferDescriptor, Image, MemoryType, Picture, ProcColorProperties,
    ProcPipelineParameterBuffer, Surface, UsageHint, VADRMPRIMESurfaceDescriptor, VAEntrypoint,
    VAProfile,
};
use vnilink_media::{Error, NativePicture};

/// `drm_fourcc.h`: `XR24` and `AR24`.
pub(crate) const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub(crate) const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");

/// `drm_fourcc.h` modifiers. Intel's tiled layouts read faster than linear
/// in the video processor (2.0 ms against 2.9 ms for 1080p on a UHD 770), so
/// they come first; linear is what every driver can import.
pub(crate) const MODIFIER_LINEAR: u64 = 0;
pub(crate) const MODIFIER_I915_X_TILED: u64 = (0x01 << 56) | 1;
pub(crate) const MODIFIER_I915_Y_TILED: u64 = (0x01 << 56) | 2;

/// What this side offers the compositor, best first.
pub(crate) const PREFERRED_MODIFIERS: [u64; 3] = [
    MODIFIER_I915_Y_TILED,
    MODIFIER_I915_X_TILED,
    MODIFIER_LINEAR,
];

/// Pictures a capture keeps: one being encoded, one held for a still
/// screen, and the compositor's next.
const POOL: usize = 4;

const REJECTED: Error = Error::CaptureRejected("VA-API rejected the screen picture");

/// The render node pictures are kept on: the GPU the compositor draws on
/// when the encoder the machine would open there is VA-API. NVENC reads
/// from CUDA memory this import does not reach, so a machine whose first
/// encoder is NVENC, or that has none, keeps pictures in shared memory.
pub(crate) fn target_node() -> Option<PathBuf> {
    first_vaapi_node(&crate::gpu::encoders(&crate::gpu::inventory()))
}

pub(crate) fn first_vaapi_node(choices: &[crate::gpu::EncoderChoice]) -> Option<PathBuf> {
    match choices.first()? {
        crate::gpu::EncoderChoice::Vaapi { render_node } => Some(render_node.clone()),
        crate::gpu::EncoderChoice::Nvenc { .. } => None,
    }
}

/// The modifier to use out of those the compositor offers: the first of
/// `PREFERRED_MODIFIERS` it offers.
pub(crate) fn choose_modifier(offered: &[u64]) -> Option<u64> {
    PREFERRED_MODIFIERS
        .into_iter()
        .find(|modifier| offered.contains(modifier))
}

/// Whether a buffer's damage regions (x, y, width, height) say nothing on
/// the screen changed: PipeWire ends the list with an empty rectangle, so a
/// list whose first entry is empty names no change.
pub(crate) fn damage_is_empty(regions: &[[i32; 4]]) -> bool {
    regions
        .first()
        .is_none_or(|region| region[2] <= 0 || region[3] <= 0)
}

/// A DMA-BUF in the form VA-API imports: DRM PRIME, one layer.
pub(crate) struct Prime {
    fds: Vec<OwnedFd>,
    sizes: Vec<u32>,
    width: u32,
    height: u32,
    fourcc: u32,
    modifier: u64,
    /// Per plane: the object it is in, its offset and its pitch.
    planes: Vec<(u32, u32, u32)>,
}

impl Prime {
    /// A buffer of `fds` (one per object), as PipeWire describes it. The
    /// descriptors are borrowed: each is duplicated.
    pub(crate) fn from_planes(
        buffers: &[(RawFd, u32, u32, u32)],
        width: u32,
        height: u32,
        fourcc: u32,
        modifier: u64,
    ) -> Result<Self, Error> {
        let mut fds = Vec::with_capacity(buffers.len());
        let mut sizes = Vec::with_capacity(buffers.len());
        let mut planes = Vec::with_capacity(buffers.len());
        for (index, (fd, size, offset, pitch)) in buffers.iter().enumerate() {
            // SAFETY: the descriptor is open for as long as the PipeWire
            // buffer is dequeued, which covers this call.
            let borrowed = unsafe { BorrowedFd::borrow_raw(*fd) };
            fds.push(borrowed.try_clone_to_owned().map_err(|_| REJECTED)?);
            sizes.push(*size);
            planes.push((index as u32, *offset, *pitch));
        }
        if fds.is_empty() || fds.len() > 4 {
            return Err(REJECTED);
        }
        Ok(Self {
            fds,
            sizes,
            width,
            height,
            fourcc,
            modifier,
            planes,
        })
    }

    /// Another handle on the same memory, for another display.
    pub(crate) fn duplicate(&self) -> Result<Self, Error> {
        Ok(Self {
            fds: self
                .fds
                .iter()
                .map(|fd| fd.try_clone().map_err(|_| REJECTED))
                .collect::<Result<_, _>>()?,
            sizes: self.sizes.clone(),
            width: self.width,
            height: self.height,
            fourcc: self.fourcc,
            modifier: self.modifier,
            planes: self.planes.clone(),
        })
    }

    /// The memory of one of this display's own surfaces.
    pub(crate) fn exported(surface: &Surface<()>) -> Result<Self, Error> {
        let descriptor = surface.export_prime().map_err(|_| REJECTED)?;
        let layer = descriptor.layers.first().ok_or(REJECTED)?;
        let planes = (0..layer.num_planes as usize)
            .map(|plane| {
                (
                    u32::from(layer.object_index[plane]),
                    layer.offset[plane],
                    layer.pitch[plane],
                )
            })
            .collect();
        let modifier = descriptor
            .objects
            .first()
            .ok_or(REJECTED)?
            .drm_format_modifier;
        let sizes = descriptor
            .objects
            .iter()
            .map(|object| object.size)
            .collect();
        Ok(Self {
            fds: descriptor
                .objects
                .into_iter()
                .map(|object| object.fd)
                .collect(),
            sizes,
            width: descriptor.width,
            height: descriptor.height,
            fourcc: layer.drm_format,
            modifier,
            planes,
        })
    }
}

impl ExternalBufferDescriptor for Prime {
    const MEMORY_TYPE: MemoryType = MemoryType::DrmPrime2;
    type DescriptorAttribute = VADRMPRIMESurfaceDescriptor;

    fn va_surface_attribute(&mut self) -> VADRMPRIMESurfaceDescriptor {
        let mut descriptor: VADRMPRIMESurfaceDescriptor = Default::default();
        descriptor.width = self.width;
        descriptor.height = self.height;
        descriptor.fourcc = libva::VA_FOURCC_BGRX;
        descriptor.num_objects = self.fds.len() as u32;
        for (index, fd) in self.fds.iter().enumerate() {
            descriptor.objects[index].fd = fd.as_raw_fd();
            descriptor.objects[index].size = self.sizes[index];
            descriptor.objects[index].drm_format_modifier = self.modifier;
        }
        descriptor.num_layers = 1;
        descriptor.layers[0].drm_format = self.fourcc;
        descriptor.layers[0].num_planes = self.planes.len() as u32;
        for (index, (object, offset, pitch)) in self.planes.iter().enumerate() {
            descriptor.layers[0].object_index[index] = *object;
            descriptor.layers[0].offset[index] = *offset;
            descriptor.layers[0].pitch[index] = *pitch;
        }
        descriptor
    }
}

/// Imports a DMA-BUF into `display` as a surface the video processor reads.
pub(crate) fn import(
    display: &Arc<Display>,
    prime: Prime,
    width: u32,
    height: u32,
) -> Result<Surface<Prime>, Error> {
    display
        .create_surfaces(
            libva::VA_RT_FORMAT_RGB32,
            Some(libva::VA_FOURCC_BGRX),
            width,
            height,
            Some(UsageHint::USAGE_HINT_VPP_READ),
            vec![prime],
        )
        .map_err(|_| REJECTED)?
        .pop()
        .ok_or(REJECTED)
}

static NEXT_PICTURE: AtomicU64 = AtomicU64::new(1);

/// One picture of the pool: memory the capture's video processor writes and
/// the encoder's display imports.
struct Slot {
    /// Unique among every pool, so an encoder's import of it is never taken
    /// for another's after a capture restarts.
    id: u64,
    surface: Mutex<Option<Surface<()>>>,
    prime: Prime,
    in_use: AtomicBool,
}

/// A screen picture in GPU memory, as a frame's native pixels.
pub(crate) struct GpuPicture {
    slot: Arc<Slot>,
    /// Serialises this picture's display between the capture thread and a
    /// reader of its pixels.
    display_lock: Arc<Mutex<()>>,
    display: Arc<Display>,
    node: PathBuf,
    size: (u16, u16),
}

impl GpuPicture {
    /// Identifies this picture's memory, for an importer's cache.
    pub(crate) fn id(&self) -> u64 {
        self.slot.id
    }

    /// The render node the picture lives on.
    pub(crate) fn node(&self) -> &Path {
        &self.node
    }

    /// The picture's memory in a form another display imports.
    pub(crate) fn share(&self) -> Result<Prime, Error> {
        self.slot.prime.duplicate()
    }
}

impl Drop for GpuPicture {
    fn drop(&mut self) {
        self.slot.in_use.store(false, Ordering::Release);
    }
}

impl NativePicture for GpuPicture {
    fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The picture as packed BGRA, copied out of GPU memory.
    fn read_back(&self) -> Result<Vec<u8>, Error> {
        let _display = self.display_lock.lock().unwrap_or_else(|e| e.into_inner());
        let guard = self.slot.surface.lock().unwrap_or_else(|e| e.into_inner());
        let surface = guard.as_ref().ok_or(REJECTED)?;
        let (width, height) = (usize::from(self.size.0), usize::from(self.size.1));
        let coded = (width as u32, height as u32);
        let format = self
            .display
            .query_image_formats()
            .map_err(|_| REJECTED)?
            .into_iter()
            .find(|format| format.fourcc == libva::VA_FOURCC_BGRX)
            .ok_or(REJECTED)?;
        let mut image = match Image::derive_from(surface, coded) {
            Ok(image) => image,
            Err(_) => Image::create_from(surface, format, coded, coded).map_err(|_| REJECTED)?,
        };
        let descriptor = *image.image();
        let offset = descriptor.offsets[0] as usize;
        let pitch = descriptor.pitches[0] as usize;
        let source = image.as_mut();
        let mut bytes = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let start = offset + y * pitch;
            bytes.extend_from_slice(source.get(start..start + width * 4).ok_or(REJECTED)?);
        }
        // The compositor's X in BGRX is unspecified.
        for pixel in bytes.chunks_exact_mut(4) {
            pixel[3] = 0xff;
        }
        Ok(bytes)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The capture side: imports PipeWire's DMA-BUFs and copies them into
/// pictures. Not [`Send`]: libva's context is the PipeWire thread's.
pub(crate) struct Importer {
    display: Arc<Display>,
    display_lock: Arc<Mutex<()>>,
    context: Rc<libva::Context>,
    node: PathBuf,
    size: (u32, u32),
    slots: Vec<Arc<Slot>>,
    /// The compositor's buffers imported so far, by what names them.
    imported: HashMap<u64, Surface<Prime>>,
}

impl Importer {
    /// An importer on the render node at `node` for pictures of `size`, or
    /// an error when the node cannot import and process them.
    pub(crate) fn open(node: &Path, size: (u16, u16)) -> Result<Self, Error> {
        let display = Display::open_drm_display(node).map_err(|_| REJECTED)?;
        let entrypoints = display
            .query_config_entrypoints(VAProfile::VAProfileNone)
            .map_err(|_| REJECTED)?;
        if !entrypoints.contains(&VAEntrypoint::VAEntrypointVideoProc) {
            return Err(REJECTED);
        }
        let config = display
            .create_config(
                vec![],
                VAProfile::VAProfileNone,
                VAEntrypoint::VAEntrypointVideoProc,
            )
            .map_err(|_| REJECTED)?;
        let (width, height) = (u32::from(size.0), u32::from(size.1));
        let surfaces = display
            .create_surfaces(
                libva::VA_RT_FORMAT_RGB32,
                Some(libva::VA_FOURCC_BGRX),
                width,
                height,
                Some(UsageHint::USAGE_HINT_VPP_READ | UsageHint::USAGE_HINT_VPP_WRITE),
                vec![(); POOL],
            )
            .map_err(|_| REJECTED)?;
        let context = display
            .create_context(&config, width, height, Some(&surfaces), true)
            .map_err(|_| REJECTED)?;
        let slots = surfaces
            .into_iter()
            .map(|surface| {
                Ok(Arc::new(Slot {
                    id: NEXT_PICTURE.fetch_add(1, Ordering::Relaxed),
                    prime: Prime::exported(&surface)?,
                    surface: Mutex::new(Some(surface)),
                    in_use: AtomicBool::new(false),
                }))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            display,
            display_lock: Arc::new(Mutex::new(())),
            context,
            node: node.to_path_buf(),
            size: (width, height),
            slots,
            imported: HashMap::new(),
        })
    }

    /// The compositor's buffer `key` (its inode), copied into a picture;
    /// `None` when every picture is in use. `prime` is made when the buffer
    /// has not been imported before.
    pub(crate) fn copy(
        &mut self,
        key: u64,
        prime: impl FnOnce() -> Result<Prime, Error>,
    ) -> Result<Option<GpuPicture>, Error> {
        let _display = self.display_lock.lock().unwrap_or_else(|e| e.into_inner());
        if !self.imported.contains_key(&key) {
            let surface = import(&self.display, prime()?, self.size.0, self.size.1)?;
            self.imported.insert(key, surface);
        }
        let source = self.imported.get(&key).ok_or(REJECTED)?;
        let Some(slot) = self.slots.iter().find(|slot| {
            slot.in_use
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        }) else {
            return Ok(None);
        };
        let slot = Arc::clone(slot);
        let target = slot
            .surface
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(REJECTED)?;
        let copied = self.process(source.id(), target);
        match copied {
            Ok(target) => {
                *slot.surface.lock().unwrap_or_else(|e| e.into_inner()) = Some(target);
            }
            Err(error) => {
                // The surface went with the failure: the slot stays unusable.
                return Err(error);
            }
        }
        Ok(Some(GpuPicture {
            slot,
            display_lock: Arc::clone(&self.display_lock),
            display: Arc::clone(&self.display),
            node: self.node.clone(),
            size: (self.size.0 as u16, self.size.1 as u16),
        }))
    }

    /// The video processor's copy of `source` into `target`, colour unchanged.
    fn process(
        &self,
        source: libva::VASurfaceID,
        target: Surface<()>,
    ) -> Result<Surface<()>, Error> {
        let properties =
            || ProcColorProperties::new(0, libva::VA_SOURCE_RANGE_FULL as u8, 1, 13, 0);
        let parameters = ProcPipelineParameterBuffer::new(
            source,
            None,
            libva::_VAProcColorStandardType_VAProcColorStandardSRGB as u8,
            None,
            0xff00_0000,
            libva::_VAProcColorStandardType_VAProcColorStandardSRGB as u8,
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
            properties(),
            properties(),
            0,
            None,
        );
        let buffer = self
            .context
            .create_buffer(BufferType::ProcPipelineParameter(parameters))
            .map_err(|_| REJECTED)?;
        let mut picture = Picture::new(0, Rc::clone(&self.context), target);
        picture.add_buffer(buffer);
        let picture = picture
            .begin()
            .and_then(|picture| picture.render())
            .and_then(|picture| picture.end())
            .map_err(|_| REJECTED)?;
        let picture = picture.sync().map_err(|_| REJECTED)?;
        picture.take_surface().map_err(|_| REJECTED)
    }
}
