use std::sync::{Arc, Mutex};
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

// --- Paste preparation (slice 2) ---

use crate::clipboard::{HostClipboard, HostClipboardFiles, PreparedPaste};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Scratch {
    path: std::path::PathBuf,
}

impl Scratch {
    fn tagged(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        let path = std::env::temp_dir().join(format!(
            "vnilink-paste-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("files")).unwrap();
        std::fs::create_dir_all(path.join("blobs")).unwrap();
        Self { path }
    }

    fn files(&self) -> std::path::PathBuf {
        self.path.join("files")
    }

    fn blobs(&self) -> std::path::PathBuf {
        self.path.join("blobs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn pair_with(host: &Core, controller: &Core, clipboard: bool, transfer: bool) {
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
        transfer,
    })
    .unwrap();
}

fn transfer_grants() -> Grants {
    Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: true,
    }
}

/// Resolves selections under one root, canonicalizing like a production
/// file-manager scope. Counts calls and optionally gates the first resolve
/// so stall tests can rendezvous with the preparation worker.
struct DirSource {
    root: std::path::PathBuf,
    calls: Arc<AtomicUsize>,
    gate: Mutex<Option<Gate>>,
}

struct Gate {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl DirSource {
    fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            calls: Arc::new(AtomicUsize::new(0)),
            gate: Mutex::new(None),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl HostClipboardFiles for DirSource {
    fn resolve_source(&self, path: &str) -> Result<std::path::PathBuf, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = self.gate.lock().unwrap().take() {
            let _ = gate.entered.send(());
            let _ = gate.release.recv();
        }
        let full = self
            .root
            .join(path)
            .canonicalize()
            .map_err(|_| Error::InvalidSetting("paste source is not selected"))?;
        let root = self
            .root
            .canonicalize()
            .map_err(|_| Error::InvalidSetting("paste source is not selected"))?;
        if !full.starts_with(&root) {
            return Err(Error::InvalidSetting("paste source is not selected"));
        }
        Ok(full)
    }
}

struct RecordingClipboard {
    texts: Arc<Mutex<Vec<String>>>,
}

impl HostClipboard for RecordingClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.texts.lock().unwrap().push(text.to_owned());
        Ok(())
    }
}

