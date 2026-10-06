use vnilink::CoreEvent;

use crate::{suggest_quality, Congestion, Error, Quality, RateController};

fn updated(quality: Quality) -> CoreEvent {
    CoreEvent::QualityUpdated { id: 7, quality }
}

#[test]
fn settings_follow_the_quality_limit_at_an_even_size() {
    let control = RateController::open(64, 64, Quality::new(500, 30, 32, 32).unwrap()).unwrap();
    let settings = control.settings();
    assert_eq!((settings.width, settings.height), (32, 32));
    assert_eq!(settings.bitrate_bps, 500_000);
    assert_eq!(settings.frame_rate, 30);
    assert_eq!(control.capture_size(), (64, 64));
    assert_eq!(control.encoded_size(), (32, 32));
}

#[test]
fn sizes_below_two_pixels_are_rejected() {
    assert!(matches!(
        RateController::open(64, 64, Quality::new(500, 30, 1, 1).unwrap()),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
    assert!(matches!(
        RateController::open(0, 64, Quality::new(500, 30, 32, 32).unwrap()),
        Err(Error::InvalidSetting(
            "encoded size must be at least 2 by 2"
        ))
    ));
}

#[test]
fn a_matching_quality_keeps_the_encoder() {
    let mut control = RateController::open(64, 64, Quality::new(500, 30, 32, 32).unwrap()).unwrap();
    assert!(!control
        .observe(&updated(Quality::new(500, 30, 32, 32).unwrap()))
        .unwrap());
    assert_eq!(control.encoded_size(), (32, 32));
}

#[test]
fn bitrate_frame_rate_and_size_changes_reopen_the_encoder() {
    let mut control = RateController::open(64, 64, Quality::new(500, 30, 32, 32).unwrap()).unwrap();
    assert!(control
        .observe(&updated(Quality::new(800, 30, 32, 32).unwrap()))
        .unwrap());
    assert!(control
        .observe(&updated(Quality::new(800, 15, 32, 32).unwrap()))
        .unwrap());
    assert!(control
        .observe(&updated(Quality::new(800, 15, 16, 16).unwrap()))
        .unwrap());
    assert_eq!(control.encoded_size(), (16, 16));
    assert_eq!(control.settings().bitrate_bps, 800_000);
    assert_eq!(control.settings().frame_rate, 15);
}

#[test]
fn events_beside_quality_never_reopen() {
    let mut control = RateController::open(64, 64, Quality::new(500, 30, 32, 32).unwrap()).unwrap();
    assert!(!control
        .observe(&CoreEvent::KeyframeRequested { id: 7 })
        .unwrap());
    assert!(!control
        .observe(&CoreEvent::InputFailed {
            id: 7,
            message: "input was rejected",
        })
        .unwrap());
}

#[test]
fn congestion_steps_down_on_queue_drops_or_high_rtt() {
    let current = Quality::new(5_000, 30, 1920, 1080).unwrap();
    let down = Quality::new(2_000, 30, 1920, 1080).unwrap();
    for congestion in [
        Congestion {
            rtt_ms: 20,
            relay: false,
            queued: 6,
            dropped_delta: 0,
        },
        Congestion {
            rtt_ms: 20,
            relay: false,
            queued: 0,
            dropped_delta: 3,
        },
        Congestion {
            rtt_ms: 200,
            relay: false,
            queued: 0,
            dropped_delta: 0,
        },
        Congestion {
            rtt_ms: 90,
            relay: true,
            queued: 0,
            dropped_delta: 0,
        },
    ] {
        assert_eq!(suggest_quality(current, congestion), Some(down));
    }
}

#[test]
fn clean_direct_path_steps_up_and_mid_path_holds() {
    let mid = Quality::new(2_000, 30, 1920, 1080).unwrap();
    let up = Quality::new(5_000, 30, 1920, 1080).unwrap();
    assert_eq!(
        suggest_quality(
            mid,
            Congestion {
                rtt_ms: 20,
                relay: false,
                queued: 0,
                dropped_delta: 0
            }
        ),
        Some(up)
    );
    assert_eq!(
        suggest_quality(
            mid,
            Congestion {
                rtt_ms: 60,
                relay: false,
                queued: 0,
                dropped_delta: 0
            }
        ),
        None
    );
    let floor = Quality::new(800, 10, 854, 480).unwrap();
    assert_eq!(
        suggest_quality(
            floor,
            Congestion {
                rtt_ms: 300,
                relay: true,
                queued: 8,
                dropped_delta: 9
            }
        ),
        None
    );
}
