use vnilink_media::Error;

use crate::encode::MacEncoder;

#[test]
fn an_odd_encoder_size_is_rejected() {
    assert!(matches!(
        MacEncoder::open(63, 64, 30, 500_000),
        Err(Error::InvalidFrame("H.264 width and height must be even"))
    ));
}
