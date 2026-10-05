use screencapturekit::stream::configuration::PixelFormat;
use vnilink_media::{CapturePlan, Error, Quality};

use crate::capture::ScreenPlan;

#[test]
fn the_capture_plan_is_one_bgra_monitor_and_does_not_start() {
    let quality = Quality::new(2_500, 30, 1280, 720).unwrap();
    let plan = CapturePlan::one_monitor(quality).unwrap();
    let screen = ScreenPlan::from_capture(&plan).unwrap();
    let config = screen.configuration();
    assert_eq!((config.width(), config.height()), (1280, 720));
    assert_eq!(config.pixel_format(), PixelFormat::BGRA);
    assert!(!config.shows_cursor());
    let interval = config.minimum_frame_interval();
    assert_eq!(interval.value, 1);
    assert_eq!(interval.timescale, 30);
    assert!(matches!(
        CapturePlan::for_monitor(1, quality),
        Err(Error::Unsupported("only one monitor is supported"))
    ));
}
