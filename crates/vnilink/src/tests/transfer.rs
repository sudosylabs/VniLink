use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::{HostDesktop, InputEvent};
use crate::session::{Grants, SessionPhase};
use crate::transfer::{self, Frame, MAX_FILE_BYTES};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_h264_support(true, true).expect("flags");
    core
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

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vnilink-transfer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn pair(host: &Core, controller: &Core, transfer: bool) {
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
            clipboard: false,
            transfer,
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

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn hides(err: &Error, secret: &str) {
    let rendered = err.to_string();
    assert!(!rendered.contains(secret), "{rendered}");
}

#[test]
fn transfer_requires_the_grant_and_leaves_the_session_up() {
    let scratch = Scratch::new();
    let root = scratch.join("host");
    let host = start_direct();
    let controller = start_direct();
    let keys = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&keys),
    }))
    .unwrap();
    host.set_transfer_directory(&root).unwrap();
    assert!(host.capabilities().file_transfer);
    assert!(!controller.capabilities().file_transfer);
    pair(&host, &controller, false);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        host.set_transfer_directory(&scratch.join("other")),
        Err(Error::Busy)
    ));
    let denied = controller
        .submit_file(
            session.id,
            session.generation,
            "notes/secret.txt",
            b"vnl-secret-bytes",
            false,
        )
        .unwrap_err();
    assert!(matches!(denied, Error::TransferDenied));
    hides(&denied, "vnl-secret");
    assert!(!root.join("notes").exists());
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
    assert!(host.capabilities().file_transfer);
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    wait_until(|| {
        keys.lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .len()
            == 1
    });
    assert!(matches!(
        controller.submit_file(session.id, session.generation + 1, "a.txt", b"nope", false),
        Err(Error::StaleSession)
    ));
    assert!(matches!(
        controller.submit_file(
            session.id,
            session.generation,
            "foo/../../x",
            b"vnl-secret-bytes",
            true
        ),
        Err(Error::InvalidSetting(_))
    ));
    assert!(!scratch.join("x").exists());
    let too_big = vec![1u8; MAX_FILE_BYTES + 1];
    assert!(matches!(
        controller.submit_file(session.id, session.generation, "big.bin", &too_big, false),
        Err(Error::InvalidSetting(_))
    ));
    assert!(!root.join("big.bin").exists());
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: true,
        },
    )
    .unwrap();
    let events = host.subscribe().unwrap();
    controller
        .submit_file(
            session.id,
            session.generation,
            "notes/today.txt",
            b"vnl-secret-bytes",
            false,
        )
        .unwrap();
    assert_eq!(read(&root.join("notes/today.txt")), b"vnl-secret-bytes");
    controller
        .submit_file(session.id, session.generation, "empty.txt", b"", false)
        .unwrap();
    assert_eq!(read(&root.join("empty.txt")), b"");
    let exists = controller
        .submit_file(
            session.id,
            session.generation,
            "notes/today.txt",
            b"new-secret",
            false,
        )
        .unwrap_err();
    assert!(matches!(exists, Error::FileExists));
    hides(&exists, "new-secret");
    assert_eq!(read(&root.join("notes/today.txt")), b"vnl-secret-bytes");
    controller
        .submit_file(
            session.id,
            session.generation,
            "notes/today.txt",
            b"replaced",
            true,
        )
        .unwrap();
    assert_eq!(read(&root.join("notes/today.txt")), b"replaced");
    #[cfg(unix)]
    {
        let outside = scratch.join("outside");
        std::fs::write(&outside, b"outside-secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let err = controller
            .submit_file(session.id, session.generation, "link", b"pwned", true)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidSetting(_)));
        hides(&err, "outside");
        hides(&err, "pwned");
        assert_eq!(read(&outside), b"outside-secret");
    }
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    assert_eq!(hosted.phase, SessionPhase::Active);
    let mut saw = false;
    while let Some(event) = events.try_recv() {
        assert!(!format!("{event:?}").contains("vnl-secret"));
        assert!(!format!("{event:?}").contains("today"));
        if let CoreEvent::TransferCompleted { id } = event {
            assert_eq!(id, hosted.id);
            saw = true;
        }
    }
    assert!(saw);
    assert!(matches!(
        controller.submit_file(99, session.generation, "a.txt", b"nope", false),
        Err(Error::NoSession)
    ));
}

#[test]
fn no_directory_refuses_the_file() {
    let host = start_direct();
    let controller = start_direct();
    assert!(!host.capabilities().file_transfer);
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.submit_file(session.id, session.generation, "a.txt", b"secret", false),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    assert!(!host.capabilities().file_transfer);
}

