//! X11 screen capture for the current session.
//!
//! [`X11Capture::open`] connects to the display and reads the root window.
//! Tests use [`raw_frame_from_ximage`] and do not connect. This path does not
//! see a Wayland session. Pixel bytes are screen content: do not log them.

use vnilink_media::{Error, RawFrame};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder};
use x11rb::rust_connection::RustConnection;

/// Which X server and screen to read. `display_name` `None` uses the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Plan {
    pub display_name: Option<String>,
    pub screen: u32,
}

impl X11Plan {
    pub fn current_session() -> Self {
        Self {
            display_name: None,
            screen: 0,
        }
    }
}

/// One ZPixmap image. Only 32-bit little-endian BGRA is accepted.
pub struct X11Image<'a> {
    pub width: u16,
    pub height: u16,
    pub stride: usize,
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub lsb_first: bool,
    pub bytes: &'a [u8],
}

pub fn raw_frame_from_ximage(image: X11Image<'_>, timestamp_us: u64) -> Result<RawFrame, Error> {
    if image.depth != 24 || image.bits_per_pixel != 32 || !image.lsb_first {
        return Err(Error::Unsupported("this X11 pixel format"));
    }
    if image.width == 0 || image.height == 0 {
        return Err(Error::InvalidFrame(
            "frame width and height must be non-zero",
        ));
    }
    RawFrame::with_stride(
        image.width,
        image.height,
        image.stride,
        timestamp_us,
        image.bytes.to_vec(),
    )
}

/// Live X11 capture. Constructing it opens the display.
pub struct X11Capture {
    connection: RustConnection,
    root: u32,
    width: u16,
    height: u16,
    depth: u8,
    bits_per_pixel: u8,
    stride: usize,
    lsb_first: bool,
}

impl X11Capture {
    pub fn open(plan: &X11Plan) -> Result<Self, Error> {
        let (connection, default_screen) = x11rb::connect(plan.display_name.as_deref())
            .map_err(|_| Error::CaptureRejected("the X11 display could not be opened"))?;
        let screen_index = if plan.screen == 0 {
            default_screen
        } else {
            plan.screen as usize
        };
        let setup = connection.setup();
        let screen = setup
            .roots
            .get(screen_index)
            .ok_or(Error::Unsupported("only one monitor is supported"))?;
        let geometry = connection
            .get_geometry(screen.root)
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?
            .reply()
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?;
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .find(|format| format.depth == geometry.depth)
            .map(|format| format.bits_per_pixel)
            .unwrap_or(0);
        let pad = usize::from(setup.bitmap_format_scanline_pad.max(8) / 8);
        let row = usize::from(geometry.width) * usize::from(bits_per_pixel) / 8;
        let stride = row.div_ceil(pad) * pad;
        let root = screen.root;
        let lsb_first = setup.image_byte_order == ImageOrder::LSB_FIRST;
        Ok(Self {
            connection,
            root,
            width: geometry.width,
            height: geometry.height,
            depth: geometry.depth,
            bits_per_pixel,
            stride,
            lsb_first,
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        let reply = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                0,
                0,
                self.width,
                self.height,
                !0,
            )
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?
            .reply()
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?;
        raw_frame_from_ximage(
            X11Image {
                width: self.width,
                height: self.height,
                stride: self.stride,
                depth: self.depth,
                bits_per_pixel: self.bits_per_pixel,
                lsb_first: self.lsb_first,
                bytes: &reply.data,
            },
            timestamp_us,
        )
    }
}
