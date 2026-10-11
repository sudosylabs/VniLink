use vnilink::HostCapture;
use vnilink::Quality;

use crate::desktop::{CapturePlan, CursorPolicy, DesktopKind, MonitorSize};
use crate::error::Error;

#[test]
fn capture_plan_is_one_even_monitor_without_a_cursor() {
    let quality = Quality::new(2_500, 30, 1919, 1080).unwrap();
    let plan = CapturePlan::for_display(1, quality).unwrap();
    assert_eq!(plan.display_id(), 1);
    assert_eq!((plan.width(), plan.height()), (1918, 1080));
    assert_eq!(plan.frame_rate(), 30);
    assert_eq!(plan.cursor(), CursorPolicy::Hidden);
    let shown = plan.with_cursor(CursorPolicy::InFrame);
    assert_eq!(shown.cursor(), CursorPolicy::InFrame);
    let other = CapturePlan::for_display(2, quality).unwrap();
    assert_eq!(other.display_id(), 2);
    assert_ne!(plan.display_id(), other.display_id());
    assert!(matches!(
        CapturePlan::for_display(0, quality),
        Err(Error::InvalidSetting("a display id is invalid"))
    ));
    let tiny = Quality::new(1, 1, 1, 1).unwrap();
    assert!(matches!(
        CapturePlan::for_display(1, tiny),
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