#[test]
fn the_host_publishes_a_file_the_controller_stores() {
    let scratch = Scratch::new();
    let root = scratch.join("controller");
    let host = start_direct();
    let controller = start_direct();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let hosted = host.snapshot().unwrap().incoming.unwrap();
    assert!(matches!(
        host.publish_file(hosted.id, "from-host.txt", b"vnl-secret-bytes", false),
        Err(Error::Unsupported(_))
    ));
    controller.set_transfer_directory(&root).unwrap();
    assert!(controller.capabilities().file_transfer);
    let events = controller.subscribe().unwrap();
    host.publish_file(hosted.id, "from-host.txt", b"vnl-secret-bytes", false)
        .unwrap();
    assert_eq!(read(&root.join("from-host.txt")), b"vnl-secret-bytes");
    let err = host
        .publish_file(hosted.id, "from-host.txt", b"other-secret", false)
        .unwrap_err();
    assert!(matches!(err, Error::FileExists));
    hides(&err, "other-secret");
    assert_eq!(read(&root.join("from-host.txt")), b"vnl-secret-bytes");
    let mut saw = false;
    while let Some(event) = events.try_recv() {
        assert!(!format!("{event:?}").contains("vnl-secret"));
        assert!(!format!("{event:?}").contains("from-host"));
        if let CoreEvent::TransferCompleted { id } = event {
            assert_eq!(id, session.id);
            saw = true;
        }
    }
    assert!(saw);
    assert!(matches!(
        controller.publish_file(session.id, "nope.txt", b"nope", false),
        Err(Error::NoSession)
    ));
}

#[test]
fn a_dropped_session_does_not_replay_a_file() {
    let scratch = Scratch::new();
    let root = scratch.join("host");
    let host = start_direct();
    let controller = start_direct();
    host.set_transfer_directory(&root).unwrap();
    pair(&host, &controller, true);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_file(session.id, session.generation, "one.txt", b"one", false)
        .unwrap();
    std::fs::write(root.join("one.txt"), b"changed").unwrap();
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
    assert!(matches!(
        controller.submit_file(
            session.id,
            session.generation,
            "replayed.txt",
            b"replayed",
            false
        ),
        Err(Error::NoSession | Error::StaleSession)
    ));
    // The native UI acts on Active immediately, rather than waiting for a
    // later polling tick that can hide an incompletely reopened side stream.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "reconnect did not become active"
        );
        if matches!(events.recv_timeout(Duration::from_millis(50)), Some(CoreEvent::SessionChanged { generation, phase: SessionPhase::Active, .. }) if generation != session.generation)
        {
            break;
        }
    }
    let again = controller.snapshot().unwrap().outgoing.unwrap();
    controller
        .submit_file(again.id, again.generation, "two.txt", b"two", false)
        .unwrap();
    assert_eq!(read(&root.join("one.txt")), b"changed");
    assert_eq!(read(&root.join("two.txt")), b"two");
    assert!(!root.join("replayed.txt").exists());
}

