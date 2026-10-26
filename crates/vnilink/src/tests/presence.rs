use crate::protocol::ControlMessage;
use crate::{Capability, Error, RemoteStatus};

#[test]
fn availability_message_is_strict() {
    let message = ControlMessage::Availability {
        entries: vec![
            (Capability::Camera, RemoteStatus::TurnedOff),
            (Capability::Files, RemoteStatus::Available),
        ],
    };
    let bytes = message.encode();
    assert_eq!(ControlMessage::decode(&bytes).unwrap(), message);
    let empty = ControlMessage::Availability { entries: vec![] };
    assert_eq!(ControlMessage::decode(&empty.encode()).unwrap(), empty);
    for bad in [
        vec![13, 1, 0, 0],       // unknown capability 0
        vec![13, 1, 6, 9],       // unknown status
        vec![13, 2, 6, 1, 6, 0], // the same capability twice
        vec![13, 2, 6, 1],       // shorter than its count
        vec![13, 1, 6, 1, 4],    // trailing byte
        vec![13, 14],            // more entries than capabilities
    ] {
        assert!(
            matches!(ControlMessage::decode(&bad), Err(Error::Malformed)),
            "{bad:?}"
        );
    }
}