#[test]
fn paste_round_trip_fetches_offered_bytes_in_both_directions() {
    let host_scratch = Scratch::tagged("roundtrip-host");
    let controller_scratch = Scratch::tagged("roundtrip-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    let controller_source = Arc::new(DirSource::new(controller_scratch.files()));
    controller
        .set_clipboard_file_source(Some(controller_source.clone()))
        .unwrap();
    let host_source = Arc::new(DirSource::new(host_scratch.files()));
    host.set_clipboard_file_source(Some(host_source.clone()))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();

    // Controller offers two files and an empty directory; the host pastes.
    std::fs::write(controller_scratch.files().join("a.txt"), b"alpha").unwrap();
    std::fs::create_dir_all(controller_scratch.files().join("nested")).unwrap();
    std::fs::write(controller_scratch.files().join("nested/b.bin"), [7u8; 1024]).unwrap();
    let offer = selection(vec![
        entry("a.txt", 5),
        entry("nested/b.bin", 1024),
        ClipboardFileEntry {
            path: "empty-dir".into(),
            size: 0,
            dir: true,
        },
    ]);
    controller
        .submit_clipboard_files(session.id, session.generation, &offer)
        .unwrap();
    let prepared = host.prepare_incoming_paste(hosted.id, 1).unwrap();
    // Only files hash; the directory recreates from the stored selection.
    assert_eq!(controller_source.calls(), 2);
    assert_eq!(prepared.files.len(), 2);
    assert_eq!(prepared.files[0].path, "a.txt");
    assert_eq!(prepared.files[0].size, 5);
    for file in &prepared.files {
        let bytes = host
            .blob_fetch(&controller.endpoint_id(), file.blob_hash(), file.size)
            .unwrap();
        let expected = std::fs::read(controller_scratch.files().join(&file.path)).unwrap();
        assert_eq!(bytes, expected);
    }
    // The stored selection still lists the directory for recreation.
    let stored = host.incoming_clipboard_files(hosted.id).unwrap().unwrap();
    assert!(stored.selection.entries.iter().any(|e| e.dir));

    // Host offers one file; the controller pastes it back the other way.
    std::fs::write(host_scratch.files().join("down.txt"), b"beta!").unwrap();
    let back = selection(vec![entry("down.txt", 5)]);
    publish_ready(&host, hosted.id, &back);
    wait_until(|| controller.clipboard_files().unwrap().is_some());
    let revision = controller.clipboard_files().unwrap().unwrap().revision;
    let prepared = controller
        .prepare_outgoing_paste(session.id, session.generation, revision)
        .unwrap();
    assert_eq!(host_source.calls(), 1);
    assert_eq!(prepared.files.len(), 1);
    let bytes = controller
        .blob_fetch(&host.endpoint_id(), prepared.files[0].blob_hash(), 5)
        .unwrap();
    assert_eq!(bytes, b"beta!");
}

#[test]
fn paste_needs_the_transfer_grant_on_both_sides() {
    let host_scratch = Scratch::tagged("grant-host");
    let controller_scratch = Scratch::tagged("grant-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    controller
        .set_clipboard_file_source(Some(Arc::new(DirSource::new(controller_scratch.files()))))
        .unwrap();
    pair_with(&host, &controller, true, false);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("a.txt"), b"alpha").unwrap();
    let offer = selection(vec![entry("a.txt", 5)]);
    controller
        .submit_clipboard_files(session.id, session.generation, &offer)
        .unwrap();
    // Receiver side refuses without Transfer.
    assert!(matches!(
        host.prepare_incoming_paste(hosted.id, 1),
        Err(Error::TransferDenied)
    ));
    host.set_desktop_sharing(transfer_grants()).unwrap();
    // Source side refuses: the controller never granted the host Transfer.
    assert!(matches!(
        host.prepare_incoming_paste(hosted.id, 1),
        Err(Error::TransferDenied)
    ));
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let prepared = host.prepare_incoming_paste(hosted.id, 1).unwrap();
    assert_eq!(prepared.files.len(), 1);
}

#[test]
fn stale_revision_is_rejected_before_any_wire_traffic() {
    let host_scratch = Scratch::tagged("stale-host");
    let controller_scratch = Scratch::tagged("stale-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    let source = Arc::new(DirSource::new(controller_scratch.files()));
    controller
        .set_clipboard_file_source(Some(source.clone()))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("a.txt"), b"a").unwrap();
    std::fs::write(controller_scratch.files().join("b.txt"), b"b").unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("a.txt", 1)]),
        )
        .unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("b.txt", 1)]),
        )
        .unwrap();
    assert!(matches!(
        host.prepare_incoming_paste(hosted.id, 1),
        Err(Error::StaleSession)
    ));
    assert_eq!(source.calls(), 0);
    let prepared = host.prepare_incoming_paste(hosted.id, 2).unwrap();
    assert_eq!(prepared.files[0].path, "b.txt");
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

#[test]
fn vanished_source_file_fails_the_paste_honestly() {
    let host_scratch = Scratch::tagged("vanished-host");
    let controller_scratch = Scratch::tagged("vanished-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    controller
        .set_clipboard_file_source(Some(Arc::new(DirSource::new(controller_scratch.files()))))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    let victim = controller_scratch.files().join("gone.txt");
    std::fs::write(&victim, b"here").unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("gone.txt", 4)]),
        )
        .unwrap();
    std::fs::remove_file(&victim).unwrap();
    assert!(matches!(
        host.prepare_incoming_paste(hosted.id, 1),
        Err(Error::Unsupported("paste source is unavailable"))
    ));
}

#[test]
fn missing_resolver_fails_fast_on_the_wire_status() {
    let host_scratch = Scratch::tagged("noresolver-host");
    let controller_scratch = Scratch::tagged("noresolver-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    // No file source installed on the controller: the refusal rides the
    // ready frame, since the dial side writes no statuses.
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("a.txt"), b"a").unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("a.txt", 1)]),
        )
        .unwrap();
    assert!(matches!(
        host.prepare_incoming_paste(hosted.id, 1),
        Err(Error::Unsupported("paste source is unavailable"))
    ));
}

