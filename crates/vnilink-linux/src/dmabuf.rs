// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Import a portal DMA-BUF and copy it to tightly packed BGRA or BGRx.
//!
//! GBM detiles the buffer. The bytes are screen content: do not log them.
//! The PipeWire buffer keeps ownership of the file descriptors.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::raw::{c_int, c_ulong};
use std::path::PathBuf;
use std::ptr;

pub(crate) const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub(crate) const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
pub(crate) const DRM_FORMAT_MOD_LINEAR: u64 = 0;
pub(crate) const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const GBM_BO_USE_RENDERING: u32 = 1 << 2;
#[cfg(test)]
const GBM_BO_USE_LINEAR: u32 = 1 << 4;
const GBM_BO_TRANSFER_READ: u32 = 1 << 0;
#[cfg(test)]
const GBM_BO_TRANSFER_WRITE: u32 = 1 << 1;
const GBM_BO_IMPORT_FD: u32 = 0x5503;
const GBM_BO_IMPORT_FD_MODIFIER: u32 = 0x5504;
const GBM_MAX_PLANES: usize = 4;
const EGL_PLATFORM_GBM_MESA: u32 = 0x31D7;
/// `DMA_BUF_IOCTL_SYNC` from `linux/dma-buf.h`.
const DMA_BUF_IOCTL_SYNC: c_ulong = 0x4008_6200;
const DMA_BUF_SYNC_READ: u64 = 1;
const DMA_BUF_SYNC_END: u64 = 1 << 2;

#[repr(C)]
struct ImportFd {
    fd: c_int,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

#[repr(C)]
struct ImportModifier {
    width: u32,
    height: u32,
    format: u32,
    num_fds: u32,
    fds: [c_int; GBM_MAX_PLANES],
    strides: [c_int; GBM_MAX_PLANES],
    offsets: [c_int; GBM_MAX_PLANES],
    modifier: u64,
}

#[repr(C)]
struct DmaBufSync {
    flags: u64,
}

#[link(name = "gbm")]
unsafe extern "C" {
    fn gbm_create_device(fd: c_int) -> *mut c_void;
    fn gbm_device_destroy(gbm: *mut c_void);
    #[cfg(test)]
    fn gbm_bo_create(
        gbm: *mut c_void,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut c_void;
    fn gbm_bo_import(gbm: *mut c_void, kind: u32, buffer: *mut c_void, usage: u32) -> *mut c_void;
    fn gbm_bo_map(
        bo: *mut c_void,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        flags: u32,
        stride: *mut u32,
        map_data: *mut *mut c_void,
    ) -> *mut c_void;
    fn gbm_bo_unmap(bo: *mut c_void, map_data: *mut c_void);
    fn gbm_bo_destroy(bo: *mut c_void);
    #[cfg(test)]
    fn gbm_bo_get_fd(bo: *mut c_void) -> c_int;
    #[cfg(test)]
    fn gbm_bo_get_modifier(bo: *mut c_void) -> u64;
}

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetPlatformDisplay(
        platform: u32,
        native_display: *mut c_void,
        attrib_list: *const isize,
    ) -> *mut c_void;
    fn eglInitialize(display: *mut c_void, major: *mut c_int, minor: *mut c_int) -> u32;
    fn eglTerminate(display: *mut c_void) -> u32;
    fn eglGetProcAddress(name: *const i8) -> *mut c_void;
}

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, argument: *mut DmaBufSync) -> c_int;
}

type QueryModifiers = unsafe extern "C" fn(
    display: *mut c_void,
    format: c_int,
    max_modifiers: c_int,
    modifiers: *mut u64,
    external_only: *mut u32,
    count: *mut c_int,
) -> u32;

pub(crate) struct Plane {
    pub fd: c_int,
    pub stride: c_int,
    pub offset: c_int,
}

struct Device {
    ptr: *mut c_void,
    _fd: OwnedFd,
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { gbm_device_destroy(self.ptr) }
    }
}

/// Render nodes kept open so a later frame can import on the same GPU.
pub(crate) struct Importer {
    devices: Vec<Device>,
    preferred: Option<usize>,
}

impl Importer {
    pub(crate) fn new() -> Self {
        Self {
            devices: Vec::new(),
            preferred: None,
        }
    }

    /// Modifiers this machine can import, without `DRM_FORMAT_MOD_INVALID`.
    ///
    /// The caller adds the invalid modifier itself. An empty list means no
    /// render node answered, and the stream still offers linear buffers.
    pub(crate) fn modifiers(&mut self) -> Vec<u64> {
        self.ensure();
        let mut found = Vec::new();
        for device in &self.devices {
            query_modifiers(device.ptr, &mut found);
        }
        found
    }

    /// Copy one frame. `explicit` means the format named `modifier`.
    pub(crate) fn map(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        explicit: bool,
        planes: &[Plane],
    ) -> Option<(Vec<u8>, usize)> {
        self.ensure();
        if self.devices.is_empty() || planes.is_empty() || width == 0 || height == 0 {
            return None;
        }
        if planes.len() > GBM_MAX_PLANES || planes.iter().any(|plane| plane.fd < 0) {
            return None;
        }
        let mut order: Vec<usize> = Vec::with_capacity(self.devices.len());
        if let Some(preferred) = self.preferred {
            order.push(preferred);
        }
        order.extend((0..self.devices.len()).filter(|index| Some(*index) != self.preferred));
        for index in order {
            let device = self.devices[index].ptr;
            if let Some(copied) =
                import_one(device, width, height, format, modifier, explicit, planes)
            {
                self.preferred = Some(index);
                return Some(copied);
            }
        }
        None
    }

