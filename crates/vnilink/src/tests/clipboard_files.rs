use std::time::Duration;

use iroh::RelayMode;

use crate::clipboard::{ClipboardFileEntry, FileSelection};
use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::CoreEvent;
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

fn entry(path: &str, size: u64) -> ClipboardFileEntry {
    ClipboardFileEntry {
        path: path.into(),
        size,
        dir: false,
    }
}

fn selection(entries: Vec<ClipboardFileEntry>) -> FileSelection {
    FileSelection { entries }
}

#[test]
fn dropped_session_does_not_replay_a_selection() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("one.txt", 3)]),
        )
        .unwrap();
    let events = controller.subscribe().unwrap();
    controller.drop_link().unwrap();
    wait_until(|| {
        controller
            .snapshot()
            .unwrap()
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let generation = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "reconnect did not become active"
        );
        if matches!(
            events.recv_timeout(Duration::from_millis(50)),
            Some(CoreEvent::SessionChanged {
                generation,
                phase: SessionPhase::Active,
                ..
            }) if generation != session.generation
        ) {
            break controller.snapshot().unwrap().outgoing.unwrap().generation;
        }
    };
    let again = controller.snapshot().unwrap().outgoing.unwrap();
    assert_eq!(again.generation, generation);
    // The old generation is dead and the new one starts without a selection.
    assert!(matches!(
        controller.submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("replayed.txt", 1)]),
        ),
        Err(Error::NoSession | Error::StaleSession)
    ));
    assert!(matches!(
        controller.session_clipboard_files(again.id, session.generation),
        Err(Error::StaleSession)
    ));
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_none());
    controller
        .submit_clipboard_files(
            again.id,
            again.generation,
            &selection(vec![entry("two.txt", 3)]),
        )
        .unwrap();
    assert_eq!(
        host.incoming_clipboard_files(hosted.id)
            .unwrap()
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn file_selection_wire_codec_round_trips_and_rejects_garbage() {
    use crate::clipboard::{decode, encode, Frame};
    let offer = selection(vec![
        entry("a.txt", 3),
        entry("nested/b.bin", u64::MAX),
        ClipboardFileEntry {
            path: "empty".into(),
            size: 0,
            dir: true,
        },
    ]);
    let bytes = encode(&Frame::Files {
        revision: 7,
        selection: offer.clone(),
    })
    .unwrap();
    assert_eq!(
        decode(&bytes[4..]).unwrap(),
        Frame::Files {
            revision: 7,
            selection: offer,
        }
    );
    let empty = encode(&Frame::Files {
        revision: 3,
        selection: FileSelection::default(),
    })
    .unwrap();
    assert_eq!(
        decode(&empty[4..]).unwrap(),
        Frame::Files {
            revision: 3,
            selection: FileSelection::default(),
        }
    );
    // Tag, truncation, flag, trailing bytes and UTF-8 all fail closed.
    assert!(decode(&[]).is_err());
    assert!(decode(&[9]).is_err());
    assert!(decode(&[4, 0, 0, 0, 0, 0, 0, 0]).is_err());
    let mut truncated = bytes.clone();
    truncated.truncate(truncated.len() - 1);
    assert!(decode(&truncated[4..]).is_err());
    let mut flag = bytes.clone();
    let flag_at = 4 + 1 + 8 + 2;
    flag[flag_at] = 7;
    assert!(decode(&flag[4..]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode(&trailing[4..]).is_err());
    let mut utf8 = encode(&Frame::Files {
        revision: 1,
        selection: selection(vec![entry("ok.txt", 1)]),
    })
    .unwrap();
    let name_at = utf8.len() - "ok.txt".len() - 8;
    utf8[name_at] = 0xFF;
    assert!(decode(&utf8[4..]).is_err());
    // A hostile peer cannot smuggle a jailed path or a duplicate past decode.
    let evil = selection(vec![entry("../evil", 1)]);
    assert!(encode(&Frame::Files {
        revision: 1,
        selection: evil,
    })
    .is_err());
    let dupe = selection(vec![entry("d.txt", 1), entry("d.txt", 1)]);
    assert!(encode(&Frame::Files {
        revision: 1,
        selection: dupe,
    })
    .is_err());
    // Debug never prints selected paths.
    let rendered = format!(
        "{:?}",
        Frame::Files {
            revision: 1,
            selection: selection(vec![entry("do-not-log.txt", 1)]),
        }
    );
    assert!(!rendered.contains("do-not-log"));
}

#[test]
fn offer_binding_covers_only_offered_paths() {
    let offered = selection(vec![entry("a.txt", 1), entry("nested/b.txt", 2)]);
    assert!(offered.covers(&selection(vec![entry("a.txt", 9)])));
    assert!(offered.covers(&offered.clone()));
    assert!(!offered.covers(&selection(vec![entry("evil.txt", 1)])));
    assert!(!offered.covers(&selection(vec![entry("a.txt", 1), entry("evil.txt", 1)])));
    assert!(!FileSelection::default().covers(&selection(vec![entry("a.txt", 1)])));
}
