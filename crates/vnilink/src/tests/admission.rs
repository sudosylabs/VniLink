use crate::session::SessionRole;

use super::support::*;

#[test]
fn the_offer_and_agreed_quality_travel_with_the_session() {
    use crate::protocol::Quality;

    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    // The 1x1 shutdown capture clamps the 1920x1080 offer, and a picture of
    // one pixel starts at the lowest rate, not the common start.
    let agreed = Quality::new(crate::bitrate::FLOOR_KBPS, 30, 1, 1).unwrap();
    assert_eq!(session.quality, Some(agreed));
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().quality,
        Some(agreed)
    );
    let updated = Quality::new(1_000, 15, 640, 480).unwrap();
    controller
        .update_session_quality(session.id, SessionRole::Controller, updated)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let current = host.snapshot().unwrap().incoming.map(|info| info.quality);
        if current == Some(Some(updated)) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("host did not apply the propagated quality: {current:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    controller.disconnect().unwrap();
    wait_closed(&controller, SessionRole::Controller);
}
