//! Desktop Duplication for the interactive desktop of this session.
//!
//! The secure UAC desktop is a different desktop. DuplicateOutput fails there,
//! and this module reports that instead of pretending the picture is available.
//!
//! The output captured is the primary display's, on whichever adapter it is
//! attached to ([`crate::outputs`]), and the capture device is created on
//! that adapter.
//!
//! Pictures stay on the GPU that captured them ([`crate::gpu`]): the encoder
//! converts and scales them there, and anything else that reads their pixels
//! copies them out once.
// trace: records every ignored failure

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use vnilink_media::{Error, RawFrame};
use windows::core::Interface;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use crate::plan::frame_size;

pub struct WindowsCapture {
    duplication: IDXGIOutputDuplication,
    /// Where each presented picture is copied on the GPU.
    pool: crate::gpu::TexturePool,
    /// Desktop size validated at open. A resize surfaces as access lost on the
    /// next acquire, so the size is fixed for the life of one capture.
    size: (u16, u16),
    /// Where this output's top-left corner lies on the virtual desktop.
    origin: (i32, i32),
    last: Mutex<Option<RawFrame>>,
    pointer: Mutex<crate::cursor::Pointer>,
    /// Whether pictures carry the pointer. Off while every viewer draws the
    /// pointer itself from [`WindowsCapture::cursor`].
    embeds_cursor: AtomicBool,
    /// Last successful acquire or timeout. An error clears it.
    attested: Mutex<Option<std::time::Instant>>,
    /// What [`WindowsCapture::wait_frame`] acquired for the next poll.
    waited: Mutex<Waited>,
}

/// An acquire made by a wait, handed to the poll that follows it.
enum Waited {
    Nothing,
    /// A new picture, already in the cache.
    Picture,
    Failed(Error),
}

/// Desktop Duplication handles before the format and size are validated.
struct UnvalidatedDesktop {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    format: DXGI_FORMAT,
    width: u32,
    height: u32,
    origin: (i32, i32),
}

