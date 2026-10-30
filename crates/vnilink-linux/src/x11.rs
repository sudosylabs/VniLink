// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! X11 screen capture for the current session.
//!
//! [`X11Capture::open`] connects to the display and reads the root window.
//! Tests use [`raw_frame_from_ximage`] and do not connect. This path does not
//! see a Wayland session. Pixel bytes are screen content: do not log them.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use vnilink_media::{Error, RawFrame};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, GetCursorImageReply};
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

/// One RandR CRTC. `rotation` is the protocol bitfield, not degrees.
/// `mode` zero means the CRTC is disabled and is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrtcGeometry {
    pub id: u32,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub rotation: u16,
    pub mode: u32,
}

/// Active CRTCs as one topology. Disabled CRTCs are skipped. An empty list
/// is an error so the caller can fall back. A list that fails bounds fails
/// entirely and must not be published in part.
pub fn displays_from_crtcs(
    revision: u64,
    crtcs: &[CrtcGeometry],
) -> Result<Vec<vnilink::DisplayInfo>, Error> {
    let mut displays = Vec::new();
    for crtc in crtcs {
        if crtc.mode == 0 || crtc.width == 0 || crtc.height == 0 || crtc.id == 0 {
            continue;
        }
        displays.push(vnilink::DisplayInfo {
            id: u64::from(crtc.id),
            origin_x: i32::from(crtc.x),
            origin_y: i32::from(crtc.y),
            width: crtc.width,
            height: crtc.height,
            rotation_deg: rotation_degrees(crtc.rotation),
            scale_percent: 100,
            revision,
        });
    }
    if displays.is_empty() {
        return Err(Error::CaptureRejected("no display is available"));
    }
    vnilink::validate_topology(&displays)
        .map_err(|_| Error::InvalidSetting("a display size is out of range"))?;
    Ok(displays)
}

fn rotation_degrees(bits: u16) -> u16 {
    if bits & 2 != 0 {
        90
    } else if bits & 4 != 0 {
        180
    } else if bits & 8 != 0 {
        270
    } else {
        0
    }
}

/// Read rectangle for one CRTC, or None when the CRTC is disabled, empty,
/// oversized or has no id. Origins stay server-relative (negative for
/// left/above-primary monitors); an unreadable rect surfaces as a capture
/// error from the read itself.
pub fn crop_rect(crtc: &CrtcGeometry) -> Option<(i16, i16, u16, u16)> {
    if crtc.id == 0 || crtc.mode == 0 || crtc.width == 0 || crtc.height == 0 {
        return None;
    }
    if crtc.width > vnilink::MAX_DISPLAY_EDGE || crtc.height > vnilink::MAX_DISPLAY_EDGE {
        return None;
    }
    Some((crtc.x, crtc.y, crtc.width, crtc.height))
}

/// ZPixmap row stride for a capture width, in bytes.
pub fn stride_for(width: u16, bits_per_pixel: u8, scanline_pad: usize) -> usize {
    let row = usize::from(width) * usize::from(bits_per_pixel) / 8;
    row.div_ceil(scanline_pad.max(1)) * scanline_pad.max(1)
}

/// Root-relative cursor position in capture coordinates. A cursor outside
/// the crop keeps its offset position; viewers clip it.
pub fn offset_cursor(position: (i32, i32), origin: (i16, i16)) -> (i32, i32) {
    (
        position.0 - i32::from(origin.0),
        position.1 - i32::from(origin.1),
    )
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
    origin_x: i16,
    origin_y: i16,
    crtc: Option<u32>,
    depth: u8,
    bits_per_pixel: u8,
    stride: usize,
    lsb_first: bool,
    cursor_metadata: bool,
    cursor_cache: Mutex<Option<(u32, Arc<vnilink::CursorImage>)>>,
    /// Topology revision this crop attests. The supervisor sets it from
    /// the published topology when the capture opens and re-attests an
    /// unchanged capture on later passes, so frames stamp the revision
    /// their pixels were cropped for instead of the current one.
    revision: Arc<AtomicU64>,
}

