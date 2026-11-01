// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Portable HEVC owners. Each context is accessed exclusively through `&mut self`.
use std::{
    ffi::{c_int, c_void},
    ptr::NonNull,
};
unsafe extern "C" {
    fn vnl_hevc_encoder_open(
        w: c_int,
        h: c_int,
        fps: c_int,
        bitrate: c_int,
        threads: c_int,
        primaries: c_int,
        transfer: c_int,
        matrix: c_int,
        full_range: c_int,
    ) -> *mut c_void;
    fn vnl_hevc_encoder_close(owner: *mut c_void);
    fn vnl_hevc_encode(
        owner: *mut c_void,
        pixels: *const u8,
        length: usize,
        force: c_int,
        output: *mut *const u8,
        length_out: *mut usize,
        key: *mut c_int,
    ) -> c_int;
    fn vnl_hevc_decoder_open() -> *mut c_void;
    fn vnl_hevc_decoder_close(owner: *mut c_void);
    fn vnl_hevc_decode(
        owner: *mut c_void,
        packet: *const u8,
        length: usize,
        output: *mut *const u8,
        length_out: *mut usize,
        width: *mut c_int,
        height: *mut c_int,
        matrix: *mut c_int,
        full_range: *mut c_int,
    ) -> c_int;
}
/// The colour an encoder declares in its SPS VUI, as ITU-T H.273 codes it.
/// The caller converts pictures to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    pub primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}
#[derive(Debug)]
pub struct Error;
pub struct Encoder {
    owner: NonNull<c_void>,
    width: usize,
    height: usize,
}
// Kvazaar has no caller thread affinity. The C shim serializes all global dispatch access.
// SAFETY: the owner pointer is exclusively held by this value and the shim keeps no thread-local state, so moving it to another thread is sound.
unsafe impl Send for Encoder {}
impl Encoder {
    pub fn open(
        width: u16,
        height: u16,
        fps: u8,
        bitrate: u32,
        threads: usize,
        colour: Colour,
    ) -> Result<Self, Error> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || width % 2 != 0
            || height % 2 != 0
            || fps == 0
            || fps > 60
            || bitrate == 0
            || bitrate > i32::MAX as u32
            || !(1..=8).contains(&threads)
        {
            return Err(Error);
        }
        // SAFETY: the arguments are plain integers already range-checked above; the shim returns null on failure or a fresh owner this value takes sole ownership of.
        let owner = NonNull::new(unsafe {
            vnl_hevc_encoder_open(
                width.into(),
                height.into(),
                fps.into(),
                bitrate as i32,
                threads as i32,
                colour.primaries.into(),
                colour.transfer.into(),
                colour.matrix.into(),
                colour.full_range.into(),
            )
        })
        .ok_or(Error)?;
        Ok(Self {
            owner,
            width: width.into(),
            height: height.into(),
        })
    }
    pub fn encode(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        force: bool,
    ) -> Result<(bool, Vec<u8>), Error> {
        let luma = self.width * self.height;
        if y.len() != luma || u.len() != luma / 4 || v.len() != luma / 4 {
            return Err(Error);
        }
        let mut pixels = Vec::with_capacity(luma * 3 / 2);
        pixels.extend_from_slice(y);
        pixels.extend_from_slice(u);
        pixels.extend_from_slice(v);
        let mut output = std::ptr::null();
        let mut length = 0;
        let mut key = 0;
        // SAFETY: `owner` is live and exclusively borrowed through `&mut self`; `pixels` is a live buffer of `pixels.len()` bytes the shim only reads; the three out-pointers are valid locals. On success `output` points to `length` bytes owned by the shim and valid until the next call on this encoder.
        if unsafe {
            vnl_hevc_encode(
                self.owner.as_ptr(),
                pixels.as_ptr(),
                pixels.len(),
                force.into(),
                &mut output,
                &mut length,
                &mut key,
            )
        } != 1
            || output.is_null()
            || length == 0
            || length > 1024 * 1024
        {
            return Err(Error);
        }
        Ok((
            key != 0,
            // SAFETY: the shim returned `output` non-null with `length` initialised bytes (checked above), and no other call on this encoder happens before the copy.
            unsafe { std::slice::from_raw_parts(output, length) }.to_vec(),
        ))
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: `owner` came from `vnl_hevc_encoder_open` and is closed exactly once, here.
        unsafe { vnl_hevc_encoder_close(self.owner.as_ptr()) }
    }
}
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub i420: Vec<u8>,
    /// The matrix the SPS VUI declares (H.273; 2 when it declares none).
    pub matrix: u8,
    pub full_range: bool,
}
pub struct Decoder(NonNull<c_void>);
// libde265 forbids concurrent calls, not moves. No Sync, clones or shared native references.
// SAFETY: the owner pointer is exclusively held by this value; libde265 forbids concurrent calls on one context, not moves between threads.
unsafe impl Send for Decoder {}
impl Decoder {
    pub fn open() -> Result<Self, Error> {
        // SAFETY: takes no arguments; the shim returns null on failure or a fresh owner this value takes sole ownership of.
        NonNull::new(unsafe { vnl_hevc_decoder_open() })
            .map(Self)
            .ok_or(Error)
    }
    pub fn decode(&mut self, packet: &[u8]) -> Result<Picture, Error> {
        if packet.is_empty() || packet.len() > 1024 * 1024 {
            return Err(Error);
        }
        let mut output = std::ptr::null();
        let mut length = 0;
        let mut width = 0;
        let mut height = 0;
        let mut matrix = 0;
        let mut full_range = 0;
        // SAFETY: `self.0` is live and exclusively borrowed through `&mut self`; `packet` is a live buffer of `packet.len()` bytes the shim only reads; the out-pointers are valid locals. On success `output` points to `length` bytes owned by the shim and valid until the next call on this decoder.
        if unsafe {
            vnl_hevc_decode(
                self.0.as_ptr(),
                packet.as_ptr(),
                packet.len(),
                &mut output,
                &mut length,
                &mut width,
                &mut height,
                &mut matrix,
                &mut full_range,
            )
        } != 1
            || output.is_null()
            || width <= 0
            || height <= 0
            || width > 4096
            || height > 4096
            || width % 2 != 0
            || height % 2 != 0
            || length != (width as usize) * (height as usize) * 3 / 2
        {
            return Err(Error);
        }
        Ok(Picture {
            width: width as usize,
            height: height as usize,
            // SAFETY: the shim returned `output` non-null with `length` bytes (checked above), and no other call on this decoder happens before the copy.
            i420: unsafe { std::slice::from_raw_parts(output, length) }.to_vec(),
            matrix: u8::try_from(matrix).unwrap_or(2),
            full_range: full_range != 0,
        })
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `vnl_hevc_decoder_open` and is closed exactly once, here.
        unsafe { vnl_hevc_decoder_close(self.0.as_ptr()) }
    }
}
