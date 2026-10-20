use std::time::Duration;

use iroh::RelayMode;

use crate::clipboard::{
    ClipboardFileEntry, FileSelection, MAX_CLIPBOARD_FILES, MAX_CLIPBOARD_FILES_BYTES,
};
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

fn wait_active(controller: &Core, id: u64) -> crate::SessionInfo {
    wait_until(|| controller.controller_session(id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(id).unwrap()
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

fn publish_ready(host: &Core, hosted_id: u64, selection: &FileSelection) {
    // Controller connect sends Ready before the host installs its side-stream
    // sender; retry that one asynchronous setup like the text tests.
    wait_until(
        || match host.publish_clipboard_files(hosted_id, selection) {
            Ok(()) => true,
            Err(Error::Unsupported("clipboard stream is not open")) => false,
            Err(error) => panic!("host file publication failed: {error}"),
        },
    );
}

#[test]
fn file_selection_requires_the_grant_and_leaves_the_session_up() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, false);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let offer = selection(vec![entry("report.pdf", 12_000)]);
    // No clipboard adapter is installed: file offers store, they never call
    // a text adapter.
    assert!(matches!(
        controller.submit_clipboard_files(session.id, session.generation, &offer),
        Err(Error::ClipboardDenied)
    ));
    assert!(matches!(
        host.publish_clipboard_files(hosted.id, &offer),
        Err(Error::ClipboardDenied)
    ));
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_none());
    assert!(controller.clipboard_files().unwrap().is_none());
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
    assert!(matches!(
        controller.submit_clipboard_files(session.id, session.generation + 1, &offer),
        Err(Error::StaleSession)
    ));
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: false,
    })
    .unwrap();
    controller
        .submit_clipboard_files(session.id, session.generation, &offer)
        .unwrap();
    let stored = host.incoming_clipboard_files(hosted.id).unwrap().unwrap();
    // The refused send above consumed revision 1: issuance precedes the
    // round trip so concurrent offers never share a revision, and gaps
    // are fine because revisions are never reused.
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.selection, offer);
    publish_ready(&host, hosted.id, &offer);
    wait_until(|| {
        controller
            .clipboard_files()
            .unwrap()
            .is_some_and(|stored| stored.selection == offer)
    });
}

#[test]
fn file_selection_replaces_and_clears_keep_the_revision_chain() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let first = selection(vec![entry("a.txt", 3)]);
    let second = selection(vec![
        entry("nested/b.txt", 4),
        ClipboardFileEntry {
            path: "empty-dir".into(),
            size: 0,
            dir: true,
        },
    ]);
    controller
        .submit_clipboard_files(session.id, session.generation, &first)
        .unwrap();
    controller
        .submit_clipboard_files(session.id, session.generation, &second)
        .unwrap();
    let stored = host.incoming_clipboard_files(hosted.id).unwrap().unwrap();
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.selection, second);
    controller
        .submit_clipboard_files(session.id, session.generation, &FileSelection::default())
        .unwrap();
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_none());
    // A clear is revision 3, so the next offer is 4: a paste prepared
    // against any older revision stays stale across the clear.
    controller
        .submit_clipboard_files(session.id, session.generation, &first)
        .unwrap();
    let stored = host.incoming_clipboard_files(hosted.id).unwrap().unwrap();
    assert_eq!(stored.revision, 4);
}

#[test]
fn file_selection_bounds_reject_before_anything_is_sent() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let too_many = FileSelection {
        entries: (0..MAX_CLIPBOARD_FILES + 1)
            .map(|n| entry(&format!("f{n}.txt"), 1))
            .collect(),
    };
    let duplicate = selection(vec![entry("same.txt", 1), entry("same.txt", 2)]);
    let escape = selection(vec![entry("../outside.txt", 1)]);
    let absolute = selection(vec![entry("/etc/hosts", 1)]);
    let sized_dir = selection(vec![ClipboardFileEntry {
        path: "dir".into(),
        size: 9,
        dir: true,
    }]);
    // Each name passes the per-path jail (two short components) while the
    // total breaks the 64 KiB selection budget: 256 * (11 + 250).
    let wide = FileSelection {
        entries: (0..MAX_CLIPBOARD_FILES)
            .map(|n| {
                entry(
                    &format!("{}/{:03}-{}", "x".repeat(200), n, "y".repeat(44)),
                    1,
                )
            })
            .collect(),
    };
    assert!(wide.encoded_len() > MAX_CLIPBOARD_FILES_BYTES);
    for bad in [&too_many, &duplicate, &escape, &absolute, &sized_dir, &wide] {
        assert!(
            matches!(
                controller.submit_clipboard_files(session.id, session.generation, bad),
                Err(Error::InvalidSetting(_))
            ),
            "selection was accepted: {bad:?}"
        );
        assert!(
            matches!(
                host.publish_clipboard_files(hosted.id, bad),
                Err(Error::InvalidSetting(_))
            ),
            "publication was accepted: {bad:?}"
        );
    }
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_none());
    assert!(controller.clipboard_files().unwrap().is_none());
}

