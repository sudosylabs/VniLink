use crate::{Error, Quality, RateChange, RateController};

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
    assert_eq!(
        control
            .update(Quality::new(500, 30, 32, 32).unwrap())
            .unwrap(),
        RateChange::Same
    );
    assert_eq!(control.encoded_size(), (32, 32));
}

#[test]
fn bitrate_and_frame_rate_retune_and_only_a_new_size_reopens() {
    let mut control = RateController::open(64, 64, Quality::new(500, 30, 32, 32).unwrap()).unwrap();
    assert_eq!(
        control
            .update(Quality::new(800, 30, 32, 32).unwrap())
            .unwrap(),
        RateChange::Retune
    );
    assert_eq!(
        control
            .update(Quality::new(800, 15, 32, 32).unwrap())
            .unwrap(),
        RateChange::Retune
    );
    assert_eq!(
        control
            .update(Quality::new(800, 15, 16, 16).unwrap())
            .unwrap(),
        RateChange::Reopen
    );
    assert_eq!(control.encoded_size(), (16, 16));
    assert_eq!(control.settings().bitrate_bps, 800_000);
    assert_eq!(control.settings().frame_rate, 15);
}
