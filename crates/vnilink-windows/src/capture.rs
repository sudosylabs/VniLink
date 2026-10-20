//! Desktop Duplication for the interactive desktop of this session.
//!
//! The secure UAC desktop is a different desktop. DuplicateOutput fails there,
//! and this module reports that instead of pretending the picture is available.

use std::slice;
use std::sync::Mutex;

use vnilink_media::{Error, RawFrame};
use windows::core::Interface;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
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
    /// Desktop size validated at open. A resize surfaces as access lost on the
    /// next acquire, so the size is fixed for the life of one capture.
    size: (u16, u16),
    /// Where this output's top-left corner lies on the virtual desktop.
    origin: (i32, i32),
    last: Mutex<Option<RawFrame>>,
    pointer: Mutex<crate::cursor::Pointer>,
    /// Last successful acquire or timeout. An error clears it.
    attested: Mutex<Option<std::time::Instant>>,
}

/// Desktop Duplication handles before the format and size are validated.
struct UnvalidatedDesktop {
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    origin: (i32, i32),
}

impl WindowsCapture {
    /// Opens the first output of the first adapter. Monitor index is always 0.
    pub fn open(startup: &vnilink::BackendStartup) -> Result<Self, Error> {
        check_startup(startup)?;
        let desktop = unsafe { Self::open_desktop(startup) }?;
        check_startup(startup)?;
        if desktop.format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Error::Unsupported("Desktop Duplication format is not BGRA"));
        }
        Ok(Self {
            context: desktop.context,
            duplication: desktop.duplication,
            size: frame_size(desktop.width, desktop.height)?,
            origin: desktop.origin,
            last: Mutex::new(None),
            pointer: Mutex::new(crate::cursor::Pointer::default()),
            attested: Mutex::new(None),
        })
    }

    /// When Desktop Duplication last attested the picture, including a timeout
    /// that means the desktop has not changed. Access loss clears it.
    pub fn attested_at(&self) -> Option<std::time::Instant> {
        *self
            .attested
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Where the captured output's top-left corner lies on the virtual
    /// desktop, in the pixels pointer input is placed in.
    pub fn origin(&self) -> (i32, i32) {
        self.origin
    }

    /// `Ok(None)` when the desktop has not produced a new frame yet. The call
    /// itself never waits: blocking until a newer frame arrives lives with
    /// the host caller behind the frame-source seam.
    pub fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Check the native source even when pixels are cached: access loss or
        // permission denial must not be hidden by an old picture.
        let acquired = unsafe { self.acquire(timestamp_us) };
        if acquired.is_ok() {
            *self
                .attested
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(std::time::Instant::now());
        } else {
            *self
                .attested
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = None;
        }
        let frame = crate::idle::picture(acquired, &mut last, timestamp_us)?;
        frame
            .map(|frame| {
                self.pointer
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .composite(frame)
            })
            .transpose()
    }

    unsafe fn open_desktop(startup: &vnilink::BackendStartup) -> Result<UnvalidatedDesktop, Error> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        check_startup(startup)?;
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(map_error)?;
        check_startup(startup)?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapters1(0).map_err(map_error)?;
        let mut device = None;
        let mut context = None;
        check_startup(startup)?;
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
        )
        .map_err(map_error)?;
        check_startup(startup)?;
        let device = device.ok_or_else(|| map_error(windows::core::Error::from_hresult(E_FAIL)))?;
        let context =
            context.ok_or_else(|| map_error(windows::core::Error::from_hresult(E_FAIL)))?;
        let output = adapter.EnumOutputs(0).map_err(map_error)?;
        let bounds = output.GetDesc().map_err(map_error)?.DesktopCoordinates;
        let output: IDXGIOutput1 = output.cast().map_err(map_error)?;
        check_startup(startup)?;
        let duplication = output.DuplicateOutput(&device).map_err(map_error)?;
        check_startup(startup)?;
        let desc = duplication.GetDesc();
        Ok(UnvalidatedDesktop {
            context,
            duplication,
            format: desc.ModeDesc.Format,
            width: desc.ModeDesc.Width,
            height: desc.ModeDesc.Height,
            origin: (bounds.left, bounds.top),
        })
    }

    unsafe fn update_pointer(
        &self,
        info: &windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO,
    ) -> Result<(), Error> {
        let mut pointer = self.pointer.lock().unwrap_or_else(|p| p.into_inner());
        if info.LastMouseUpdateTime != 0 {
            pointer.visible = info.PointerPosition.Visible.as_bool();
            pointer.position = (
                info.PointerPosition.Position.x,
                info.PointerPosition.Position.y,
            );
        }
        if info.PointerShapeBufferSize != 0 {
            let size = info.PointerShapeBufferSize as usize;
            if size > crate::cursor::MAX_SHAPE_BYTES {
                return Err(Error::InvalidFrame("DXGI cursor shape is too large"));
            }
            let mut bytes = vec![0u8; size];
            let mut required = 0;
            let mut shape =
                windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            self.duplication
                .GetFramePointerShape(
                    info.PointerShapeBufferSize,
                    bytes.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape,
                )
                .map_err(map_error)?;
            if required as usize > bytes.len() {
                return Err(Error::InvalidFrame("DXGI cursor shape changed size"));
            }
            bytes.truncate(required as usize);
            let kind = match shape.Type {
                1 => crate::cursor::Kind::Monochrome,
                2 => crate::cursor::Kind::Color,
                4 => crate::cursor::Kind::MaskedColor,
                _ => return Err(Error::Unsupported("DXGI cursor shape type")),
            };
            pointer.shape = Some(crate::cursor::Shape::new(
                kind,
                shape.Width,
                shape.Height,
                shape.Pitch,
                bytes,
            )?);
        }
        Ok(())
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
        // ReleaseFrame still runs if shape extraction fails. Cache only the
        // desktop base; each presentation composites the newest pointer once.
        // A frame with no LastPresentTime only moved the pointer: its surface
        // is not a desktop image (the first one after DuplicateOutput is
        // black), so it updates the pointer and keeps the cached picture.
        let presented = info.LastPresentTime != 0;
        let frame = self.update_pointer(&info).and_then(|_| {
            if presented {
                self.copy_resource(resource.as_ref(), timestamp_us)
            } else {
                Ok(None)
            }
        });
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

// COM allocations are owned locals at every boundary, so cancellation also
// releases a duplication returned after the owner ended. Synchronous driver
// calls cannot be forcibly interrupted; cancellation is checked on return.
fn check_startup(startup: &vnilink::BackendStartup) -> Result<(), Error> {
    startup
        .check()
        .map_err(|_| Error::CaptureRejected("Desktop capture startup ended"))
}

#[cfg(test)]
mod startup_tests {
    #[test]
    fn cancelled_startup_does_not_enter_desktop_duplication() {
        let startup = vnilink::BackendStartup::new(std::time::Duration::ZERO);
        assert!(matches!(
            super::WindowsCapture::open(&startup),
            Err(vnilink_media::Error::CaptureRejected(
                "Desktop capture startup ended"
            ))
        ));
    }
}
