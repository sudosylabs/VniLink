use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::clipboard::{self, Frame, HostClipboard, MAX_CLIPBOARD_TEXT};
use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::session::{Grants, SessionPhase};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

struct RecordingClipboard {
    texts: Arc<Mutex<Vec<String>>>,
}

impl HostClipboard for RecordingClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.texts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(text.to_owned());
        Ok(())
    }
}

fn pair(host: &Core, controller: &Core, clipboard: bool) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard,
        transfer: false,
    })
    .unwrap();
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ready() {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("condition was not met");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn texts(slot: &Mutex<Vec<String>>) -> Vec<String> {
    slot.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

#[test]
fn a_dropped_session_does_not_replay_clipboard_text() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_clipboard(Box::new(RecordingClipboard {
        texts: Arc::clone(&applied),
    }))
    .unwrap();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_clipboard(session.id, session.generation, "one")
        .unwrap();
    controller.drop_link().unwrap();
    wait_until(|| {
        controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
    });
    assert!(matches!(
        controller.submit_clipboard(session.id, session.generation, "replayed"),
        Err(Error::NoSession | Error::StaleSession)
    ));
    wait_until(|| {
        let outgoing = controller.snapshot().unwrap().outgoing.unwrap();
        outgoing.phase == SessionPhase::Active && outgoing.generation != session.generation
    });
    let again = controller.snapshot().unwrap().outgoing.unwrap();
    controller
        .submit_clipboard(again.id, again.generation, "two")
        .unwrap();
    assert_eq!(texts(&applied), ["one".to_string(), "two".to_string()]);
}

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
