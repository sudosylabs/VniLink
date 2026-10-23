//! NVENC encode on NVIDIA GPUs: H.264, H.265 Main and AV1 Main, 8-bit
//! 4:2:0, where the GPU has the codec.
//!
//! The encoder and CUDA come with NVIDIA's driver; both are loaded at run
//! time, so a machine without them only reports NVENC missing. Sessions
//! run on the first GPU's primary CUDA context, which is made current around
//! every call. Pictures are uploaded as NV12 from the same conversion the
//! VA-API encoder uses, so colour does not depend on the encoder. Each
//! picture is encoded synchronously, without B pictures or lookahead, and
//! a keyframe carries its parameter sets or sequence header.
//!
//! While any session is open, a small anchor session opened before it stays
//! open too ([`Anchor`]). NVIDIA's driver (seen on 595) starts one thread per
//! process that recalibrates the GPU clock every 10 s through the device of
//! the first session initialized, and keeps that device until the last
//! session closes, even when its own session closed long before. A host
//! reopens its encoder by opening the new one first, so without the anchor
//! the thread soon reads a freed device. Where the allocator keeps freed
//! memory mapped that goes unnoticed; Electron's allocator unmaps it, and
//! the engine crashed about a minute into a stream.

#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unnecessary_transmutes,
    unused_imports,
    clippy::all,
    unsafe_op_in_unsafe_fn
)]
mod bindings;
mod guid;

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use bindings::*;
use vnilink::VideoCodec;
use vnilink_media::{hevc_sample, EncodedSample, Error};

/// The API this module is written against: 12.1, drivers from 530.
const API_VERSION: u32 = NVENCAPI_MAJOR_VERSION | (NVENCAPI_MINOR_VERSION << 24);

/// `NVENCAPI_STRUCT_VERSION(version)`, with the high bit some structs add.
const fn struct_version(version: u32, high: bool) -> u32 {
    API_VERSION | (version << 16) | (0x7 << 28) | if high { 1 << 31 } else { 0 }
}

/// Pictures between keyframes, as the VA-API encoder bounds recovery.
const KEYFRAME_INTERVAL: u32 = 120;

type CuContext = *mut c_void;

/// The driver's NVENC entry points and the CUDA context sessions use.
struct Api {
    functions: NV_ENCODE_API_FUNCTION_LIST,
    context: CuContext,
    push: unsafe extern "C" fn(CuContext) -> i32,
    pop: unsafe extern "C" fn(*mut CuContext) -> i32,
    // The libraries stay loaded for the process: function pointers above
    // point into them.
    _cuda: libloading::Library,
    _nvenc: libloading::Library,
}

// SAFETY: the function list and context are immutable after loading.
// NVENC and CUDA allow calls from any thread; a call that needs the context
// pushes it on its own thread first ([`Current`]).
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe { load() }).as_ref()
}

/// # Safety
/// Calls into libraries named by NVIDIA's driver with the signatures their
/// headers declare; nothing else is resolved from them.
unsafe fn load() -> Option<Api> {
    let cuda = libloading::Library::new("libcuda.so.1").ok()?;
    let nvenc = libloading::Library::new("libnvidia-encode.so.1").ok()?;
    let init = *cuda
        .get::<unsafe extern "C" fn(u32) -> i32>(b"cuInit\0")
        .ok()?;
    let device_get = *cuda
        .get::<unsafe extern "C" fn(*mut i32, i32) -> i32>(b"cuDeviceGet\0")
        .ok()?;
    let retain = *cuda
        .get::<unsafe extern "C" fn(*mut CuContext, i32) -> i32>(b"cuDevicePrimaryCtxRetain\0")
        .ok()?;
    let push = *cuda
        .get::<unsafe extern "C" fn(CuContext) -> i32>(b"cuCtxPushCurrent_v2\0")
        .ok()?;
    let pop = *cuda
        .get::<unsafe extern "C" fn(*mut CuContext) -> i32>(b"cuCtxPopCurrent_v2\0")
        .ok()?;
    let max_version = *nvenc
        .get::<unsafe extern "C" fn(*mut u32) -> NVENCSTATUS>(
            b"NvEncodeAPIGetMaxSupportedVersion\0",
        )
        .ok()?;
    let create = *nvenc
        .get::<unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS>(
            b"NvEncodeAPICreateInstance\0",
        )
        .ok()?;
    let mut supported = 0;
    if max_version(&mut supported) != NV_ENC_SUCCESS
        || supported < (NVENCAPI_MAJOR_VERSION << 4 | NVENCAPI_MINOR_VERSION)
    {
        return None;
    }
    let mut functions = NV_ENCODE_API_FUNCTION_LIST {
        version: struct_version(2, false),
        ..Default::default()
    };
    if create(&mut functions) != NV_ENC_SUCCESS {
        return None;
    }
    let mut device = 0;
    let mut context = std::ptr::null_mut();
    if init(0) != 0 || device_get(&mut device, 0) != 0 || retain(&mut context, device) != 0 {
        return None;
    }
    Some(Api {
        functions,
        context,
        push,
        pop,
        _cuda: cuda,
        _nvenc: nvenc,
    })
}

