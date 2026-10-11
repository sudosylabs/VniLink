use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{Core, Error, Grants, HostClipboard, ServiceKind, SessionPhase};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait(mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Clipboard(Arc<Mutex<Vec<String>>>);
impl HostClipboard for Clipboard {
    fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.0.lock().unwrap().push(text.into());
        Ok(())
    }
}
fn prepare(host: &Core, controller: &Core) -> crate::SessionInfo {
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    pair_for_control(host, controller);
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(pending.id).unwrap()
}
fn open(controller: &Core, parent: &crate::SessionInfo) -> crate::RemoteDesktopService {
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap()
}
fn owner(host: &Core) -> crate::ServiceInfo {
    host.hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServiceKind::Desktop)
        .unwrap()
}
fn grant(host: &Core, controller: &Core, clipboard: bool) {
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            view: true,
            control: true,
            clipboard,
            transfer: false,
        },
    )
    .unwrap();
}

#[test]
fn desktop_clipboard_acknowledges_both_directions_and_isolates_reopen_and_revoke() {
    let host = start_direct();
    let controller = start_direct();
    let texts = Arc::new(Mutex::new(Vec::new()));
    let parent = prepare(&host, &controller);
    host.set_clipboard(Box::new(Clipboard(texts.clone())))
        .unwrap();
    let desktop = open(&controller, &parent);
    let remote = owner(&host);
    assert_eq!(desktop.clipboard_text().unwrap(), None);
    assert!(matches!(
        desktop.submit_clipboard("denied"),
        Err(Error::ClipboardDenied)
    ));
    assert!(matches!(
        host.publish_desktop_clipboard(remote.id, remote.generation, "denied"),
        Err(Error::ClipboardDenied)
    ));
    assert!(texts.lock().unwrap().is_empty());
    grant(&host, &controller, true);
    let events = controller.subscribe().unwrap();
    std::thread::scope(|scope| {
        let outgoing = scope.spawn(|| {
            controller.submit_controller_desktop_clipboard(
                desktop.info().id,
                desktop.info().generation,
                "controller copy",
            )
        });
        let incoming = scope
            .spawn(|| host.publish_desktop_clipboard(remote.id, remote.generation, "host copy"));
        outgoing.join().unwrap().unwrap();
        incoming.join().unwrap().unwrap();
    });
    assert_eq!(*texts.lock().unwrap(), vec!["controller copy"]);
    assert_eq!(
        controller
            .controller_desktop_clipboard_text(desktop.info().id, desktop.info().generation)
            .unwrap()
            .as_deref(),
        Some("host copy")
    );
    let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(&event, crate::CoreEvent::DesktopClipboardReceived { service } if service.id == desktop.info().id && service.generation == desktop.info().generation)
    );
    assert!(!format!("{event:?}").contains("host copy"));
    let maximum = "é".repeat(crate::MAX_CLIPBOARD_TEXT / 2);
    desktop.submit_clipboard(&maximum).unwrap();
    assert_eq!(texts.lock().unwrap().last(), Some(&maximum));
    assert!(desktop.submit_clipboard(&(maximum.clone() + "a")).is_err());
    desktop.submit_clipboard("").unwrap();
    assert_eq!(texts.lock().unwrap().last().unwrap(), "");
    host.publish_desktop_clipboard(remote.id, remote.generation, "")
        .unwrap();
    assert_eq!(desktop.clipboard_text().unwrap().as_deref(), Some(""));
    grant(&host, &controller, false);
    assert!(matches!(
        desktop.submit_clipboard("revoked"),
        Err(Error::ClipboardDenied)
    ));
    assert!(!desktop.is_cancelled());
    assert_eq!(controller.controller_session(parent.id).unwrap(), parent);
    grant(&host, &controller, true);
    desktop.submit_clipboard("restored").unwrap();
    desktop.close();
    wait(|| host.hosted_services().unwrap().is_empty());
    let fresh = open(&controller, &parent);
    assert_eq!(fresh.clipboard_text().unwrap(), None);
    assert!(desktop.clipboard_text().is_err());
    assert!(controller
        .controller_desktop_clipboard_text(desktop.info().id, desktop.info().generation)
        .is_err());
    assert!(controller
        .submit_controller_desktop_clipboard(
            desktop.info().id,
            desktop.info().generation,
            "late controller"
        )
        .is_err());
    assert_eq!(fresh.clipboard_text().unwrap(), None);
    assert!(host
        .publish_desktop_clipboard(remote.id, remote.generation, "late")
        .is_err());
    desktop.close();
    fresh.submit_clipboard("fresh").unwrap();
    assert_eq!(texts.lock().unwrap().last().unwrap(), "fresh");
}

