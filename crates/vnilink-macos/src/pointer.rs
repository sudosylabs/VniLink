// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The system pointer, read for a viewer that draws it itself.
//!
//! ScreenCaptureKit can leave the pointer out of the picture but does not
//! report it, so its position comes from Quartz and its shape from AppKit.
//! `+[NSCursor currentSystemCursor]` is deprecated and Apple says it will
//! return nil in a future macOS; then only the position is known, and the
//! host keeps the pointer in the picture instead.

use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGEvent, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};

/// The largest shape the engine carries, on either side.
const EDGE: f64 = vnilink::MAX_CURSOR_EDGE as f64;

/// Where the pointer is, in global display points from the top-left of the
/// main display, and its shape.
pub struct SystemPointer {
    pub x: f64,
    pub y: f64,
    pub image: Option<PointerImage>,
}

/// Premultiplied BGRA at one pixel per point, with the point under the mouse.
pub struct PointerImage {
    pub width: u16,
    pub height: u16,
    pub hotspot: (u16, u16),
    pub bgra: Vec<u8>,
}

/// The pointer now. Safe to call from any thread.
pub fn system_pointer() -> SystemPointer {
    objc2::rc::autoreleasepool(|_| {
        let location = CGEvent::location(CGEvent::new(None).as_deref());
        #[allow(deprecated)]
        let image = NSCursor::currentSystemCursor().and_then(|cursor| pointer_image(&cursor));
        SystemPointer {
            x: location.x,
            y: location.y,
            image,
        }
    })
}

fn pointer_image(cursor: &NSCursor) -> Option<PointerImage> {
    let picture = cursor.image();
    let size = picture.size();
    let (width, height) = (size.width.round(), size.height.round());
    if !(1.0..=EDGE).contains(&width) || !(1.0..=EDGE).contains(&height) {
        return None;
    }
    // SAFETY: a null proposed rect asks for the image at its own size; no
    // context or hints are passed.
    let source =
        unsafe { picture.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }?;
    let (width, height) = (width as usize, height as usize);
    let mut bgra = vec![0u8; width * height * 4];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: `bgra` holds `height` rows of `width * 4` bytes and outlives
    // the context, which is dropped before the buffer is read.
    let context = unsafe {
        CGBitmapContextCreate(
            bgra.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0,
        )
    }?;
    // The shape is drawn from its Retina representation down to points.
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    CGContext::draw_image(
        Some(&context),
        CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        ),
        Some(&source),
    );
    drop(context);
    // NSCursor measures the hot spot from the image's top-left corner.
    let hotspot = cursor.hotSpot();
    let clamp = |value: f64, size: usize| value.round().clamp(0.0, (size - 1) as f64) as u16;
    Some(PointerImage {
        width: width as u16,
        height: height as u16,
        hotspot: (clamp(hotspot.x, width), clamp(hotspot.y, height)),
        bgra,
    })
}