impl WindowsCapture {
    /// Opens the primary display's output, on the adapter it is attached to.
    pub fn open(startup: &vnilink::BackendStartup) -> Result<Self, Error> {
        check_startup(startup)?;
        let desktop = unsafe { Self::open_desktop(startup) }?;
        check_startup(startup)?;
        if desktop.format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(Error::Unsupported("Desktop Duplication format is not BGRA"));
        }
        Ok(Self {
            duplication: desktop.duplication,
            pool: crate::gpu::TexturePool::new(
                desktop.device,
                desktop.context,
                (desktop.width, desktop.height),
            ),
            size: frame_size(desktop.width, desktop.height)?,
            origin: desktop.origin,
            last: Mutex::new(None),
            pointer: Mutex::new(crate::cursor::Pointer::default()),
            embeds_cursor: AtomicBool::new(true),
            attested: Mutex::new(None),
            waited: Mutex::new(Waited::Nothing),
        })
    }

    /// When Desktop Duplication last delivered a new picture: a timeout or a
    /// pointer-only update leaves it, as the desktop did not change. Access
    /// loss clears it.
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
    /// itself never waits: [`WindowsCapture::wait_frame`] is where the host
    /// caller blocks until a newer frame arrives, behind the frame-source seam.
    pub fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let waited = std::mem::replace(
            &mut *self
                .waited
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
            Waited::Nothing,
        );
        match waited {
            Waited::Failed(error) => return Err(error),
            // The wait took the picture; this poll has nothing more to ask.
            Waited::Picture => {}
            // Check the native source even when pixels are cached: access
            // loss or permission denial must not be hidden by an old picture.
            Waited::Nothing => {
                self.refresh(&mut last, 0, timestamp_us)?;
            }
        }
        let frame = last.clone().map(|frame| frame.with_timestamp(timestamp_us));
        if !self.embeds_cursor.load(Ordering::Acquire) {
            return Ok(frame);
        }
        frame
            .map(|frame| {
                self.pointer
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .composite(frame)
            })
            .transpose()
    }

    /// One acquire of at most `timeout_ms`, into the cached picture and the
    /// attestation. Whether a new picture arrived.
    fn refresh(
        &self,
        last: &mut Option<RawFrame>,
        timeout_ms: u32,
        timestamp_us: u64,
    ) -> Result<bool, Error> {
        let acquired = unsafe { self.acquire(timeout_ms, timestamp_us) };
        let arrived = matches!(acquired, Ok(Some(_)));
        {
            let mut attested = self
                .attested
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            *attested = crate::idle::attestation(*attested, &acquired, std::time::Instant::now());
        }
        crate::idle::picture(acquired, last, timestamp_us)?;
        Ok(arrived)
    }

    /// Waits in Desktop Duplication for the desktop's next picture, until
    /// `deadline`. Whether one arrived or the capture failed; the poll that
    /// follows returns it without acquiring again.
    pub fn wait_frame(&self, deadline: std::time::Instant) -> bool {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        // Rounded up: a wait must not end a millisecond early and spin.
        let timeout_ms = u32::try_from(remaining.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let outcome = match self.refresh(&mut last, timeout_ms, 0) {
            Ok(true) => Waited::Picture,
            Ok(false) => return false,
            Err(error) => Waited::Failed(error),
        };
        *self
            .waited
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = outcome;
        true
    }

    /// The device pictures live on, to open an encoder that reads them there.
    pub fn device(&self) -> crate::gpu::GpuDevice {
        self.pool.device()
    }

    /// Whether pictures carry the pointer; on until every viewer draws it.
    pub fn set_embeds_cursor(&self, embeds: bool) {
        self.embeds_cursor.store(embeds, Ordering::Release);
    }

    /// The pointer, in capture pixels, while pictures leave it out.
    pub fn cursor(&self) -> Option<vnilink::CursorState> {
        if self.embeds_cursor.load(Ordering::Acquire) {
            return None;
        }
        self.pointer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .state()
    }

    unsafe fn open_desktop(startup: &vnilink::BackendStartup) -> Result<UnvalidatedDesktop, Error> {
        ignored_hr!(
            vnilink::trace::CAPTURE,
            "a thread with COM already set up keeps it",
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()
        );
        check_startup(startup)?;
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(map_error)?;
        check_startup(startup)?;
        let listed = list_outputs(&factory, startup)?;
        let descriptions: Vec<_> = listed.iter().map(|(_, _, output)| *output).collect();
        let chosen = crate::outputs::primary(&descriptions).ok_or(Error::CaptureRejected(
            "no display is attached to the desktop",
        ))?;
        let Some((adapter, output, _)) =
            listed.into_iter().find(|(_, _, output)| *output == chosen)
        else {
            return Err(Error::CaptureRejected(
                "no display is attached to the desktop",
            ));
        };
        let mut device = None;
        let mut context = None;
        check_startup(startup)?;
        // Video support lets encoders convert pictures to NV12 on this
        // device. A driver without it (Microsoft's basic display adapter, a
        // virtual machine's) refuses the flag: capture without it, and
        // encoders convert on the CPU.
        let create = |flags, device: &mut _, context: &mut _| {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(device),
                None,
                Some(context),
            )
        };
        create(
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            &mut device,
            &mut context,
        )
        .or_else(|_| create(D3D11_CREATE_DEVICE_BGRA_SUPPORT, &mut device, &mut context))
        .map_err(map_error)?;
        check_startup(startup)?;
        let device = device.ok_or_else(|| map_error(windows::core::Error::from_hresult(E_FAIL)))?;
        let context =
            context.ok_or_else(|| map_error(windows::core::Error::from_hresult(E_FAIL)))?;
        crate::gpu::protect(&device).map_err(map_error)?;
        let output: IDXGIOutput1 = output.cast().map_err(map_error)?;
        check_startup(startup)?;
        let duplication = output.DuplicateOutput(&device).map_err(map_error)?;
        check_startup(startup)?;
        let desc = duplication.GetDesc();
        Ok(UnvalidatedDesktop {
            device,
            context,
            duplication,
            format: desc.ModeDesc.Format,
            width: desc.ModeDesc.Width,
            height: desc.ModeDesc.Height,
            origin: chosen.origin,
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
            pointer.set_shape(
                crate::cursor::Shape::new(kind, shape.Width, shape.Height, shape.Pitch, bytes)?,
                (
                    u32::try_from(shape.HotSpot.x).unwrap_or(0),
                    u32::try_from(shape.HotSpot.y).unwrap_or(0),
                ),
            )?;
        }
        Ok(())
    }

    unsafe fn acquire(
        &self,
        timeout_ms: u32,
        timestamp_us: u64,
    ) -> Result<Option<RawFrame>, Error> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if let Err(err) = self
            .duplication
            .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
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
                let frame = self.copy_resource(resource.as_ref(), timestamp_us)?;
                Ok(frame.map(|frame| {
                    let changed = self.changes(&info, frame.width(), frame.height());
                    frame
                        .with_captured_at(presented_at(info.LastPresentTime))
                        .with_changed(changed)
                }))
            } else {
                Ok(None)
            }
        });
        if let Err(err) = self.duplication.ReleaseFrame() {
            return Err(map_error(err));
        }
        frame
    }

    /// Where the acquired frame changed the output, from Desktop
    /// Duplication's dirty rectangles and where its moves landed, over every
    /// frame it accumulated; `None` when it cannot say. Read before the
    /// frame is released.
    unsafe fn changes(
        &self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        width: u16,
        height: u16,
    ) -> Option<vnilink_media::Changes> {
        let bytes = info.TotalMetadataBufferSize as usize;
        if bytes == 0 {
            return None;
        }
        let move_size = std::mem::size_of::<DXGI_OUTDUPL_MOVE_RECT>();
        let mut moves = vec![DXGI_OUTDUPL_MOVE_RECT::default(); bytes / move_size + 1];
        let mut used = 0u32;
        self.duplication
            .GetFrameMoveRects(
                (moves.len() * move_size) as u32,
                moves.as_mut_ptr(),
                &mut used,
            )
            .ok()?;
        moves.truncate(used as usize / move_size);
        let rect_size = std::mem::size_of::<RECT>();
        let mut dirty = vec![RECT::default(); bytes / rect_size + 1];
        let mut used = 0u32;
        self.duplication
            .GetFrameDirtyRects(
                (dirty.len() * rect_size) as u32,
                dirty.as_mut_ptr(),
                &mut used,
            )
            .ok()?;
        dirty.truncate(used as usize / rect_size);
        let rects = dirty
            .iter()
            .chain(moves.iter().map(|moved| &moved.DestinationRect))
            .map(|rect| {
                [
                    i64::from(rect.left),
                    i64::from(rect.top),
                    i64::from(rect.right) - i64::from(rect.left),
                    i64::from(rect.bottom) - i64::from(rect.top),
                ]
            });
        Some(vnilink_media::Changes::of(
            rects,
            u32::from(width),
            u32::from(height),
        ))
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
        frame_size(desc.Width, desc.Height)?;
        let picture = self.pool.picture(&source).map_err(map_error)?;
        RawFrame::native(timestamp_us, Box::new(picture)).map(Some)
    }
}

