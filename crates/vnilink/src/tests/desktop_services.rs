#[test]
fn a_stream_end_is_one_known_byte() {
    use crate::{StreamEnd, StreamFailure};
    for end in [
        StreamEnd::Failed(StreamFailure::Encoder),
        StreamEnd::Failed(StreamFailure::Capture),
        StreamEnd::Failed(StreamFailure::Permission),
        StreamEnd::Failed(StreamFailure::DisplayGone),
        StreamEnd::Stopped,
        StreamEnd::Shutdown,
        StreamEnd::Revoked,
    ] {
        assert_eq!(StreamEnd::decode(end.encode()).unwrap(), end);
    }
    for unknown in [0, 8, 255] {
        assert!(matches!(
            StreamEnd::decode(unknown),
            Err(crate::Error::Malformed)
        ));
    }
    assert_eq!(StreamEnd::for_close(crate::CloseCause::PeerClose), None);
    assert_eq!(
        StreamEnd::for_close(crate::CloseCause::DeviceLoss),
        Some(StreamEnd::Failed(StreamFailure::Capture))
    );
}
