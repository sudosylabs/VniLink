use crate::clipboard::{self, Frame, MAX_CLIPBOARD_TEXT};
use crate::error::Error;

#[test]
fn text_frames_round_trip_and_hide_the_characters() {
    let frame = Frame::Text("vnl-secret-héllo".to_string());
    assert!(!format!("{frame:?}").contains("vnl-secret"));
    let encoded = clipboard::encode(&frame).unwrap();
    let len = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
    assert_eq!(encoded.len(), 4 + len);
    assert_eq!(clipboard::decode(&encoded[4..]).unwrap(), frame);
    assert_eq!(
        clipboard::decode(&clipboard::encode(&Frame::Text(String::new())).unwrap()[4..]).unwrap(),
        Frame::Text(String::new())
    );
    assert_eq!(
        clipboard::decode(&clipboard::encode(&Frame::Status(1)).unwrap()[4..]).unwrap(),
        Frame::Status(1)
    );
    assert!(clipboard::check_text("").is_ok());
    assert!(clipboard::check_text(&"a".repeat(MAX_CLIPBOARD_TEXT)).is_ok());
    assert!(matches!(
        clipboard::check_text(&"a".repeat(MAX_CLIPBOARD_TEXT + 1)),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(clipboard::check_len(0), Err(Error::Malformed)));
    assert!(matches!(
        clipboard::check_len(usize::MAX),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        clipboard::decode(&[1, 0, 0, 0, 1, 0xff]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        clipboard::decode(b"\x01\x00\x00\x00\x01aextra"),
        Err(Error::Malformed)
    ));
    assert!(matches!(clipboard::decode(&[2, 9]), Err(Error::Malformed)));
    assert!(matches!(clipboard::decode(&[9]), Err(Error::Malformed)));
    assert_eq!(
        clipboard::decode(&clipboard::encode(&Frame::Ready).unwrap()[4..]).unwrap(),
        Frame::Ready
    );
}