#[test]
fn slow_preparation_does_not_stall_text_paste() {
    let host_scratch = Scratch::tagged("stall-host");
    let controller_scratch = Scratch::tagged("stall-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_clipboard(Box::new(RecordingClipboard {
        texts: applied.clone(),
    }))
    .unwrap();
    let source = Arc::new(DirSource::new(controller_scratch.files()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *source.gate.lock().unwrap() = Some(Gate {
        entered: entered_tx,
        release: release_rx,
    });
    controller
        .set_clipboard_file_source(Some(source.clone()))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("slow.bin"), [9u8; 64]).unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("slow.bin", 64)]),
        )
        .unwrap();
    std::thread::scope(|scope| {
        let paste = scope.spawn(|| host.prepare_incoming_paste(hosted.id, 1));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The preparation worker is blocked hashing; text still flows.
        controller
            .submit_clipboard(session.id, session.generation, "unstalled")
            .unwrap();
        wait_until(|| applied.lock().unwrap().as_slice() == ["unstalled"]);
        release_tx.send(()).unwrap();
        let prepared = paste.join().unwrap().unwrap();
        assert_eq!(prepared.files.len(), 1);
        let bytes = host
            .blob_fetch(&controller.endpoint_id(), prepared.files[0].blob_hash(), 64)
            .unwrap();
        assert_eq!(bytes, [9u8; 64]);
    });
}

#[test]
fn same_path_reoffer_during_preparation_refuses_the_stale_revision() {
    let host_scratch = Scratch::tagged("reoffer-host");
    let controller_scratch = Scratch::tagged("reoffer-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    let source = Arc::new(DirSource::new(controller_scratch.files()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *source.gate.lock().unwrap() = Some(Gate {
        entered: entered_tx,
        release: release_rx,
    });
    controller
        .set_clipboard_file_source(Some(source.clone()))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("a.txt"), b"12345").unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("a.txt", 5)]),
        )
        .unwrap();
    std::thread::scope(|scope| {
        let paste = scope.spawn(|| host.prepare_incoming_paste(hosted.id, 1));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Same path, replacement bytes, fresh revision — while rev1 hashes.
        std::fs::write(controller_scratch.files().join("a.txt"), b"12345678901").unwrap();
        controller
            .submit_clipboard_files(
                session.id,
                session.generation,
                &selection(vec![entry("a.txt", 11)]),
            )
            .unwrap();
        release_tx.send(()).unwrap();
        // The rev1 preparation must not succeed with rev2's hash/size.
        assert!(matches!(paste.join().unwrap(), Err(Error::Unauthorized)));
    });
    // A fresh prepare against the current revision serves the new bytes.
    let prepared = host.prepare_incoming_paste(hosted.id, 2).unwrap();
    assert_eq!(prepared.files.len(), 1);
    assert_eq!(prepared.files[0].size, 11);
    let bytes = host
        .blob_fetch(&controller.endpoint_id(), prepared.files[0].blob_hash(), 11)
        .unwrap();
    assert_eq!(bytes, b"12345678901");
}

#[test]
fn second_concurrent_prepare_is_busy() {
    let host_scratch = Scratch::tagged("busy-host");
    let controller_scratch = Scratch::tagged("busy-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    let source = Arc::new(DirSource::new(controller_scratch.files()));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *source.gate.lock().unwrap() = Some(Gate {
        entered: entered_tx,
        release: release_rx,
    });
    controller
        .set_clipboard_file_source(Some(source.clone()))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    std::fs::write(controller_scratch.files().join("a.txt"), b"a").unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("a.txt", 1)]),
        )
        .unwrap();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| host.prepare_incoming_paste(hosted.id, 1));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            host.prepare_incoming_paste(hosted.id, 1),
            Err(Error::Busy)
        ));
        release_tx.send(()).unwrap();
        assert_eq!(first.join().unwrap().unwrap().files.len(), 1);
    });
}

