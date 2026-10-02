use vnilink::HostCapture;
use vnilink::Quality;

use crate::desktop::{CapturePlan, CursorPolicy, DesktopKind, MonitorSize};
use crate::error::Error;
use crate::frame::{encoded_size, scale_bgra, RawFrame};
use crate::nv12::bgra_to_nv12;
fn solid(width: u16, height: u16, pixel: [u8; 4]) -> RawFrame {
    let mut bgra = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        bgra.extend_from_slice(&pixel);
    }
    RawFrame::bgra(width, height, 10, bgra).unwrap()
}

#[test]
fn raw_frames_reject_empty_and_short_buffers() {
    assert!(matches!(
        RawFrame::bgra(0, 2, 0, vec![]),
        Err(Error::InvalidFrame(
            "frame width and height must be non-zero"
        ))
    ));
    assert!(matches!(
        RawFrame::bgra(2, 2, 0, vec![0; 8]),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert!(matches!(
        RawFrame::with_stride(2, 1, 4, 0, vec![0; 8]),
        Err(Error::InvalidFrame("frame stride is shorter than one row"))
    ));
    let frame = solid(2, 1, [1, 2, 3, 4]);
    let rendered = format!("{frame:?}");
    assert!(rendered.contains("width"));
    assert!(!rendered.contains("[1, 2, 3, 4]"));
}

#[test]
fn encoded_size_keeps_aspect_and_stays_even() {
    assert_eq!(encoded_size(320, 180, 160, 120).unwrap(), (160, 90));
    assert_eq!(encoded_size(1920, 1080, 1280, 720).unwrap(), (1280, 720));
    assert_eq!(encoded_size(100, 100, 1919, 1080).unwrap(), (100, 100));
    assert_eq!(encoded_size(101, 51, 200, 200).unwrap(), (100, 50));
    assert_eq!(encoded_size(3, 3, 3, 3).unwrap(), (2, 2));
    assert!(matches!(
        encoded_size(1, 1, 1, 1),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
    assert!(matches!(
        encoded_size(0, 10, 10, 10),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
}

#[test]
fn scale_uses_nearest_neighbor_and_ignores_row_padding() {
    let mut bgra = vec![0u8; 2 * 12];
    bgra[0..4].copy_from_slice(&[1, 0, 0, 255]);
    bgra[4..8].copy_from_slice(&[2, 0, 0, 255]);
    bgra[8..12].copy_from_slice(&[9, 9, 9, 9]);
    bgra[12..16].copy_from_slice(&[3, 0, 0, 255]);
    bgra[16..20].copy_from_slice(&[4, 0, 0, 255]);
    let frame = RawFrame::with_stride(2, 2, 12, 7, bgra).unwrap();
    let scaled = scale_bgra(&frame, 1, 1).unwrap();
    assert_eq!(scaled.timestamp_us(), 7);
    assert_eq!(scaled.pixels(), &[1, 0, 0, 255]);
    assert_eq!(
        scale_bgra(&frame, 2, 2).unwrap().pixels()[0..8],
        frame.row(0)[0..8]
    );
    assert!(matches!(
        scale_bgra(&frame, 0, 2),
        Err(Error::InvalidFrame(
            "frame width and height must be non-zero"
        ))
    ));
}

#[test]
fn nv12_uses_limited_range_and_rejects_odd_sizes() {
    let black = solid(2, 2, [0, 0, 0, 255]);
    let nv12 = bgra_to_nv12(&black).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 16));
    assert!(nv12.uv_plane().iter().all(|sample| *sample == 128));
    let white = solid(2, 2, [255, 255, 255, 255]);
    let nv12 = bgra_to_nv12(&white).unwrap();
    assert!(nv12.y_plane().iter().all(|sample| *sample == 235));
    assert_eq!(nv12.uv_plane().len(), 2);
    let rendered = format!("{nv12:?}");
    assert!(!rendered.contains("235"));
    assert!(matches!(
        bgra_to_nv12(&solid(3, 2, [0, 0, 0, 255])),
        Err(Error::InvalidFrame("NV12 width and height must be even"))
    ));
}

#[test]
fn capture_plan_is_one_even_monitor_without_a_cursor() {
    let quality = Quality::new(2_500, 30, 1919, 1080).unwrap();
    let plan = CapturePlan::one_monitor(quality).unwrap();
    assert_eq!(plan.monitor_index(), 0);
    assert_eq!((plan.width(), plan.height()), (1918, 1080));
    assert_eq!(plan.frame_rate(), 30);
    assert_eq!(plan.cursor(), CursorPolicy::Hidden);
    let shown = plan.with_cursor(CursorPolicy::InFrame);
    assert_eq!(shown.cursor(), CursorPolicy::InFrame);
    assert!(matches!(
        CapturePlan::for_monitor(1, quality),
        Err(Error::Unsupported("only one monitor is supported"))
    ));
    let tiny = Quality::new(1, 1, 1, 1).unwrap();
    assert!(matches!(
        CapturePlan::one_monitor(tiny),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
    let size = MonitorSize::from_plan(&plan);
    assert_eq!(HostCapture::monitor(&size), (1918, 1080));
}

#[test]
fn this_process_names_its_desktop_and_its_limit() {
    let kinds = DesktopKind::on_this_process();
    assert!(!kinds.is_empty());
    for kind in kinds {
        assert!(!kind.limitation().is_empty());
    }
    #[cfg(target_os = "macos")]
    assert_eq!(kinds, &[DesktopKind::MacOs]);
    #[cfg(target_os = "windows")]
    assert_eq!(kinds, &[DesktopKind::Windows]);
    #[cfg(target_os = "linux")]
    assert_eq!(kinds, &[DesktopKind::LinuxX11, DesktopKind::LinuxWayland]);
}
