//! One still screenshot of the host screen.
//!
//! The host admits this on the existing view permission, captures one fresh
//! frame, and encodes a JPEG that fits in 640 by 360 and 512 KiB. Pixels stay
//! in memory for that request. This module does not log them, store them, or
//! start a desktop, audio, input, or clipboard session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Largest encoded width. The host never enlarges a smaller source.
pub const MAX_SNAPSHOT_WIDTH: u16 = 640;
/// Largest encoded height.
pub const MAX_SNAPSHOT_HEIGHT: u16 = 360;
/// Largest JPEG this protocol will send.
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
/// Bound on one acquisition, including encode. Permission prompts are not included.
pub const SNAPSHOT_DEADLINE: Duration = Duration::from_secs(10);
/// Minimum time between admitted captures for one peer.
pub const SNAPSHOT_MIN_INTERVAL: Duration = Duration::from_secs(2);
/// Admitted captures across every peer, including the one being encoded.
pub const MAX_SNAPSHOT_JOBS: usize = 4;
pub(crate) const WIRE_KIND: u8 = 8;
/// Payload frame ceiling: the JPEG plus a short header.
pub(crate) const FRAME_LIMIT: usize = MAX_SNAPSHOT_BYTES + 128;
const DISPLAY_ID_LIMIT: usize = 64;
const QUALITIES: [u8; 6] = [75, 60, 45, 30, 15, 5];

/// Why a capture produced no image. Transport failures stay on [`crate::Error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFailure {
    /// The operating system has not granted reusable screen-capture consent,
    /// or a Wayland source would need a new selection dialog.
    NeedsPermission,
    /// Capture or encoding cannot produce a bounded image.
    Unavailable,
    TimedOut,
    /// The caller stopped the request. The image must not be delivered.
    Cancelled,
    /// No X11 display. A portal source has to be chosen on that computer.
    NeedsSourceSelection,
}

/// A JPEG of one acquired frame. `captured_unix_ms` is the acquisition time,
/// not the time the bytes were encoded or received.
pub struct ScreenImage {
    pub jpeg: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub captured_unix_ms: u64,
    pub display_id: String,
}

impl std::fmt::Debug for ScreenImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.jpeg.len())
            .field("captured_unix_ms", &self.captured_unix_ms)
            .field("display_id", &self.display_id)
            .finish()
    }
}

/// Whether a screenshot would be admitted. A probe does not capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenAccess {
    Allowed,
    Denied,
    Unavailable,
}

/// Outcome of one admitted snapshot. A refused admission is [`crate::Error`],
/// including [`crate::Error::Busy`] and [`crate::Error::Unauthorized`].
#[derive(Debug)]
pub enum ScreenSnapshot {
    Ready(ScreenImage),
    NeedsPermission,
    NeedsSourceSelection,
    Unavailable,
    TimedOut,
    /// The caller cancelled. This is not an absent session.
    Cancelled,
}