#[test]
fn names_frames_and_the_directory_jail() {
    let long_name = "a".repeat(transfer::MAX_NAME_BYTES + 1);
    let long_component = format!("{}/x", "b".repeat(transfer::MAX_COMPONENT_BYTES + 1));
    let too_many = vec!["c"; transfer::MAX_COMPONENTS + 1].join("/");
    for name in [
        "",
        ".",
        "..",
        "foo/../../x",
        "foo/./x",
        "foo//x",
        "/abs",
        "foo/",
        "foo\\bar",
        "C:foo",
        "a:b",
        "has\0nul",
        long_name.as_str(),
        long_component.as_str(),
        too_many.as_str(),
    ] {
        let err = transfer::check_name(name).unwrap_err();
        if !name.is_empty() {
            hides(&err, name);
        }
        hides(&err, "secret");
    }
    assert!(transfer::check_name("notes/today.txt").is_ok());
    assert!(transfer::check_name(&"a".repeat(transfer::MAX_COMPONENT_BYTES)).is_ok());
    assert!(transfer::checked_size(0).is_ok());
    assert!(transfer::checked_size(MAX_FILE_BYTES as u64).is_ok());
    assert!(matches!(
        transfer::checked_size(MAX_FILE_BYTES as u64 + 1),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        transfer::checked_size(u64::MAX),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        transfer::finish_decision(4, 4, true, None),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        transfer::finish_decision(3, 4, false, None),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        transfer::finish_decision(4, 4, false, Some(Error::TransferDenied)),
        Err(Error::TransferDenied)
    ));
    assert!(transfer::finish_decision(0, 0, false, None).is_ok());

    let frame = Frame::Begin {
        replace: true,
        name: "vnl-secret-name.txt".to_string(),
        size: 4,
    };
    let chunk = Frame::Chunk(b"vnl-secret-bytes".to_vec());
    assert!(!format!("{frame:?}").contains("vnl-secret"));
    assert!(!format!("{chunk:?}").contains("vnl-secret"));
    assert_eq!(
        transfer::decode(&transfer::encode(&frame).unwrap()[4..]).unwrap(),
        frame
    );
    assert_eq!(
        transfer::decode(&transfer::encode(&chunk).unwrap()[4..]).unwrap(),
        chunk
    );
    assert_eq!(
        transfer::decode(&transfer::encode(&Frame::Ready).unwrap()[4..]).unwrap(),
        Frame::Ready
    );
    assert_eq!(
        transfer::decode(&transfer::encode(&Frame::Finish).unwrap()[4..]).unwrap(),
        Frame::Finish
    );
    assert_eq!(
        transfer::decode(&transfer::encode(&Frame::Status(4)).unwrap()[4..]).unwrap(),
        Frame::Status(4)
    );
    assert!(matches!(transfer::check_len(0), Err(Error::Malformed)));
    assert!(matches!(
        transfer::check_len(transfer::MAX_FRAME + 1),
        Err(Error::Malformed)
    ));
    assert!(matches!(transfer::decode(&[5, 9]), Err(Error::Malformed)));
    assert!(matches!(transfer::decode(&[2, 7]), Err(Error::Malformed)));
    let mut huge = vec![2, 0, 0, 1, b'a'];
    huge.extend_from_slice(&u64::MAX.to_be_bytes());
    let decoded = transfer::decode(&huge).unwrap();
    match decoded {
        Frame::Begin { size, .. } => assert!(transfer::checked_size(size).is_err()),
        other => panic!("expected a begin frame, got {other:?}"),
    }
    assert!(matches!(
        transfer::encode(&Frame::Begin {
            replace: false,
            name: "..".to_string(),
            size: 0,
        }),
        Err(Error::InvalidSetting(_))
    ));

    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    let err = transfer::write_file(&root, "foo/../../x", b"vnl-secret-bytes", true).unwrap_err();
    hides(&err, "vnl-secret");
    hides(&err, "foo");
    assert!(!scratch.join("x").exists());
    assert!(!root.join("x").exists());

    transfer::write_file(&root, "notes/today.txt", b"vnl-secret-bytes", false).unwrap();
    assert_eq!(read(&root.join("notes/today.txt")), b"vnl-secret-bytes");
    let err = transfer::write_file(&root, "notes/today.txt", b"new", false).unwrap_err();
    assert!(matches!(err, Error::FileExists));
    hides(&err, "today");
    assert_eq!(read(&root.join("notes/today.txt")), b"vnl-secret-bytes");
    transfer::write_file(&root, "notes/today.txt", b"replaced", true).unwrap();
    assert_eq!(read(&root.join("notes/today.txt")), b"replaced");
    transfer::write_file(&root, "empty.txt", b"", false).unwrap();
    assert_eq!(read(&root.join("empty.txt")), b"");
    assert!(transfer::prepare_directory(&scratch.join("missing")).is_ok());
    assert!(transfer::prepare_directory(&root.join("empty.txt")).is_err());

    for entry in std::fs::read_dir(root.join("notes")).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(
            !name.to_string_lossy().contains("vnlpart"),
            "{}",
            name.to_string_lossy()
        );
    }

    #[cfg(unix)]
    {
        let outside = scratch.join("outside");
        std::fs::write(&outside, b"outside-secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let err = transfer::write_file(&root, "link", b"pwned", true).unwrap_err();
        hides(&err, "outside");
        hides(&err, "pwned");
        assert_eq!(read(&outside), b"outside-secret");
        let outside_dir = scratch.join("out-dir");
        std::fs::create_dir(&outside_dir).unwrap();
        std::fs::write(outside_dir.join("kept"), b"kept-secret").unwrap();
        std::os::unix::fs::symlink(&outside_dir, root.join("dirlink")).unwrap();
        let err = transfer::write_file(&root, "dirlink/new.txt", b"pwned", true).unwrap_err();
        hides(&err, "kept");
        assert_eq!(read(&outside_dir.join("kept")), b"kept-secret");
        assert!(!outside_dir.join("new.txt").exists());
        let linked_root = scratch.join("linked-root");
        std::os::unix::fs::symlink(&root, &linked_root).unwrap();
        let canonical = transfer::prepare_directory(&linked_root).unwrap();
        transfer::write_file(&canonical, "via-root.txt", b"inside", false).unwrap();
        assert_eq!(read(&root.join("via-root.txt")), b"inside");
    }
}
