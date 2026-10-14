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
    ) -> c_int;
}
#[derive(Debug)]
pub struct Error;
pub struct Encoder {
    owner: NonNull<c_void>,
    width: usize,
    height: usize,
}
// Kvazaar has no caller thread affinity. The C shim serializes all global dispatch access.
unsafe impl Send for Encoder {}
impl Encoder {
    pub fn open(
        width: u16,
        height: u16,
        fps: u8,
        bitrate: u32,
        threads: usize,
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
        let owner = NonNull::new(unsafe {
            vnl_hevc_encoder_open(
                width.into(),
                height.into(),
                fps.into(),
                bitrate as i32,
                threads as i32,
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
        // The shim borrows exactly pixels.len bytes and returns its own bounded buffer, valid until the next call.
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
            unsafe { std::slice::from_raw_parts(output, length) }.to_vec(),
        ))
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { vnl_hevc_encoder_close(self.owner.as_ptr()) }
    }
}
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub i420: Vec<u8>,
}
pub struct Decoder(NonNull<c_void>);
// libde265 forbids concurrent calls, not moves. No Sync, clones or shared native references.
unsafe impl Send for Decoder {}
impl Decoder {
    pub fn open() -> Result<Self, Error> {
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
        if unsafe {
            vnl_hevc_decode(
                self.0.as_ptr(),
                packet.as_ptr(),
                packet.len(),
                &mut output,
                &mut length,
                &mut width,
                &mut height,
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
            i420: unsafe { std::slice::from_raw_parts(output, length) }.to_vec(),
        })
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { vnl_hevc_decoder_close(self.0.as_ptr()) }
    }
}
