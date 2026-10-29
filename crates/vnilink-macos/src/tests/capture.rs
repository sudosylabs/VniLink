use screencapturekit::stream::configuration::PixelFormat;
use vnilink_media::{CapturePlan, Error, Quality};

use crate::capture::ScreenPlan;

#[test]
fn the_capture_plan_is_one_bgra_monitor_and_does_not_start() {
    let quality = Quality::new(2_500, 30, 1280, 720).unwrap();
    let plan = CapturePlan::for_display(1, quality).unwrap();
    let screen = ScreenPlan::from_capture(&plan).unwrap();
    let config = screen.configuration();
    assert_eq!((config.width(), config.height()), (1280, 720));
    assert_eq!(config.pixel_format(), PixelFormat::BGRA);
    assert!(!config.shows_cursor());
    let interval = config.minimum_frame_interval();
    assert_eq!(interval.value, 1);
    assert_eq!(interval.timescale, 30);
    let other = CapturePlan::for_display(2, quality).unwrap();
    assert_ne!(plan.display_id(), other.display_id());
    assert!(ScreenPlan::from_capture(&other).is_ok());
    assert!(matches!(
        CapturePlan::for_display(0, quality),
        Err(Error::InvalidSetting("a display id is invalid"))
    ));
    assert_eq!(crate::display_scale(3024, 1512.0), 200);
    assert_eq!(crate::display_scale(100, 0.0), 100);
    assert_eq!(crate::display_scale(100, 1.0), 100);
}

#[test]
fn a_snapshot_plan_stays_inside_the_thumbnail_and_hides_the_cursor() {
    let plan = ScreenPlan::snapshot(640, 360).unwrap();
    assert_eq!((plan.width(), plan.height()), (640, 360));
    assert_eq!(plan.frame_rate(), 5);
    assert!(!plan.shows_cursor());
    let config = plan.configuration();
    assert_eq!((config.width(), config.height()), (640, 360));
    assert!(!config.shows_cursor());
    assert!(ScreenPlan::snapshot(641, 360).is_err());
    assert!(ScreenPlan::snapshot(640, 361).is_err());
    assert!(ScreenPlan::snapshot(0, 1).is_err());
}

#[test]
fn cancelled_startup_refuses_capture_before_requesting_os_permission() {
    let quality = Quality::new(2_500, 30, 1280, 720).unwrap();
    let plan = ScreenPlan::from_capture(&CapturePlan::for_display(1, quality).unwrap()).unwrap();
    let startup = vnilink::BackendStartup::new(std::time::Duration::ZERO);
    assert!(matches!(
        crate::ScreenCapture::start(&plan, &startup),
        Err(Error::CaptureRejected(
            "desktop capture startup cancelled or expired"
        ))
    ));
}