/// One-shot capture installed by the native backend. Registering it starts nothing.
pub trait HostSnapshotFactory: Send + Sync {
    fn capture(
        &self,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenImage, SnapshotFailure>;
}

/// Whether an attested desktop frame is still young enough to call just captured.
pub fn tap_is_fresh(at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(at) <= Duration::from_secs(2)
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Classify a stopped capture. Cancel wins over the deadline.
pub fn classify_stop(cancel: &AtomicBool, deadline: Instant) -> SnapshotFailure {
    if cancelled(cancel) {
        SnapshotFailure::Cancelled
    } else if Instant::now() >= deadline {
        SnapshotFailure::TimedOut
    } else {
        SnapshotFailure::Unavailable
    }
}

/// Output size that preserves aspect ratio, never exceeds the snapshot bound, and never enlarges.
pub fn fitted_size(src_width: u32, src_height: u32) -> Option<(u16, u16)> {
    if src_width == 0 || src_height == 0 {
        return None;
    }
    let max_width = u32::from(MAX_SNAPSHOT_WIDTH);
    let max_height = u32::from(MAX_SNAPSHOT_HEIGHT);
    if src_width <= max_width && src_height <= max_height {
        return Some((src_width as u16, src_height as u16));
    }
    let width_bound = u64::from(src_width) * u64::from(max_height);
    let height_bound = u64::from(src_height) * u64::from(max_width);
    let (width, height) = if width_bound >= height_bound {
        let height = (u64::from(src_height) * u64::from(max_width) / u64::from(src_width))
            .clamp(1, u64::from(max_height));
        (u64::from(max_width), height)
    } else {
        let width = (u64::from(src_width) * u64::from(max_height) / u64::from(src_height))
            .clamp(1, u64::from(max_width));
        (width, u64::from(max_height))
    };
    Some((width as u16, height as u16))
}

pub(crate) fn validate_image(image: &ScreenImage) -> Result<(), SnapshotFailure> {
    if image.width == 0
        || image.height == 0
        || image.width > MAX_SNAPSHOT_WIDTH
        || image.height > MAX_SNAPSHOT_HEIGHT
        || image.display_id.len() > DISPLAY_ID_LIMIT
        || !jpeg_envelope(&image.jpeg)
        || jpeg_size(&image.jpeg) != Some((image.width, image.height))
    {
        return Err(SnapshotFailure::Unavailable);
    }
    Ok(())
}

/// SOF dimensions. This does not decode the entropy; it rejects a header that
/// disagrees with the image the bytes actually describe.
pub fn jpeg_size(jpeg: &[u8]) -> Option<(u16, u16)> {
    if jpeg.len() < 4 || jpeg[0] != 0xff || jpeg[1] != 0xd8 {
        return None;
    }
    let mut index = 2;
    while index + 1 < jpeg.len() {
        if jpeg[index] != 0xff {
            return None;
        }
        while index < jpeg.len() && jpeg[index] == 0xff {
            index += 1;
        }
        if index >= jpeg.len() {
            return None;
        }
        let marker = jpeg[index];
        index += 1;
        if marker == 0xd8 || marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if marker == 0xd9 || marker == 0xda {
            return None;
        }
        if index + 1 >= jpeg.len() {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([jpeg[index], jpeg[index + 1]]));
        if len < 2 || index + len > jpeg.len() {
            return None;
        }
        if matches!(marker, 0xc0 | 0xc1 | 0xc2) {
            if len < 7 {
                return None;
            }
            let height = u16::from_be_bytes([jpeg[index + 3], jpeg[index + 4]]);
            let width = u16::from_be_bytes([jpeg[index + 5], jpeg[index + 6]]);
            return Some((width, height));
        }
        index += len;
    }
    None
}

fn jpeg_envelope(jpeg: &[u8]) -> bool {
    jpeg.len() >= 4
        && jpeg.len() <= MAX_SNAPSHOT_BYTES
        && jpeg[0] == 0xff
        && jpeg[1] == 0xd8
        && jpeg[jpeg.len() - 2] == 0xff
        && jpeg[jpeg.len() - 1] == 0xd9
}

/// Scale strided BGRA into a bounded JPEG. Channel order is B, G, R, A.
pub fn encode_bgra(
    width: u16,
    height: u16,
    stride: usize,
    bgra: &[u8],
    captured_unix_ms: u64,
    display_id: &str,
) -> Result<ScreenImage, SnapshotFailure> {
    if display_id.len() > DISPLAY_ID_LIMIT {
        return Err(SnapshotFailure::Unavailable);
    }
    let (out_width, out_height) =
        fitted_size(u32::from(width), u32::from(height)).ok_or(SnapshotFailure::Unavailable)?;
    let row = usize::from(width)
        .checked_mul(4)
        .ok_or(SnapshotFailure::Unavailable)?;
    if stride < row {
        return Err(SnapshotFailure::Unavailable);
    }
    let needed = stride
        .checked_mul(usize::from(height))
        .ok_or(SnapshotFailure::Unavailable)?;
    if bgra.len() < needed {
        return Err(SnapshotFailure::Unavailable);
    }
    let packed = scale_bgra(width, height, stride, bgra, out_width, out_height);
    let mut jpeg = Vec::new();
    for quality in QUALITIES {
        jpeg.clear();
        let encoder = jpeg_encoder::Encoder::new(&mut jpeg, quality);
        if encoder
            .encode(
                &packed,
                out_width,
                out_height,
                jpeg_encoder::ColorType::Bgra,
            )
            .is_err()
        {
            continue;
        }
        if jpeg_envelope(&jpeg) {
            return Ok(ScreenImage {
                jpeg,
                width: out_width,
                height: out_height,
                captured_unix_ms,
                display_id: display_id.to_owned(),
            });
        }
    }
    Err(SnapshotFailure::Unavailable)
}

fn scale_bgra(
    src_width: u16,
    src_height: u16,
    stride: usize,
    bgra: &[u8],
    out_width: u16,
    out_height: u16,
) -> Vec<u8> {
    let mut out = vec![0u8; usize::from(out_width) * usize::from(out_height) * 4];
    for y in 0..out_height {
        let src_y = (u32::from(y) * u32::from(src_height) / u32::from(out_height))
            .min(u32::from(src_height) - 1) as usize;
        let row = &bgra[src_y * stride..];
        for x in 0..out_width {
            let src_x = (u32::from(x) * u32::from(src_width) / u32::from(out_width))
                .min(u32::from(src_width) - 1) as usize;
            let dest = (usize::from(y) * usize::from(out_width) + usize::from(x)) * 4;
            out[dest..dest + 4].copy_from_slice(&row[src_x * 4..src_x * 4 + 4]);
        }
    }
    out
}

pub fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire)
}