struct ProbedScreen {
    connection: RustConnection,
    root: u32,
    width: u16,
    height: u16,
    depth: u8,
    bits_per_pixel: u8,
    scanline_pad: usize,
    lsb_first: bool,
}

fn probe(plan: &X11Plan) -> Result<ProbedScreen, Error> {
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
    Ok(ProbedScreen {
        root: screen.root,
        width: geometry.width,
        height: geometry.height,
        depth: geometry.depth,
        bits_per_pixel,
        scanline_pad: usize::from(setup.bitmap_format_scanline_pad.max(8) / 8),
        lsb_first: setup.image_byte_order == ImageOrder::LSB_FIRST,
        connection,
    })
}

fn query_cursor_metadata(connection: &RustConnection) -> bool {
    connection
        .xfixes_query_version(2, 0)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some_and(|version| version.major_version >= 2)
}

fn crtc_geometries(connection: &RustConnection, root: u32) -> Result<Vec<CrtcGeometry>, Error> {
    use x11rb::protocol::randr::ConnectionExt as _;
    let resources = connection
        .randr_get_screen_resources_current(root)
        .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?
        .reply()
        .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?;
    let mut crtcs = Vec::with_capacity(resources.crtcs.len());
    for crtc in resources.crtcs {
        let info = connection
            .randr_get_crtc_info(crtc, resources.config_timestamp)
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?
            .reply()
            .map_err(|_| Error::CaptureRejected("the X11 display could not be read"))?;
        crtcs.push(CrtcGeometry {
            id: crtc,
            x: info.x,
            y: info.y,
            width: info.width,
            height: info.height,
            rotation: u16::from(info.rotation),
            mode: info.mode,
        });
    }
    Ok(crtcs)
}