/// The CUDA context made current on this thread until dropped.
struct Current<'a>(&'a Api);

impl<'a> Current<'a> {
    fn new(api: &'a Api) -> Result<Self, Error> {
        // SAFETY: the context was retained at load and is never released.
        if unsafe { (api.push)(api.context) } != 0 {
            return Err(Error::EncodeRejected("NVENC could not use the GPU"));
        }
        Ok(Self(api))
    }
}

impl Drop for Current<'_> {
    fn drop(&mut self) {
        let mut popped = std::ptr::null_mut();
        // SAFETY: pops the context pushed by `new` on this thread.
        unsafe { (self.0.pop)(&mut popped) };
    }
}

fn check(status: NVENCSTATUS, failure: &'static str) -> Result<(), Error> {
    if status == NV_ENC_SUCCESS {
        Ok(())
    } else {
        Err(Error::EncodeRejected(failure))
    }
}

/// An entry point from the function list, or a rejection when the driver
/// left it out.
macro_rules! call {
    ($api:expr, $name:ident) => {
        $api.functions
            .$name
            .ok_or(Error::Unsupported("this NVENC driver"))?
    };
}

/// Whether this machine's NVIDIA driver loads and its GPU has `codec`.
pub(crate) fn offers(codec: VideoCodec) -> bool {
    Encoder::open(codec, 256, 144, 30, 1_000_000, false).is_ok()
}

/// The session initialized before every other open session and closed after
/// them, so the driver's clock thread only ever holds its device.
struct Anchor {
    sessions: usize,
    encoder: Option<Encoder>,
}

static ANCHOR: Mutex<Anchor> = Mutex::new(Anchor {
    sessions: 0,
    encoder: None,
});

/// Sessions counted by the anchor, and whether the anchor is open.
#[cfg(test)]
pub(crate) fn anchor_state() -> (usize, bool) {
    let anchor = ANCHOR.lock().unwrap_or_else(|e| e.into_inner());
    (anchor.sessions, anchor.encoder.is_some())
}

/// One NVENC session and its two buffers.
pub(crate) struct Encoder {
    api: &'static Api,
    session: *mut c_void,
    input: NV_ENC_INPUT_PTR,
    output: NV_ENC_OUTPUT_PTR,
    codec: VideoCodec,
    width: u16,
    height: u16,
    pictures: u32,
    /// Counted in [`ANCHOR`]; false for the anchor itself.
    anchored: bool,
}

// SAFETY: a session and its buffers are used from one thread at a time
// (the encoder is `&mut` for every call), with the CUDA context current.
unsafe impl Send for Encoder {}

