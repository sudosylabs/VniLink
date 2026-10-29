//! Desktop pictures kept on the GPU that captured them, and their
//! conversion to NV12 there.
//!
//! Desktop Duplication hands out its picture until the next `ReleaseFrame`,
//! so the capture copies it into a pooled texture on the same GPU
//! ([`TexturePool`]) instead of into memory. The encoder converts that
//! texture to NV12 and scales it on the GPU with a D3D11 video processor
//! ([`Nv12Converter`]) and reads back only the NV12 picture for the
//! transform. Anything else that reads the pixels gets the BGRA picture
//! copied out once ([`vnilink_media::Pixels`]).
//!
//! The device is multithread-protected: the capture thread and each encoder
//! thread use its immediate context.
// trace: records every ignored failure

use std::sync::{Arc, Mutex};

use vnilink::YuvMatrix;
use vnilink_media::{Error, NativePicture};
use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext1,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BIND_VIDEO_ENCODER, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12,
    DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

/// Lets the capture thread and encoder threads share one device's
/// immediate context: D3D11 serialises its calls.
pub(crate) unsafe fn protect(device: &ID3D11Device) -> windows::core::Result<()> {
    let multithread: ID3D11Multithread = device.cast()?;
    // Returns whether it was protected before, not whether it worked.
    let _was_protected = multithread.SetMultithreadProtected(true);
    Ok(())
}

/// One pooled BGRA texture; a picture holds it while anything uses it.
pub(crate) struct Pooled {
    texture: ID3D11Texture2D,
}

// D3D11 objects of a multithread-protected device may be used from any thread.
unsafe impl Send for Pooled {}
unsafe impl Sync for Pooled {}

/// BGRA textures the capture copies desktop pictures into. A texture is
/// reused only when no picture holds it.
pub(crate) struct TexturePool {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    size: (u32, u32),
    textures: Mutex<Vec<Arc<Pooled>>>,
}

unsafe impl Send for TexturePool {}
unsafe impl Sync for TexturePool {}

impl TexturePool {
    pub(crate) fn new(
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        size: (u32, u32),
    ) -> Self {
        Self {
            device,
            context,
            size,
            textures: Mutex::new(Vec::new()),
        }
    }

    /// The device pictures are copied on.
    pub(crate) fn device(&self) -> GpuDevice {
        GpuDevice::new(self.device.clone())
    }

    /// `source` copied on the GPU into a free texture, as a picture.
    pub(crate) unsafe fn picture(
        &self,
        source: &ID3D11Texture2D,
    ) -> windows::core::Result<WindowsPicture> {
        let pooled = {
            let mut textures = self
                .textures
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            match textures
                .iter()
                .find(|pooled| Arc::strong_count(pooled) == 1)
            {
                Some(free) => Arc::clone(free),
                None => {
                    let pooled = Arc::new(Pooled {
                        texture: texture(
                            &self.device,
                            self.size,
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            true,
                        )?,
                    });
                    textures.push(Arc::clone(&pooled));
                    pooled
                }
            }
        };
        self.context.CopyResource(&pooled.texture, source);
        Ok(WindowsPicture {
            pooled,
            device: self.device.clone(),
            context: self.context.clone(),
            size: (self.size.0 as u16, self.size.1 as u16),
        })
    }
}

/// A desktop picture in a texture on the GPU that captured it.
pub struct WindowsPicture {
    pooled: Arc<Pooled>,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    size: (u16, u16),
}

unsafe impl Send for WindowsPicture {}
unsafe impl Sync for WindowsPicture {}

impl WindowsPicture {
    pub(crate) fn texture(&self) -> &ID3D11Texture2D {
        &self.pooled.texture
    }

    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub(crate) fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }
}

impl NativePicture for WindowsPicture {
    fn size(&self) -> (u16, u16) {
        self.size
    }