#[test]
fn large_file_paste_streams_past_legacy_caps() {
    let host_scratch = Scratch::tagged("large-host");
    let controller_scratch = Scratch::tagged("large-controller");
    let host = start_direct();
    let controller = start_direct();
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    controller
        .set_clipboard_file_source(Some(Arc::new(DirSource::new(controller_scratch.files()))))
        .unwrap();
    pair_with(&host, &controller, true, true);
    controller.set_desktop_sharing(transfer_grants()).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    // 40 MiB clears the 32 MiB whole-bytes cap and the legacy 8 MiB
    // transfer cap: the paste must stream and export, never buffer whole.
    let big = vec![0x5Au8; 40 * 1024 * 1024];
    std::fs::write(controller_scratch.files().join("big.bin"), &big).unwrap();
    controller
        .submit_clipboard_files(
            session.id,
            session.generation,
            &selection(vec![entry("big.bin", big.len() as u64)]),
        )
        .unwrap();
    let prepared = host.prepare_incoming_paste(hosted.id, 1).unwrap();
    assert_eq!(prepared.files[0].size, big.len() as u64);
    host.blob_fetch_streamed(
        &controller.endpoint_id(),
        prepared.files[0].blob_hash(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        host.blob_verified(prepared.files[0].blob_hash()).unwrap(),
        Some(big.len() as u64)
    );
    let dest = host_scratch.files().join("big.bin");
    assert_eq!(
        host.blob_export(prepared.files[0].blob_hash(), &dest)
            .unwrap(),
        big.len() as u64
    );
    assert_eq!(std::fs::read(&dest).unwrap(), big);
}

#[test]
fn prepare_ready_codec_round_trips_and_rejects_garbage() {
    use crate::clipboard::{decode, encode, Frame, PasteReadyOutcome};
    let request = selection(vec![entry("a.txt", 1)]);
    let bytes = encode(&Frame::Prepare {
        nonce: 7,
        revision: 2,
        selection: request.clone(),
    })
    .unwrap();
    assert_eq!(
        decode(&bytes[4..]).unwrap(),
        Frame::Prepare {
            nonce: 7,
            revision: 2,
            selection: request
        }
    );
    // A truncated revision fails closed.
    assert!(decode(&[5, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0]).is_err());
    let ready = PasteReadyOutcome::Ready(vec![([3u8; 32], 41u64), ([4u8; 32], 0u64)]);
    let bytes = encode(&Frame::FilesReady {
        nonce: 9,
        outcome: ready.clone(),
    })
    .unwrap();
    assert_eq!(
        decode(&bytes[4..]).unwrap(),
        Frame::FilesReady {
            nonce: 9,
            outcome: ready
        }
    );
    for refusal in [
        PasteReadyOutcome::TransferDenied,
        PasteReadyOutcome::Unauthorized,
        PasteReadyOutcome::Unavailable,
    ] {
        let bytes = encode(&Frame::FilesReady {
            nonce: 11,
            outcome: refusal.clone(),
        })
        .unwrap();
        assert_eq!(
            decode(&bytes[4..]).unwrap(),
            Frame::FilesReady {
                nonce: 11,
                outcome: refusal
            }
        );
    }
    // Truncation, trailing bytes, count lies and bad outcomes fail closed.
    let mut truncated = bytes.clone();
    truncated.truncate(truncated.len() - 1);
    assert!(decode(&truncated[4..]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode(&trailing[4..]).is_err());
    let mut lied = bytes.clone();
    lied[14] = 0;
    lied[15] = 9;
    assert!(decode(&lied[4..]).is_err());
    let mut bad_outcome = bytes.clone();
    bad_outcome[13] = 9;
    assert!(decode(&bad_outcome[4..]).is_err());
    assert!(decode(&[6u8, 7]).is_err());
    assert!(decode(&[6u8]).is_err());
    assert!(decode(&[5u8, 1, 2, 3]).is_err());
    // Debug never prints hashes.
    let rendered = format!(
        "{:?}",
        Frame::FilesReady {
            nonce: 9,
            outcome: PasteReadyOutcome::Ready(vec![([9u8; 32], 1)])
        }
    );
    assert!(!rendered.contains("0909"));
    let _ = PreparedPaste::default();
}