    fn ensure(&mut self) {
        if !self.devices.is_empty() {
            return;
        }
        for path in render_nodes() {
            let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
                continue;
            };
            let ptr = unsafe { gbm_create_device(file.as_raw_fd()) };
            if ptr.is_null() {
                continue;
            }
            self.devices.push(Device {
                ptr,
                _fd: OwnedFd::from(file),
            });
        }
    }
}

fn render_nodes() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Ok(entries) = std::fs::read_dir("/dev/dri") else {
        return paths;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("renderD") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    paths
}

fn query_modifiers(device: *mut c_void, found: &mut Vec<u64>) {
    unsafe {
        let display = eglGetPlatformDisplay(EGL_PLATFORM_GBM_MESA, device, ptr::null());
        if display.is_null() {
            return;
        }
        if eglInitialize(display, ptr::null_mut(), ptr::null_mut()) == 0 {
            return;
        }
        let query = eglGetProcAddress(c"eglQueryDmaBufModifiersEXT".as_ptr().cast::<i8>());
        if !query.is_null() {
            let query: QueryModifiers = std::mem::transmute(query);
            for format in [DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888] {
                append_modifiers(query, display, format, found);
            }
        }
        eglTerminate(display);
    }
}

fn append_modifiers(
    query: QueryModifiers,
    display: *mut c_void,
    format: u32,
    found: &mut Vec<u64>,
) {
    unsafe {
        let mut count = 0;
        if query(
            display,
            format as c_int,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut count,
        ) == 0
            || count <= 0
        {
            return;
        }
        let mut count = count.min(128);
        let mut modifiers = vec![0u64; count as usize];
        let mut external = vec![0u32; count as usize];
        if query(
            display,
            format as c_int,
            count,
            modifiers.as_mut_ptr(),
            external.as_mut_ptr(),
            &mut count,
        ) == 0
        {
            return;
        }
        let count = (count.max(0) as usize).min(modifiers.len());
        for modifier in modifiers.into_iter().take(count) {
            if modifier != DRM_FORMAT_MOD_INVALID && !found.contains(&modifier) && found.len() < 126
            {
                found.push(modifier);
            }
        }
    }
}

fn import_one(
    device: *mut c_void,
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
    explicit: bool,
    planes: &[Plane],
) -> Option<(Vec<u8>, usize)> {
    for plane in planes {
        sync_fd(plane.fd, false);
    }
    let bo = unsafe {
        if explicit && modifier != DRM_FORMAT_MOD_INVALID {
            let mut data = ImportModifier {
                width,
                height,
                format,
                num_fds: planes.len() as u32,
                fds: [-1; GBM_MAX_PLANES],
                strides: [0; GBM_MAX_PLANES],
                offsets: [0; GBM_MAX_PLANES],
                modifier,
            };
            for (index, plane) in planes.iter().enumerate() {
                data.fds[index] = plane.fd;
                data.strides[index] = if plane.stride == 0 && index == 0 {
                    width.saturating_mul(4) as c_int
                } else {
                    plane.stride
                };
                data.offsets[index] = plane.offset;
            }
            gbm_bo_import(
                device,
                GBM_BO_IMPORT_FD_MODIFIER,
                (&mut data as *mut ImportModifier).cast(),
                GBM_BO_USE_RENDERING,
            )
        } else {
            let stride = if planes[0].stride > 0 {
                planes[0].stride as u32
            } else {
                width.saturating_mul(4)
            };
            let mut data = ImportFd {
                fd: planes[0].fd,
                width,
                height,
                stride,
                format,
            };
            gbm_bo_import(
                device,
                GBM_BO_IMPORT_FD,
                (&mut data as *mut ImportFd).cast(),
                GBM_BO_USE_RENDERING,
            )
        }
    };
    if bo.is_null() {
        for plane in planes {
            sync_fd(plane.fd, true);
        }
        return None;
    }
    let mut stride = 0u32;
    let mut map_data = ptr::null_mut();
    let mapped = unsafe {
        gbm_bo_map(
            bo,
            0,
            0,
            width,
            height,
            GBM_BO_TRANSFER_READ,
            &mut stride,
            &mut map_data,
        )
    };
    let copied = copy_mapped(mapped, stride, width, height);
    unsafe {
        if !mapped.is_null() {
            gbm_bo_unmap(bo, map_data);
        }
        gbm_bo_destroy(bo);
    }
    for plane in planes {
        sync_fd(plane.fd, true);
    }
    copied
}

fn copy_mapped(
    mapped: *mut c_void,
    stride: u32,
    width: u32,
    height: u32,
) -> Option<(Vec<u8>, usize)> {
    if mapped.is_null() {
        return None;
    }
    let stride = stride as usize;
    let row = (width as usize).checked_mul(4)?;
    let rows = height as usize;
    if stride < row || rows > usize::MAX / stride.max(1) {
        return None;
    }
    let mut packed = vec![0u8; row.checked_mul(rows)?];
    for y in 0..rows {
        let src = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>().add(y * stride), row) };
        packed[y * row..(y + 1) * row].copy_from_slice(src);
    }
    Some((packed, row))
}

fn sync_fd(fd: c_int, end: bool) {
    if fd < 0 {
        return;
    }
    let mut sync = DmaBufSync {
        flags: DMA_BUF_SYNC_READ | if end { DMA_BUF_SYNC_END } else { 0 },
    };
    unsafe {
        ioctl(fd, DMA_BUF_IOCTL_SYNC, &mut sync);
    }
}

#[cfg(test)]
#[path = "tests/dmabuf.rs"]
mod dmabuf_tests;