pub fn watch_cancel(deadline: Instant, cancel: &AtomicBool) -> Result<(), SnapshotFailure> {
    if cancelled(cancel) {
        return Err(SnapshotFailure::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(SnapshotFailure::TimedOut);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn solid(blue: u8, green: u8, red: u8, width: u16, height: u16, stride: usize) -> Vec<u8> {
        let mut pixels = vec![0x11u8; stride * usize::from(height)];
        for y in 0..usize::from(height) {
            for x in 0..usize::from(width) {
                let index = y * stride + x * 4;
                pixels[index] = blue;
                pixels[index + 1] = green;
                pixels[index + 2] = red;
                pixels[index + 3] = 255;
            }
        }
        pixels
    }

    fn decoded(jpeg: &[u8]) -> (u16, u16, Vec<u8>) {
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
        let pixels = decoder.decode().expect("jpeg");
        let info = decoder.info().expect("jpeg info");
        (info.width, info.height, pixels)
    }

    fn near(actual: u8, expected: u8) -> bool {
        actual.abs_diff(expected) <= 12
    }

    #[test]
    fn fitted_sizes_keep_aspect_and_never_enlarge() {
        assert_eq!(fitted_size(2, 2), Some((2, 2)));
        assert_eq!(fitted_size(1920, 1080), Some((640, 360)));
        assert_eq!(fitted_size(1080, 1920), Some((202, 360)));
        assert_eq!(fitted_size(3440, 1440), Some((640, 267)));
        assert_eq!(fitted_size(0, 10), None);
        let (width, height) = fitted_size(8000, 1000).unwrap();
        assert!(width <= MAX_SNAPSHOT_WIDTH && height <= MAX_SNAPSHOT_HEIGHT);
        assert!(width < 8000 && height < 1000);
    }

    #[test]
    fn encode_reads_strided_bgra_and_stays_inside_the_byte_cap() {
        let red = solid(0, 0, 255, 8, 8, 8 * 4 + 16);
        let image = encode_bgra(8, 8, 8 * 4 + 16, &red, 1_700_000_000_000, "panel").unwrap();
        assert!(image.jpeg.len() <= MAX_SNAPSHOT_BYTES);
        assert_eq!((image.width, image.height), (8, 8));
        let (width, height, pixels) = decoded(&image.jpeg);
        assert_eq!((width, height), (8, 8));
        assert!(pixels
            .chunks_exact(3)
            .all(|pixel| near(pixel[0], 255) && near(pixel[1], 0) && near(pixel[2], 0)));

        let green = solid(0, 255, 0, 8, 8, 32);
        let image = encode_bgra(8, 8, 32, &green, 5, "g").unwrap();
        let (_, _, pixels) = decoded(&image.jpeg);
        assert!(pixels
            .chunks_exact(3)
            .all(|pixel| near(pixel[0], 0) && near(pixel[1], 255) && near(pixel[2], 0)));

        let blue = solid(255, 0, 0, 8, 8, 32);
        let image = encode_bgra(8, 8, 32, &blue, 5, "b").unwrap();
        let (_, _, pixels) = decoded(&image.jpeg);
        assert!(pixels
            .chunks_exact(3)
            .all(|pixel| near(pixel[0], 0) && near(pixel[1], 0) && near(pixel[2], 255)));

        let wide = solid(0, 0, 255, 1920, 1080, 1920 * 4);
        let image = encode_bgra(1920, 1080, 1920 * 4, &wide, 9, "wide").unwrap();
        assert_eq!((image.width, image.height), (640, 360));
        assert!(image.jpeg.len() <= MAX_SNAPSHOT_BYTES);
        assert!(encode_bgra(1, 1, 2, &[0, 0, 0, 0], 0, "short").is_err());
        assert!(encode_bgra(1, 1, 4, &[0, 0, 0, 0], 0, &"x".repeat(65)).is_err());
    }

    #[test]
    fn jpeg_dimensions_must_match_the_declared_image() {
        let image = encode_bgra(8, 8, 32, &solid(0, 0, 255, 8, 8, 32), 9, "panel").unwrap();
        assert_eq!(jpeg_size(&image.jpeg), Some((image.width, image.height)));
        assert!(validate_image(&image).is_ok());
        let mut wrong = ScreenImage {
            jpeg: image.jpeg.clone(),
            width: 9,
            height: image.height,
            captured_unix_ms: image.captured_unix_ms,
            display_id: image.display_id.clone(),
        };
        assert!(validate_image(&wrong).is_err());
        wrong.width = image.width;
        wrong.jpeg[0] = 0;
        assert!(validate_image(&wrong).is_err());
    }

    #[test]
    fn classify_stop_lets_cancel_win_and_a_tap_expires_after_two_seconds() {
        let cancel = AtomicBool::new(true);
        let deadline = Instant::now() + Duration::from_secs(5);
        assert_eq!(classify_stop(&cancel, deadline), SnapshotFailure::Cancelled);
        cancel.store(false, Ordering::Release);
        assert_eq!(
            classify_stop(&cancel, Instant::now() - Duration::from_millis(1)),
            SnapshotFailure::TimedOut
        );
        assert_eq!(
            classify_stop(&cancel, deadline),
            SnapshotFailure::Unavailable
        );
        let now = Instant::now();
        assert!(tap_is_fresh(now, now + Duration::from_secs(2)));
        assert!(!tap_is_fresh(
            now,
            now + Duration::from_secs(2) + Duration::from_millis(1)
        ));
    }
}