/// Every output of every adapter, in DXGI's order, with what it shows.
unsafe fn list_outputs(
    factory: &IDXGIFactory1,
    startup: &vnilink::BackendStartup,
) -> Result<Vec<(IDXGIAdapter1, IDXGIOutput, crate::outputs::Output)>, Error> {
    let mut listed = Vec::new();
    for adapter_index in 0.. {
        check_startup(startup)?;
        let adapter = match factory.EnumAdapters1(adapter_index) {
            Ok(adapter) => adapter,
            Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(err) => return Err(map_error(err)),
        };
        for output_index in 0.. {
            let output = match adapter.EnumOutputs(output_index) {
                Ok(output) => output,
                Err(err) if err.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(err) => return Err(map_error(err)),
            };
            let desc = output.GetDesc().map_err(map_error)?;
            let bounds = desc.DesktopCoordinates;
            listed.push((
                adapter.clone(),
                output,
                crate::outputs::Output {
                    adapter: adapter_index,
                    output: output_index,
                    attached: desc.AttachedToDesktop.as_bool(),
                    primary: is_primary(desc.Monitor),
                    origin: (bounds.left, bounds.top),
                },
            ));
        }
    }
    Ok(listed)
}

/// Whether Windows names `monitor` the primary one.
fn is_primary(monitor: HMONITOR) -> bool {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid MONITORINFO with its size set.
    unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool()
        && info.dwFlags & MONITORINFOF_PRIMARY != 0
}

/// When the desktop was last presented, from Desktop Duplication's
/// `LastPresentTime`, a QueryPerformanceCounter value: a picture's latency
/// starts there, not when it was acquired. `Instant` counts on the same
/// clock on Windows, so the age converts exactly.
fn presented_at(present_qpc: i64) -> Option<std::time::Instant> {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (now, mut counter, mut frequency) = (std::time::Instant::now(), 0i64, 0i64);
    // SAFETY: both write one i64 each to valid locals.
    unsafe {
        QueryPerformanceCounter(&mut counter).ok()?;
        QueryPerformanceFrequency(&mut frequency).ok()?;
    }
    let ticks = u64::try_from(counter.checked_sub(present_qpc)?).ok()?;
    let frequency = u64::try_from(frequency).ok().filter(|hz| *hz > 0)?;
    let age = std::time::Duration::from_nanos(
        u64::try_from(u128::from(ticks) * 1_000_000_000 / u128::from(frequency)).ok()?,
    );
    now.checked_sub(age)
}

fn map_error(err: windows::core::Error) -> Error {
    let code = err.code();
    // Duplication loses access when the input desktop switches to the
    // sign-in screen, a lock screen or an elevation prompt.
    if code == DXGI_ERROR_ACCESS_LOST || code == E_ACCESSDENIED {
        return Error::SecureDesktop;
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