#[test]
fn withdrawing_the_grant_drops_offers_while_the_session_survives() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let from_controller = selection(vec![entry("up.bin", 8)]);
    let from_host = selection(vec![entry("down.bin", 9)]);
    controller
        .submit_clipboard_files(session.id, session.generation, &from_controller)
        .unwrap();
    publish_ready(&host, hosted.id, &from_host);
    wait_until(|| controller.clipboard_files().unwrap().is_some());
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    // The host's stored offer is withdrawn: a selection stays a live paste
    // capability after the grant is gone.
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_none());
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
    // The controller keeps inert metadata it already received; any paste
    // against it revalidates with the host, which no longer grants it.
    assert_eq!(
        controller.clipboard_files().unwrap().unwrap().selection,
        from_host
    );
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: false,
    })
    .unwrap();
    controller
        .submit_clipboard_files(session.id, session.generation, &from_controller)
        .unwrap();
    // The counter survives the withdrawal clear: revision 1 is retired
    // with the withdrawn offer and never reused, so a paste pinned to it
    // stays stale even if identical paths are re-offered.
    assert_eq!(
        host.incoming_clipboard_files(hosted.id)
            .unwrap()
            .unwrap()
            .revision,
        2
    );
}

#[test]
fn revoked_peer_loses_its_hosted_selection() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("up.bin", 8)]),
        )
        .unwrap();
    assert!(host.incoming_clipboard_files(hosted.id).unwrap().is_some());
    host.revoke(&controller.endpoint_id()).unwrap();
    assert!(matches!(
        host.incoming_clipboard_files(hosted.id),
        Err(Error::NoSession)
    ));
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
fn host_selections_publish_only_to_the_selected_session() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    pair(&host, &first, true);
    pair(&host, &second, true);
    let _one = first.connect(&host.endpoint_id()).unwrap();
    let _two = second.connect(&host.endpoint_id()).unwrap();
    let hosted: Vec<_> = host.hosted_sessions().unwrap();
    assert_eq!(hosted.len(), 2);
    let (older, newer) = (hosted[0].id, hosted[1].id);
    let offer = selection(vec![entry("new.bin", 1)]);
    // Admission selects the newest session; the older one is Busy.
    assert!(matches!(
        host.publish_clipboard_files(older, &offer),
        Err(Error::Busy)
    ));
    publish_ready(&host, newer, &offer);
    wait_until(|| second.clipboard_files().unwrap().is_some());
    assert!(first.clipboard_files().unwrap().is_none());
}

#[test]
fn controller_file_reads_need_one_unambiguous_session() {
    let a = start_direct();
    let b = start_direct();
    let controller = start_direct();
    pair(&a, &controller, true);
    pair(&b, &controller, true);
    let pending_a = controller.begin_connect(&a.endpoint_id()).unwrap();
    let pending_b = controller.begin_connect(&b.endpoint_id()).unwrap();
    let to_a = wait_active(&controller, pending_a.id);
    let to_b = wait_active(&controller, pending_b.id);
    assert!(matches!(controller.clipboard_files(), Err(Error::Busy)));
    // Generation-scoped reads stay exact with two sessions up.
    assert!(controller
        .session_clipboard_files(to_a.id, to_a.generation)
        .unwrap()
        .is_none());
    assert!(controller
        .session_clipboard_files(to_b.id, to_b.generation)
        .unwrap()
        .is_none());
    assert!(matches!(
        controller.session_clipboard_files(to_a.id, to_a.generation + 1),
        Err(Error::StaleSession)
    ));
}

#[test]
fn file_selection_events_carry_no_paths() {
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let host_events = host.subscribe().unwrap();
    let controller_events = controller.subscribe().unwrap();
    let secret = selection(vec![entry("secret-plan.pdf", 100)]);
    controller
        .submit_clipboard_files(session.id, session.generation, &secret)
        .unwrap();
    publish_ready(&host, hosted.id, &secret);
    wait_until(|| controller.clipboard_files().unwrap().is_some());
    let mut saw_host = false;
    while let Some(event) = host_events.try_recv() {
        assert!(!format!("{event:?}").contains("secret-plan"));
        if let CoreEvent::ClipboardFilesReceived { id } = event {
            assert_eq!(id, hosted.id);
            saw_host = true;
        }
    }
    let mut saw_controller = false;
    while let Some(event) = controller_events.try_recv() {
        assert!(!format!("{event:?}").contains("secret-plan"));
        if let CoreEvent::ClipboardFilesReceived { id } = event {
            assert_eq!(id, session.id);
            saw_controller = true;
        }
    }
    assert!(saw_host && saw_controller);
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