impl Encoder {
    pub(crate) fn open(
        codec: VideoCodec,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<Self, Error> {
        // Held while the session opens, so no other session is initialized
        // between the anchor and this one.
        let mut anchor = ANCHOR.lock().unwrap_or_else(|e| e.into_inner());
        if anchor.encoder.is_none() {
            anchor.encoder = Some(Self::open_session(
                VideoCodec::H264,
                256,
                144,
                30,
                1_000_000,
                false,
            )?);
        }
        match Self::open_session(codec, width, height, frame_rate, bitrate_bps, high) {
            Ok(mut encoder) => {
                encoder.anchored = true;
                anchor.sessions += 1;
                Ok(encoder)
            }
            Err(error) => {
                if anchor.sessions == 0 {
                    anchor.encoder = None;
                }
                Err(error)
            }
        }
    }

    fn open_session(
        codec: VideoCodec,
        width: u16,
        height: u16,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<Self, Error> {
        let api = api().ok_or(Error::Unsupported("NVENC"))?;
        let encode_guid = match codec {
            VideoCodec::H264 => guid::CODEC_H264,
            VideoCodec::H265 => guid::CODEC_HEVC,
            VideoCodec::Av1 => guid::CODEC_AV1,
            _ => return Err(Error::Unsupported("NVENC for this codec")),
        };
        let _current = Current::new(api)?;
        let mut session = std::ptr::null_mut();
        let mut params = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
            version: struct_version(1, false),
            deviceType: NV_ENC_DEVICE_TYPE_CUDA,
            device: api.context,
            apiVersion: API_VERSION,
            ..Default::default()
        };
        // SAFETY: `params` and `session` outlive the call.
        check(
            unsafe { call!(api, nvEncOpenEncodeSessionEx)(&mut params, &mut session) },
            "NVENC could not open a session",
        )
        .map_err(|_| Error::Unsupported("NVENC"))?;
        // From here the session is closed by `Drop`, also on failure.
        let mut encoder = Self {
            api,
            session,
            input: std::ptr::null_mut(),
            output: std::ptr::null_mut(),
            codec,
            width,
            height,
            pictures: 0,
            anchored: false,
        };
        encoder.initialize(encode_guid, frame_rate, bitrate_bps, high)?;
        Ok(encoder)
    }

    fn initialize(
        &mut self,
        encode_guid: GUID,
        frame_rate: u8,
        bitrate_bps: u32,
        high: bool,
    ) -> Result<(), Error> {
        let api = self.api;
        let unsupported = |_| Error::Unsupported("NVENC for this codec");
        let mut preset = Box::new(NV_ENC_PRESET_CONFIG {
            version: struct_version(4, true),
            presetCfg: NV_ENC_CONFIG {
                version: struct_version(8, true),
                ..Default::default()
            },
            ..Default::default()
        });
        // SAFETY: the session is open and `preset` outlives the call.
        check(
            unsafe {
                call!(api, nvEncGetEncodePresetConfigEx)(
                    self.session,
                    encode_guid,
                    guid::PRESET_P3,
                    NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
                    &mut *preset,
                )
            },
            "NVENC has no preset for this codec",
        )
        .map_err(unsupported)?;
        let mut config = Box::new(preset.presetCfg);
        config.version = struct_version(8, true);
        config.gopLength = KEYFRAME_INTERVAL;
        config.frameIntervalP = 1;
        let rc = &mut config.rcParams;
        rc.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        rc.averageBitRate = bitrate_bps;
        rc.maxBitRate = bitrate_bps;
        // One picture's worth of buffer: a picture leaves as it is made.
        rc.vbvBufferSize = bitrate_bps / u32::from(frame_rate.max(1));
        rc.vbvInitialDelay = rc.vbvBufferSize;
        rc.multiPass = NV_ENC_MULTI_PASS_DISABLED;
        // SAFETY: each arm writes the union member of the codec opened.
        unsafe {
            match self.codec {
                VideoCodec::H264 => {
                    config.profileGUID = if high {
                        guid::H264_PROFILE_HIGH
                    } else {
                        guid::H264_PROFILE_BASELINE
                    };
                    let h264 = &mut config.encodeCodecConfig.h264Config;
                    h264.idrPeriod = KEYFRAME_INTERVAL;
                    h264.set_repeatSPSPPS(1);
                }
                VideoCodec::H265 => {
                    config.profileGUID = guid::HEVC_PROFILE_MAIN;
                    let hevc = &mut config.encodeCodecConfig.hevcConfig;
                    hevc.idrPeriod = KEYFRAME_INTERVAL;
                    hevc.set_repeatSPSPPS(1);
                    hevc.set_chromaFormatIDC(1);
                    hevc.set_pixelBitDepthMinus8(0);
                }
                _ => {
                    config.profileGUID = guid::AV1_PROFILE_MAIN;
                    let av1 = &mut config.encodeCodecConfig.av1Config;
                    av1.idrPeriod = KEYFRAME_INTERVAL;
                    av1.set_repeatSeqHdr(1);
                    av1.set_chromaFormatIDC(1);
                    av1.set_inputPixelBitDepthMinus8(0);
                    av1.set_pixelBitDepthMinus8(0);
                }
            }
        }
        let mut init = Box::new(NV_ENC_INITIALIZE_PARAMS {
            version: struct_version(6, true),
            encodeGUID: encode_guid,
            presetGUID: guid::PRESET_P3,
            encodeWidth: u32::from(self.width),
            encodeHeight: u32::from(self.height),
            darWidth: u32::from(self.width),
            darHeight: u32::from(self.height),
            frameRateNum: u32::from(frame_rate.max(1)),
            frameRateDen: 1,
            enablePTD: 1,
            encodeConfig: &mut *config,
            maxEncodeWidth: u32::from(self.width),
            maxEncodeHeight: u32::from(self.height),
            tuningInfo: NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY,
            ..Default::default()
        });
        // SAFETY: `init` and the `config` it points to outlive the call.
        check(
            unsafe { call!(api, nvEncInitializeEncoder)(self.session, &mut *init) },
            "NVENC refused the settings",
        )
        .map_err(unsupported)?;
        let mut input = NV_ENC_CREATE_INPUT_BUFFER {
            version: struct_version(1, false),
            width: u32::from(self.width),
            height: u32::from(self.height),
            memoryHeap: NV_ENC_MEMORY_HEAP_AUTOSELECT,
            bufferFmt: NV_ENC_BUFFER_FORMAT_NV12,
            ..Default::default()
        };
        // SAFETY: the session is initialized; `input` outlives the call.
        check(
            unsafe { call!(api, nvEncCreateInputBuffer)(self.session, &mut input) },
            "NVENC has no input buffer",
        )?;
        self.input = input.inputBuffer;
        let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER {
            version: struct_version(1, false),
            ..Default::default()
        };
        // SAFETY: as above.
        check(
            unsafe { call!(api, nvEncCreateBitstreamBuffer)(self.session, &mut output) },
            "NVENC has no output buffer",
        )?;
        self.output = output.bitstreamBuffer;
        Ok(())
    }

    pub(crate) fn encode(
        &mut self,
        y: &[u8],
        uv: &[u8],
        force_keyframe: bool,
    ) -> Result<EncodedSample, Error> {
        let api = self.api;
        let rejected = |_| Error::EncodeRejected("NVENC rejected the frame");
        let width = usize::from(self.width);
        let height = usize::from(self.height);
        if y.len() < width * height || uv.len() < width * height / 2 {
            return Err(Error::InvalidFrame(
                "the frame size does not match the encoder",
            ));
        }
        let _current = Current::new(api)?;
        let mut lock = NV_ENC_LOCK_INPUT_BUFFER {
            version: struct_version(1, false),
            inputBuffer: self.input,
            ..Default::default()
        };
        // SAFETY: the input buffer belongs to this session.
        check(
            unsafe { call!(api, nvEncLockInputBuffer)(self.session, &mut lock) },
            "NVENC rejected the frame",
        )?;
        let pitch = lock.pitch as usize;
        if pitch >= width && !lock.bufferDataPtr.is_null() {
            // SAFETY: a locked NV12 buffer is `pitch` bytes by `height`
            // luma rows, then half as many chroma rows, until unlocked.
            let planes = unsafe {
                std::slice::from_raw_parts_mut(
                    lock.bufferDataPtr.cast::<u8>(),
                    pitch * height * 3 / 2,
                )
            };
            for (row, source) in y.chunks_exact(width).take(height).enumerate() {
                planes[row * pitch..row * pitch + width].copy_from_slice(source);
            }
            let chroma = &mut planes[pitch * height..];
            for (row, source) in uv.chunks_exact(width).take(height / 2).enumerate() {
                chroma[row * pitch..row * pitch + width].copy_from_slice(source);
            }
        }
        // SAFETY: unlocks the buffer locked above.
        let unlocked = unsafe { call!(api, nvEncUnlockInputBuffer)(self.session, self.input) };
        if pitch < width || lock.bufferDataPtr.is_null() {
            return Err(Error::EncodeRejected("NVENC rejected the frame"));
        }
        check(unlocked, "NVENC rejected the frame")?;

        let key = force_keyframe || self.pictures == 0;
        let mut picture = Box::new(NV_ENC_PIC_PARAMS {
            version: struct_version(6, true),
            inputWidth: u32::from(self.width),
            inputHeight: u32::from(self.height),
            inputPitch: pitch as u32,
            encodePicFlags: if key {
                NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS
            } else {
                0
            },
            frameIdx: self.pictures,
            inputTimeStamp: u64::from(self.pictures),
            inputBuffer: self.input,
            outputBitstream: self.output,
            bufferFmt: NV_ENC_BUFFER_FORMAT_NV12,
            pictureStruct: NV_ENC_PIC_STRUCT_FRAME,
            ..Default::default()
        });
        // SAFETY: both buffers belong to this session; `picture` outlives it.
        check(
            unsafe { call!(api, nvEncEncodePicture)(self.session, &mut *picture) },
            "NVENC rejected the frame",
        )?;
        self.pictures = self.pictures.wrapping_add(1);
        let mut bitstream = NV_ENC_LOCK_BITSTREAM {
            version: struct_version(1, true),
            outputBitstream: self.output,
            ..Default::default()
        };
        // SAFETY: waits for the picture just encoded into this buffer.
        check(
            unsafe { call!(api, nvEncLockBitstream)(self.session, &mut bitstream) },
            "NVENC rejected the frame",
        )?;
        let bytes = if bitstream.bitstreamBufferPtr.is_null() {
            Vec::new()
        } else {
            // SAFETY: a locked bitstream holds `bitstreamSizeInBytes` bytes.
            unsafe {
                std::slice::from_raw_parts(
                    bitstream.bitstreamBufferPtr.cast::<u8>(),
                    bitstream.bitstreamSizeInBytes as usize,
                )
            }
            .to_vec()
        };
        // SAFETY: unlocks the bitstream locked above.
        check(
            unsafe { call!(api, nvEncUnlockBitstream)(self.session, self.output) },
            "NVENC rejected the frame",
        )?;
        if bytes.is_empty() {
            return Err(Error::EncodeRejected("the encoder produced no frame"));
        }
        let key = matches!(
            bitstream.pictureType,
            NV_ENC_PIC_TYPE_IDR | NV_ENC_PIC_TYPE_I
        );
        match self.codec {
            VideoCodec::H264 => EncodedSample::annex_b(key, bytes).map_err(rejected),
            VideoCodec::H265 => hevc_sample(bytes),
            _ => EncodedSample::packet(VideoCodec::Av1, key, bytes),
        }
    }
}

impl Encoder {
    /// Releases this session's buffers, then the session.
    fn close(&mut self) {
        let api = self.api;
        let Ok(_current) = Current::new(api) else {
            return;
        };
        // SAFETY: each was created by this session and is not used again.
        unsafe {
            if let (false, Some(destroy)) =
                (self.input.is_null(), api.functions.nvEncDestroyInputBuffer)
            {
                destroy(self.session, self.input);
            }
            if let (false, Some(destroy)) = (
                self.output.is_null(),
                api.functions.nvEncDestroyBitstreamBuffer,
            ) {
                destroy(self.session, self.output);
            }
            if let Some(destroy) = api.functions.nvEncDestroyEncoder {
                destroy(self.session);
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.close();
        if self.anchored {
            let mut anchor = ANCHOR.lock().unwrap_or_else(|e| e.into_inner());
            anchor.sessions -= 1;
            if anchor.sessions == 0 {
                // The last session is closed: the anchor closes last.
                anchor.encoder = None;
            }
        }
    }
}