#[test]
fn blocked_clipboard_callback_does_not_block_snapshot_or_desktop_close() {
    struct Blocking {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl HostClipboard for Blocking {
        fn set_text(&mut self, _: &str) -> Result<(), Error> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
    }
    let host = Arc::new(start_direct());
    let controller = start_direct();
    let parent = prepare(&host, &controller);
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    host.set_clipboard(Box::new(Blocking {
        entered,
        release: released,
    }))
    .unwrap();
    grant(&host, &controller, true);
    let desktop = Arc::new(open(&controller, &parent));
    let worker_desktop = desktop.clone();
    let (done, result) = std::sync::mpsc::channel();
    let request = std::thread::spawn(move || {
        done.send(worker_desktop.submit_clipboard("blocked"))
            .unwrap();
    });
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    let snapshot_host = host.clone();
    let (snapshot_done, snapshot_rx) = std::sync::mpsc::channel();
    let snapshot = std::thread::spawn(move || {
        snapshot_done.send(snapshot_host.snapshot()).unwrap();
    });
    let snapshot_result = snapshot_rx.recv_timeout(Duration::from_secs(1));
    desktop.close();
    let request_result = result.recv_timeout(Duration::from_secs(1));
    release.send(()).unwrap();
    request.join().unwrap();
    snapshot.join().unwrap();
    assert!(snapshot_result.unwrap().is_ok());
    assert!(request_result.unwrap().is_err());
    assert_eq!(controller.controller_session(parent.id).unwrap(), parent);
}

#[test]
fn desktop_clipboards_for_two_computers_are_independent_and_unavailable_apply_is_nonfatal() {
    let controller = start_direct();
    let hosts = [start_direct(), start_direct()];
    let mut desktops = Vec::new();
    let mut owners = Vec::new();
    for host in &hosts {
        let parent = prepare(host, &controller);
        grant(host, &controller, true);
        desktops.push(open(&controller, &parent));
        owners.push(owner(host));
    }
    assert!(matches!(
        desktops[0].submit_clipboard("no adapter"),
        Err(Error::Unsupported(_))
    ));
    assert!(!desktops[0].is_cancelled());
    for (index, host) in hosts.iter().enumerate() {
        host.publish_desktop_clipboard(
            owners[index].id,
            owners[index].generation,
            if index == 0 { "one" } else { "two" },
        )
        .unwrap();
    }
    assert_eq!(
        desktops[0].clipboard_text().unwrap().as_deref(),
        Some("one")
    );
    assert_eq!(
        desktops[1].clipboard_text().unwrap().as_deref(),
        Some("two")
    );
    desktops[0].close();
    hosts[1]
        .publish_desktop_clipboard(owners[1].id, owners[1].generation, "still active")
        .unwrap();
    assert_eq!(
        desktops[1].clipboard_text().unwrap().as_deref(),
        Some("still active")
    );
    assert!(!desktops[1].is_cancelled());
}

#[test]
fn clipboard_ack_timeout_cancels_queued_text_and_keeps_video_active() {
    struct Blocking {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        applied: Arc<Mutex<Vec<String>>>,
    }
    impl HostClipboard for Blocking {
        fn set_text(&mut self, text: &str) -> Result<(), Error> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            self.applied.lock().unwrap().push(text.into());
            Ok(())
        }
    }
    let host = start_direct();
    let controller = start_direct();
    let parent = prepare(&host, &controller);
    let (entered, entry) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_clipboard(Box::new(Blocking {
        entered,
        release: released,
        applied: applied.clone(),
    }))
    .unwrap();
    grant(&host, &controller, true);
    let desktop = Arc::new(open(&controller, &parent));
    let first = desktop.clone();
    let request = std::thread::spawn(move || first.submit_clipboard("in flight"));
    entry.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = desktop.clone();
    let queued = std::thread::spawn(move || second.submit_clipboard("queued"));
    let first_result = request.join().unwrap();
    let queued_result = queued.join().unwrap();
    // Wait for transport teardown, not merely the synchronous caller timeout.
    wait(|| desktop.clipboard_text().is_err());
    release.send(()).unwrap();
    assert!(matches!(first_result, Err(Error::Timeout)));
    assert!(queued_result.is_err());
    wait(|| applied.lock().unwrap().len() == 1);
    assert_eq!(*applied.lock().unwrap(), vec!["in flight"]);
    assert!(!desktop.is_cancelled());
    let remote = owner(&host);
    let video = desktop.subscribe_video().unwrap();
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        crate::EncodedFrame::h264(true, 1, vec![1]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        vec![1]
    );
    assert_eq!(controller.controller_session(parent.id).unwrap(), parent);
}
