use super::support::start_direct;
use crate::core::snapshot::{decode_payload, snapshot_admitted};
use crate::snapshot::{
    classify_stop, encode_bgra, fitted_size, jpeg_size, tap_is_fresh, validate_image, ScreenImage,
    ScreenSnapshot, SnapshotFailure, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_HEIGHT, MAX_SNAPSHOT_JOBS,
    MAX_SNAPSHOT_WIDTH, SNAPSHOT_MIN_INTERVAL,
};
use crate::Error;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// Found by the `snapshot_payload` fuzz target: an outcome with bytes after
/// it decoded, though the host writes none.
#[test]
fn an_outcome_with_trailing_bytes_is_malformed() {
    assert!(matches!(
        decode_payload(&[1, 1]),
        Ok(ScreenSnapshot::NeedsPermission)
    ));
    assert!(matches!(
        decode_payload(&[1, 1, 0x37, 0x37]),
        Err(Error::Malformed)
    ));
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
fn admission_bounds_jobs_and_spacing() {
    assert!(snapshot_admitted(0, None));
    assert!(snapshot_admitted(
        MAX_SNAPSHOT_JOBS - 1,
        Some(SNAPSHOT_MIN_INTERVAL)
    ));
    assert!(!snapshot_admitted(MAX_SNAPSHOT_JOBS, None));
    assert!(!snapshot_admitted(
        0,
        Some(SNAPSHOT_MIN_INTERVAL - Duration::from_millis(1))
    ));
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

#[test]
fn a_cancelled_request_is_not_a_missing_session() {
    let controller = start_direct();
    let cancel = Arc::new(AtomicBool::new(true));
    assert!(matches!(
        controller.request_screen_snapshot(1, 1, &cancel, false),
        Ok(ScreenSnapshot::Cancelled)
    ));
}