impl X11Capture {
    fn from_probed(
        probed: ProbedScreen,
        origin: (i16, i16),
        size: (u16, u16),
        crtc: Option<u32>,
    ) -> Self {
        let stride = stride_for(size.0, probed.bits_per_pixel, probed.scanline_pad);
        let cursor_metadata = query_cursor_metadata(&probed.connection);
        Self {
            connection: probed.connection,
            root: probed.root,
            width: size.0,
            height: size.1,
            origin_x: origin.0,
            origin_y: origin.1,
            crtc,
            depth: probed.depth,
            bits_per_pixel: probed.bits_per_pixel,
            stride,
            lsb_first: probed.lsb_first,
            cursor_metadata,
            cursor_cache: Mutex::new(None),
            revision: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn open(plan: &X11Plan) -> Result<Self, Error> {
        let probed = probe(plan)?;
        let size = (probed.width, probed.height);
        Ok(Self::from_probed(probed, (0, 0), size, None))
    }

    /// Capture one RandR CRTC instead of the whole root window. Rotation is
    /// reported in the topology while pixels stay framebuffer-oriented.
    /// Unknown, disabled or unreadable CRTCs are refused so the supervisor
    /// can fall back or leave the display dark.
    pub fn open_display(plan: &X11Plan, crtc_id: u32) -> Result<Self, Error> {
        if crtc_id == 0 {
            return Err(Error::InvalidSetting("a display id is invalid"));
        }
        let probed = probe(plan)?;
        let crtcs = crtc_geometries(&probed.connection, probed.root)?;
        let crtc = crtcs
            .iter()
            .find(|crtc| crtc.id == crtc_id)
            .ok_or(Error::CaptureRejected("this display cannot be captured"))?;
        let (x, y, width, height) =
            crop_rect(crtc).ok_or(Error::CaptureRejected("this display cannot be captured"))?;
        Ok(Self::from_probed(
            probed,
            (x, y),
            (width, height),
            Some(crtc_id),
        ))
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The captured CRTC id, or None for the whole root window.
    pub fn crtc(&self) -> Option<u32> {
        self.crtc
    }

    /// Topology revision this crop attests. 0 until the supervisor
    /// attests the capture to a published revision.
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Attest this crop to a published topology revision.
    pub fn set_revision(&self, revision: u64) {
        self.revision.store(revision, Ordering::Release);
    }

    /// Share the attestation with the owner registry so later passes can
    /// re-attest an unchanged capture without reopening it.
    pub fn shared_revision(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.revision)
    }

    pub fn separate_cursor(&self) -> bool {
        self.cursor_metadata
    }

    /// XFixes supplies lossless premultiplied ARGB plus the root-relative
    /// position and hotspot. Screen pixels themselves never contain a cursor.
    pub fn cursor(&self) -> Option<vnilink::CursorState> {
        if !self.cursor_metadata {
            return None;
        }
        let reply = self
            .connection
            .xfixes_get_cursor_image()
            .ok()?
            .reply()
            .ok()?;
        let mut cache = self
            .cursor_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let image = if let Some((_, image)) = cache
            .as_ref()
            .filter(|(serial, _)| *serial == reply.cursor_serial)
        {
            Arc::clone(image)
        } else {
            let image = Arc::new(cursor_image(&reply)?);
            *cache = Some((reply.cursor_serial, Arc::clone(&image)));
            image
        };
        let visible = image.pixels().chunks_exact(4).any(|pixel| pixel[3] != 0);
        Some(vnilink::CursorState {
            position: offset_cursor(
                (i32::from(reply.x), i32::from(reply.y)),
                (self.origin_x, self.origin_y),
            ),
            visible,
            image: Some(image),
        })
    }

    /// Connected CRTCs, or the root window when RandR is missing or empty.
    /// A CRTC list that fails the topology bounds is an error. The root
    /// window is not substituted for a display RandR already named.
    pub fn displays(&self, revision: u64) -> Result<Vec<vnilink::DisplayInfo>, Error> {
        match self.randr_displays(revision) {
            Ok(displays) => Ok(displays),
            Err(Error::InvalidSetting(message)) => Err(Error::InvalidSetting(message)),
            Err(_) => Ok(vec![self.root_display(revision)]),
        }
    }

    fn root_display(&self, revision: u64) -> vnilink::DisplayInfo {
        vnilink::DisplayInfo {
            id: u64::from(self.root),
            origin_x: 0,
            origin_y: 0,
            width: self.width,
            height: self.height,
            rotation_deg: 0,
            scale_percent: 100,
            revision,
        }
    }

    fn randr_displays(&self, revision: u64) -> Result<Vec<vnilink::DisplayInfo>, Error> {
        displays_from_crtcs(revision, &crtc_geometries(&self.connection, self.root)?)
    }

    pub fn next_frame(&self, timestamp_us: u64) -> Result<RawFrame, Error> {
        let reply = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                self.origin_x,
                self.origin_y,
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

pub(crate) fn cursor_image(reply: &GetCursorImageReply) -> Option<vnilink::CursorImage> {
    if reply.width == 0
        || reply.height == 0
        || reply.width > vnilink::MAX_CURSOR_EDGE
        || reply.height > vnilink::MAX_CURSOR_EDGE
        || reply.cursor_image.len() != usize::from(reply.width) * usize::from(reply.height)
    {
        return None;
    }
    let pixels = reply
        .cursor_image
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .collect();
    vnilink::CursorImage::new(reply.width, reply.height, (reply.xhot, reply.yhot), pixels).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crtc(id: u32, rotation: u16) -> CrtcGeometry {
        CrtcGeometry {
            id,
            x: (id as i16 - 1) * 1920,
            y: 0,
            width: 1920,
            height: 1080,
            rotation,
            mode: 1,
        }
    }

    /// SPEC14: RandR rotation bitfields map to physical degrees, and
    /// disabled CRTCs are skipped without failing the topology.
    #[test]
    fn crtc_rotation_maps_to_physical_degrees() {
        let displays = displays_from_crtcs(
            3,
            &[
                crtc(1, 1),
                crtc(2, 2),
                crtc(3, 4),
                crtc(4, 8),
                CrtcGeometry {
                    id: 5,
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    rotation: 0,
                    mode: 0,
                },
            ],
        )
        .unwrap();
        let degrees: Vec<(u64, u16)> = displays.iter().map(|d| (d.id, d.rotation_deg)).collect();
        assert_eq!(degrees, vec![(1, 0), (2, 90), (3, 180), (4, 270)]);
        assert!(displays.iter().all(|d| d.revision == 3));
    }
}
