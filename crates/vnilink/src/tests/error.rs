use crate::error::Error;

#[test]
fn network_messages_are_truncated_to_256_bytes() {
    match Error::network("abc") {
        Error::Network(text) => assert_eq!(text, "abc"),
        other => panic!("short network error was {other:?}"),
    }
    match Error::network("x".repeat(256)) {
        Error::Network(text) => assert_eq!(text.len(), 256),
        other => panic!("exact network error was {other:?}"),
    }
    match Error::network("y".repeat(300)) {
        Error::Network(text) => {
            assert_eq!(text.len(), 256);
            assert!(text.chars().all(|character| character == 'y'));
        }
        other => panic!("long network error was {other:?}"),
    }
    match Error::network_debug("z".repeat(400)) {
        Error::Network(text) => assert_eq!(text.len(), 256),
        other => panic!("debug network error was {other:?}"),
    }
}
