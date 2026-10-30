// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A picture's bytes: in memory, or still held by the platform (an
//! IOSurface on macOS) for an encoder that reads it there. The bytes are
//! screen content: do not log them.

use std::any::Any;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

use crate::error::Error;

/// A picture the platform holds, such as a ScreenCaptureKit IOSurface. An
/// encoder that knows the type reads it in place ([`Pixels::native`]); any
/// other reader gets it copied into memory once ([`Pixels`]'s `Deref`).
pub trait NativePicture: Send + Sync {
    /// Width and height in pixels.
    fn size(&self) -> (u16, u16);
    /// The picture as tightly packed BGRA, `width * 4` bytes a row.
    fn read_back(&self) -> Result<Vec<u8>, Error>;
    /// For an encoder to recognise its platform's pictures.
    fn as_any(&self) -> &dyn Any;
}

/// BGRA bytes shared by every frame and slot that holds them. Cloning does
/// not copy. A native picture is read into memory the first time its bytes
/// are asked for, once for all its holders.
#[derive(Clone)]
pub struct Pixels(Inner);

#[derive(Clone)]
enum Inner {
    Memory(Arc<Vec<u8>>),
    Native(Arc<NativeCell>),
}

struct NativeCell {
    picture: Box<dyn NativePicture>,
    read: OnceLock<Vec<u8>>,
}

impl Pixels {
    pub fn native(picture: Box<dyn NativePicture>) -> Self {
        Self(Inner::Native(Arc::new(NativeCell {
            picture,
            read: OnceLock::new(),
        })))
    }

    /// The platform's picture, when these bytes are one.
    pub fn native_picture(&self) -> Option<&dyn NativePicture> {
        match &self.0 {
            Inner::Memory(_) => None,
            Inner::Native(cell) => Some(cell.picture.as_ref()),
        }
    }
}

impl From<Vec<u8>> for Pixels {
    fn from(bytes: Vec<u8>) -> Self {
        Self(Inner::Memory(Arc::new(bytes)))
    }
}

impl Deref for Pixels {
    type Target = [u8];

    /// A native picture the platform can no longer read comes back black:
    /// readers of pixels have no way to fail, and an IOSurface that cannot
    /// be locked is not one ScreenCaptureKit delivered.
    fn deref(&self) -> &[u8] {
        match &self.0 {
            Inner::Memory(bytes) => bytes,
            Inner::Native(cell) => cell.read.get_or_init(|| {
                let (width, height) = cell.picture.size();
                cell.picture
                    .read_back()
                    .unwrap_or_else(|_| vec![0; usize::from(width) * usize::from(height) * 4])
            }),
        }
    }
}

impl fmt::Debug for Pixels {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Inner::Memory(bytes) => formatter
                .debug_struct("Pixels")
                .field("bytes", &bytes.len())
                .finish(),
            Inner::Native(cell) => formatter
                .debug_struct("Pixels")
                .field("native", &cell.picture.size())
                .finish(),
        }
    }
}
