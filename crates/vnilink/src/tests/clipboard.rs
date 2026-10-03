use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::clipboard::{self, Frame, HostClipboard, MAX_CLIPBOARD_TEXT};
use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{HostDesktop, InputEvent};
use crate::session::{Grants, SessionPhase};

fn start_direct() -> Core {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
    .expect("loopback core")
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

struct RecordingDesktop {
    events: Arc<Mutex<Vec<InputEvent>>>,
}

impl HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
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
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard,
            transfer: false,
        },
    )
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
fn clipboard_requires_the_grant_and_leaves_the_session_up() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    let keys = Arc::new(Mutex::new(Vec::new()));
    host.set_clipboard(Box::new(RecordingClipboard {
        texts: Arc::clone(&applied),
    }))
    .unwrap();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&keys),
    }))
    .unwrap();
    assert!(host.capabilities().clipboard);
    assert!(!host.capabilities().file_transfer);
    pair(&host, &controller, false);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        host.set_clipboard(Box::new(RecordingClipboard {
            texts: Arc::clone(&applied),
        })),
        Err(Error::Busy)
    ));
    assert!(matches!(
        controller.submit_clipboard(session.id, session.generation, "secret-paste"),
        Err(Error::ClipboardDenied)
    ));
    assert!(texts(&applied).is_empty());
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    wait_until(|| texts_len(&keys) == 1);
    assert_eq!(
        keys.lock().unwrap().as_slice(),
        &[InputEvent::key_down(crate::input::usage::A)]
    );
    assert!(matches!(
        controller.submit_clipboard(session.id, session.generation + 1, "nope"),
        Err(Error::StaleSession)
    ));
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    controller
        .submit_clipboard(session.id, session.generation, "secret-paste")
        .unwrap();
    controller
        .submit_clipboard(session.id, session.generation, "")
        .unwrap();
    assert_eq!(texts(&applied), ["secret-paste".to_string(), String::new()]);
    assert!(!host.capabilities().file_transfer);
}

fn texts_len(slot: &Mutex<Vec<InputEvent>>) -> usize {
    slot.lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .len()
}

#[test]
fn oversize_text_is_rejected_before_it_is_sent() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_clipboard(Box::new(RecordingClipboard {
        texts: Arc::clone(&applied),
    }))
    .unwrap();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let too_big = "a".repeat(MAX_CLIPBOARD_TEXT + 1);
    assert!(matches!(
        controller.submit_clipboard(session.id, session.generation, &too_big),
        Err(Error::InvalidSetting(_))
    ));
    assert!(texts(&applied).is_empty());
    controller
        .submit_clipboard(session.id, session.generation, "ok")
        .unwrap();
    assert_eq!(texts(&applied), ["ok".to_string()]);
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn the_host_publishes_text_the_controller_can_read() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, false);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let events = controller.subscribe().unwrap();
    assert!(matches!(
        host.publish_clipboard(hosted.id, "secret-from-host"),
        Err(Error::ClipboardDenied)
    ));
    assert!(controller.clipboard_text().unwrap().is_none());
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    host.publish_clipboard(hosted.id, "secret-from-host")
        .unwrap();
    wait_until(|| controller.clipboard_text().unwrap().as_deref() == Some("secret-from-host"));
    host.publish_clipboard(hosted.id, "").unwrap();
    wait_until(|| controller.clipboard_text().unwrap().as_deref() == Some(""));
    let mut saw = false;
    while let Some(event) = events.try_recv() {
        let rendered = format!("{event:?}");
        assert!(!rendered.contains("secret-from-host"));
        if let CoreEvent::ClipboardReceived { id } = event {
            assert_eq!(id, session.id);
            saw = true;
        }
    }
    assert!(saw);
    assert!(matches!(
        controller.publish_clipboard(session.id, "nope"),
        Err(Error::NoSession)
    ));
}

#[test]
fn no_adapter_refuses_the_paste() {
    let host = start_direct();
    let controller = start_direct();
    assert!(!host.capabilities().clipboard);
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.submit_clipboard(session.id, session.generation, "secret-paste"),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    assert!(controller.clipboard_text().unwrap().is_none());
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
