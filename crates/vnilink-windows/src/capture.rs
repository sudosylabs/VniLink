//! Desktop Duplication for the interactive desktop of this session.
//!
//! The secure UAC desktop is a different desktop. DuplicateOutput fails there,
//! and this module reports that instead of pretending the picture is available.

use std::slice;

use vnilink_media::{Error, RawFrame};
use windows::core::Interface;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use crate::plan::frame_size;

pub struct WindowsCapture {
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
}

impl WindowsCapture {
    /// Opens the first output of the first adapter. Monitor index is always 0.
    pub fn open() -> Result<Self, Error> {
        let capture = unsafe { Self::open_desktop() }.map_err(map_error)?;
        if capture.format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Error::Unsupported("Desktop Duplication format is not BGRA"));
        }
        frame_size(capture.width, capture.height)?;
        Ok(capture)
    }

    pub fn size(&self) -> Result<(u16, u16), Error> {
        frame_size(self.width, self.height)
    }

    /// `Ok(None)` when the desktop has not produced a new frame yet.
    pub fn next_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        unsafe { self.acquire(timestamp_us) }
    }

    unsafe fn open_desktop() -> windows::core::Result<Self> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0)?;
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        let context = context.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
        let output = adapter.EnumOutputs(0)?;
        let output: IDXGIOutput1 = output.cast()?;
        let duplication = output.DuplicateOutput(&device)?;
        let desc = duplication.GetDesc();
        Ok(Self {
            context,
            duplication,
            format: desc.ModeDesc.Format,
            width: desc.ModeDesc.Width,
            height: desc.ModeDesc.Height,
        })
    }

    unsafe fn acquire(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let mut info = windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if let Err(err) = self
            .duplication
            .AcquireNextFrame(0, &mut info, &mut resource)
        {
            if err.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return Ok(None);
            }
            return Err(map_error(err));
        }
        let frame = self.copy_resource(resource.as_ref(), timestamp_us);
        if let Err(err) = self.duplication.ReleaseFrame() {
            return Err(map_error(err));
        }
        frame
    }

    unsafe fn copy_resource(
        &self,
        resource: Option<&IDXGIResource>,
        timestamp_us: u64,
    ) -> Result<Option<RawFrame>, Error> {
        let Some(resource) = resource else {
            return Ok(None);
        };
        let source: ID3D11Texture2D = resource.cast().map_err(map_error)?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        source.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Error::Unsupported("Desktop Duplication format is not BGRA"));
        }
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let device = self.context.GetDevice().map_err(map_error)?;
        let mut staging = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .map_err(map_error)?;
        let staging =
            staging.ok_or_else(|| map_error(windows::core::Error::from_hresult(E_FAIL)))?;
        self.context.CopyResource(&staging, &source);
        let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(map_error)?;
        let copied = copy_mapped(
            desc.Width,
            desc.Height,
            mapped.RowPitch,
            mapped.pData,
            timestamp_us,
        );
        self.context.Unmap(&staging, 0);
        copied
    }
}

fn copy_mapped(
    width: u32,
    height: u32,
    stride: u32,
    data: *mut std::ffi::c_void,
    timestamp_us: u64,
) -> Result<Option<RawFrame>, Error> {
    let (width, height) = frame_size(width, height)?;
    if data.is_null() || stride == 0 {
        return Err(Error::CaptureRejected(
            "Desktop Duplication returned an empty frame",
        ));
    }
    let bytes = stride as usize * usize::from(height);
    let pixels = unsafe { slice::from_raw_parts(data as *const u8, bytes) };
    RawFrame::with_stride(
        width,
        height,
        stride as usize,
        timestamp_us,
        pixels.to_vec(),
    )
    .map(Some)
}

fn map_error(err: windows::core::Error) -> Error {
    let code = err.code();
    if code == DXGI_ERROR_ACCESS_LOST || code == E_ACCESSDENIED {
        return Error::CaptureRejected("Desktop Duplication cannot read this desktop");
    }
    Error::CaptureFailed(code.0)
}
