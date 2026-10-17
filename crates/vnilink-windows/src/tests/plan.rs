use vnilink_media::{CapturePlan, Error, Quality};

use crate::plan::{
    frame_size, snapshot_prompts_for_consent, WindowsPlan, SECURE_DESKTOP_SUPPORTED,
    SNAPSHOT_DISPLAY_ID,
};

fn plan() -> CapturePlan {
    let quality = Quality::new(2_500, 30, 1920, 1080).unwrap();
    CapturePlan::for_display(4, quality).unwrap()
}

#[test]
fn the_plan_is_one_monitor_and_not_the_secure_desktop() {
    assert!(!SECURE_DESKTOP_SUPPORTED);
    let windows = WindowsPlan::from_capture(&plan(), 2_500).unwrap();
    assert_eq!(windows.display_id(), 4);
    assert_eq!((windows.width(), windows.height()), (1920, 1080));
    assert_eq!(windows.frame_rate(), 30);
    assert_eq!(windows.bitrate_bps(), 2_500_000);
    assert!(!windows.secure_desktop_supported());
}

#[test]
fn a_desktop_larger_than_one_frame_is_rejected() {
    assert!(matches!(
        frame_size(u32::from(u16::MAX) + 1, 1080),
        Err(Error::Unsupported("the desktop is larger than one frame"))
    ));
    assert!(matches!(
        frame_size(1920, 0),
        Err(Error::CaptureRejected(
            "Desktop Duplication returned an empty frame"
        ))
    ));
}

#[test]
fn a_screenshot_uses_the_first_output_and_does_not_ask_for_consent() {
    assert_eq!(SNAPSHOT_DISPLAY_ID, "0");
    assert!(!snapshot_prompts_for_consent(false));
    assert!(!snapshot_prompts_for_consent(true));
}

#[test]
fn an_absurd_bitrate_is_rejected() {
    assert!(matches!(
        WindowsPlan::from_capture(&plan(), u32::MAX),
        Err(Error::InvalidSetting(
            "bitrate does not fit in bits per second"
        ))
    ));
}