    fn read_back(&self) -> Result<Vec<u8>, Error> {
        let (width, height) = (u32::from(self.size.0), u32::from(self.size.1));
        let rows = unsafe {
            read_rows(
                &self.device,
                &self.context,
                self.texture(),
                (width, height),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                width as usize * 4,
                height as usize,
            )
        }
        .map_err(|_| Error::CaptureRejected("the captured picture could not be read"))?;
        Ok(rows)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Converts captured pictures to limited NV12 in the session's matrix at one output size on
/// their GPU, and reads the NV12 back for a transform that takes memory.
pub(crate) struct Nv12Converter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video: ID3D11VideoContext1,
    video_device: ID3D11VideoDevice,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    /// NV12 pictures in turn: an encoder that takes textures may still be
    /// reading the last one when the next is converted.
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: std::cell::Cell<usize>,
    staging: ID3D11Texture2D,
    input: (u32, u32),
    size: (u32, u32),
}

// Used through `&mut self` by one encoder; the device is multithread-protected.
unsafe impl Send for Nv12Converter {}

impl Nv12Converter {
    /// For pictures of `input` size on `device`, converted to `size`.
    pub(crate) unsafe fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        input: (u32, u32),
        size: (u32, u32),
        matrix: YuvMatrix,
    ) -> windows::core::Result<Self> {
        let video_device: ID3D11VideoDevice = device.cast()?;
        let video: ID3D11VideoContext1 = context.cast()?;
        let rate = DXGI_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input.0,
            InputHeight: input.1,
            OutputFrameRate: rate,
            OutputWidth: size.0,
            OutputHeight: size.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
        let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
        let mut outputs = Vec::with_capacity(OUTPUTS);
        for _ in 0..OUTPUTS {
            // An encoder may read the texture itself; a driver that refuses
            // the video-encoder binding gets one without it.
            let output = texture_bound(
                device,
                size,
                DXGI_FORMAT_NV12,
                D3D11_BIND_VIDEO_ENCODER.0 as u32,
            )
            .or_else(|_| texture(device, size, DXGI_FORMAT_NV12, true))?;
            let mut view = None;
            video_device.CreateVideoProcessorOutputView(
                &output,
                &enumerator,
                &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    ..Default::default()
                },
                Some(&mut view),
            )?;
            let view =
                view.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            outputs.push((output, view));
        }
        // Full-range RGB in, the stream's colour out: the session's matrix,
        // limited range.
        video.VideoProcessorSetStreamColorSpace1(
            &processor,
            0,
            DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
        );
        video.VideoProcessorSetOutputColorSpace1(
            &processor,
            match matrix {
                YuvMatrix::Bt709 => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                YuvMatrix::Bt601 => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
            },
        );
        video.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
        video.VideoProcessorSetStreamFrameFormat(
            &processor,
            0,
            D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
        );
        let whole = RECT {
            left: 0,
            top: 0,
            right: size.0 as i32,
            bottom: size.1 as i32,
        };
        video.VideoProcessorSetOutputTargetRect(&processor, true, Some(&whole));
        let staging = staging(device, size, DXGI_FORMAT_NV12)?;
        Ok(Self {
            device: device.clone(),
            context: context.clone(),
            video,
            video_device,
            enumerator,
            processor,
            outputs,
            next: std::cell::Cell::new(0),
            staging,
            input,
            size,
        })
    }

    /// Whether `picture` can go through this converter.
    pub(crate) fn fits(&self, picture: &WindowsPicture) -> bool {
        picture.device().as_raw() == self.device.as_raw()
            && (u32::from(picture.size.0), u32::from(picture.size.1)) == self.input
    }

    /// `picture` as NV12 at this converter's size, in one of its textures,
    /// for an encoder that takes textures on this device.
    pub(crate) unsafe fn convert_texture(
        &self,
        picture: &WindowsPicture,
    ) -> windows::core::Result<ID3D11Texture2D> {
        let at = self.next.get();
        self.next.set((at + 1) % self.outputs.len());
        let (output, view) = &self.outputs[at];
        let mut input_view = None;
        self.video_device.CreateVideoProcessorInputView(
            picture.texture(),
            &self.enumerator,
            &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                ..Default::default()
            },
            Some(&mut input_view),
        )?;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(input_view),
            ..Default::default()
        };
        let mut streams = [stream];
        let blitted = self
            .video
            .VideoProcessorBlt(&self.processor, view, 0, &streams);
        std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
        blitted?;
        Ok(output.clone())
    }

    /// `picture` as packed NV12 at this converter's size, read into memory:
    /// the Y plane, then interleaved UV.
    pub(crate) unsafe fn convert(
        &self,
        picture: &WindowsPicture,
    ) -> windows::core::Result<(Vec<u8>, Vec<u8>)> {
        let output = self.convert_texture(picture)?;
        let (width, height) = (self.size.0 as usize, self.size.1 as usize);
        let bytes = read_rows_from(&self.context, &self.staging, &output, width, height * 3 / 2)?;
        let (y, uv) = bytes.split_at(width * height);
        Ok((y.to_vec(), uv.to_vec()))
    }
}

/// Converted pictures a converter keeps in turn.
const OUTPUTS: usize = 3;

/// A device that captured pictures live on, for opening an encoder on it.
#[derive(Clone)]
pub struct GpuDevice(ID3D11Device);

unsafe impl Send for GpuDevice {}
unsafe impl Sync for GpuDevice {}

impl GpuDevice {
    pub(crate) fn new(device: ID3D11Device) -> Self {
        Self(device)
    }

    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.0
    }

    /// The adapter the device is on.
    pub(crate) unsafe fn adapter(&self) -> windows::core::Result<windows::Win32::Foundation::LUID> {
        let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = self.0.cast()?;
        Ok(dxgi.GetAdapter()?.GetDesc()?.AdapterLuid)
    }
}

unsafe fn texture_bound(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    extra: u32,
) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32 | extra,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut created = None;
    device.CreateTexture2D(&desc, None, Some(&mut created))?;
    created.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

unsafe fn texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    render_target: bool,
) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: if render_target {
            (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32
        } else {
            0
        },
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut created = None;
    device.CreateTexture2D(&desc, None, Some(&mut created))?;
    created.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

unsafe fn staging(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
) -> windows::core::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut created = None;
    device.CreateTexture2D(&desc, None, Some(&mut created))?;
    created.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

/// `texture` copied out row by row: `rows` rows of `row` bytes, packed.
unsafe fn read_rows(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    size: (u32, u32),
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    row: usize,
    rows: usize,
) -> windows::core::Result<Vec<u8>> {
    let staging = staging(device, size, format)?;
    read_rows_from(context, &staging, texture, row, rows)
}

/// Copies `texture` into `staging` and reads `rows` rows of `row` bytes. For
/// NV12 the UV rows follow the Y rows at the same pitch.
unsafe fn read_rows_from(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    texture: &ID3D11Texture2D,
    row: usize,
    rows: usize,
) -> windows::core::Result<Vec<u8>> {
    context.CopyResource(staging, texture);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    let pitch = mapped.RowPitch as usize;
    let mut packed = Vec::with_capacity(row * rows);
    if !mapped.pData.is_null() && pitch >= row {
        let source = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * rows);
        for y in 0..rows {
            packed.extend_from_slice(&source[y * pitch..y * pitch + row]);
        }
    }
    context.Unmap(staging, 0);
    if packed.len() != row * rows {
        return Err(windows::core::Error::from(
            windows::Win32::Foundation::E_FAIL,
        ));
    }
    Ok(packed)
}
