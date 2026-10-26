//! File selections copied over a control session: names and sizes offered
//! for a later paste, each under the host's grant, each offer a new
//! revision that is never reused.

use expect_test::expect;

use super::{assert_unrecorded, paired, sharing};
use vnilink::{
    ClipboardFileEntry, CoreEvent, Error, FileSelection, SessionPhase, MAX_CLIPBOARD_FILES,
};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

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

/// Publishes `offer` from `host` on its session `hosted`, retrying until
/// its side of the clipboard stream is open: a controller sends Ready
/// before the host installs its sender. Every other failure fails at once.
fn publish_once_open(host: &Engine, hosted: u64, offer: &FileSelection) {
    wait_for(
        "the host's selection to be sent",
        DEFAULT_WAIT,
        || match host.core.publish_clipboard_files(hosted, offer) {
            Ok(()) => Some(()),
            Err(Error::Unsupported("clipboard stream is not open")) => None,
            Err(error) => panic!("the host's publication failed: {error}"),
        },
    );
}

/// Waits until `controller` holds a selection from its host.
fn wait_for_selection(controller: &Engine, what: &str) -> FileSelection {
    wait_for(what, DEFAULT_WAIT, || {
        Some(
            controller
                .core
                .clipboard_files()
                .expect("the clipboard files")?
                .selection,
        )
    })
}

/// Without the clipboard grant neither side's selection crosses and the
/// session carries on; once granted both cross, and the refused offer
/// spent a revision that is never reused.
#[test]
fn a_selection_needs_the_grant_and_the_session_lives_without_it() -> TestResult {
    let trace = traced();
    let pair = paired(false);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let offer = selection(vec![entry("report.pdf", 12_000)]);
    // No clipboard is installed: a file offer is stored, never handed to
    // a text clipboard.
    assert!(
        matches!(
            controller.submit_clipboard_files(session.id, session.generation, &offer),
            Err(Error::ClipboardDenied)
        ),
        "an offer without the grant is refused"
    );
    assert!(
        matches!(
            host.publish_clipboard_files(hosted.id, &offer),
            Err(Error::ClipboardDenied)
        ),
        "the host publishes nothing without the grant"
    );
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_none(),
        "a refused offer is not stored on the host"
    );
    assert!(
        controller.clipboard_files()?.is_none(),
        "a refused publication does not arrive"
    );
    assert_eq!(
        controller.snapshot()?.outgoing.ok_or("the session")?.phase,
        SessionPhase::Active,
        "a refused offer leaves the session up"
    );
    assert!(
        matches!(
            controller.submit_clipboard_files(session.id, session.generation + 1, &offer),
            Err(Error::StaleSession)
        ),
        "an offer for another generation is stale"
    );
    host.set_desktop_sharing(sharing(true))?;
    controller.submit_clipboard_files(session.id, session.generation, &offer)?;
    let stored = host
        .incoming_clipboard_files(hosted.id)?
        .ok_or("the granted offer")?;
    // Issuance precedes the round trip so concurrent offers never share a
    // revision; the refused one spent revision 1.
    assert_eq!(
        stored.revision, 2,
        "a refused offer's revision is never reused"
    );
    assert_eq!(stored.selection, offer, "the host stores the offer");
    publish_once_open(&pair.host, hosted.id, &offer);
    wait_for("the host's selection to arrive", DEFAULT_WAIT, || {
        controller
            .clipboard_files()
            .ok()?
            .is_some_and(|stored| stored.selection == offer)
            .then_some(())
    });
    trace.wait_for_records("clipboard received", 3);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files refused=clipboard denied
          clipboard: clipboard not delivered direction=to the host kind=files refused=clipboard denied
          clipboard: clipboard not delivered direction=to the host kind=files refused=stale session
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
          clipboard: clipboard received amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files refused=clipboard denied
          clipboard: clipboard not delivered direction=to the controller kind=files refused=clipboard denied
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard sent amount=1 kind=files
    "#]].assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["report"]);
    Ok(())
}

/// Each offer replaces the last, a clear withdraws it, and each spends a
/// revision, so a paste prepared against any older one stays stale across
/// a clear.
#[test]
fn replacing_and_clearing_a_selection_keeps_its_revisions_rising() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let first = selection(vec![entry("a.txt", 3)]);
    let second = selection(vec![
        entry("nested/b.txt", 4),
        ClipboardFileEntry {
            path: "empty-dir".into(),
            size: 0,
            dir: true,
        },
    ]);
    controller.submit_clipboard_files(session.id, session.generation, &first)?;
    controller.submit_clipboard_files(session.id, session.generation, &second)?;
    let stored = host
        .incoming_clipboard_files(hosted.id)?
        .ok_or("the second offer")?;
    assert_eq!(stored.revision, 2, "the second offer is revision 2");
    assert_eq!(
        stored.selection, second,
        "the second offer replaces the first"
    );
    controller.submit_clipboard_files(session.id, session.generation, &FileSelection::default())?;
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_none(),
        "an empty offer clears the host's"
    );
    controller.submit_clipboard_files(session.id, session.generation, &first)?;
    assert_eq!(
        host.incoming_clipboard_files(hosted.id)?
            .ok_or("the offer after the clear")?
            .revision,
        4,
        "a clear spends revision 3"
    );
    trace.wait_for_records("clipboard received", 4);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
          clipboard: clipboard sent amount=2 kind=files
          clipboard: clipboard answered amount=2 kind=files
          clipboard: clipboard sent amount=0 kind=files
          clipboard: clipboard answered amount=0 kind=files
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard received amount=2 kind=files
          clipboard: clipboard received amount=0 kind=files
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["a.txt", "nested", "empty-dir"]);
    Ok(())
}

/// A selection out of bounds (too many entries, a name twice, a name that
/// leaves the selection or is absolute, a sized folder, names too long
/// together) is refused on either side before anything is sent.
#[test]
fn a_selection_out_of_bounds_is_refused_before_it_is_sent() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let too_many = FileSelection {
        entries: (0..MAX_CLIPBOARD_FILES + 1)
            .map(|n| entry(&format!("f{n}.txt"), 1))
            .collect(),
    };
    let twice = selection(vec![entry("same.txt", 1), entry("same.txt", 2)]);
    let escape = selection(vec![entry("../outside.txt", 1)]);
    let absolute = selection(vec![entry("/etc/hosts", 1)]);
    let sized_folder = selection(vec![ClipboardFileEntry {
        path: "dir".into(),
        size: 9,
        dir: true,
    }]);
    // Each name passes on its own (two short components) while together
    // they break the selection's budget: 256 * (11 + 250).
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
    assert!(
        matches!(
            wide.validate(),
            Err(Error::InvalidSetting(
                "clipboard file selection is larger than 64 KiB"
            ))
        ),
        "the wide selection is over the budget, not refused for its names"
    );
    for bad in [&too_many, &twice, &escape, &absolute, &sized_folder, &wide] {
        assert!(
            matches!(
                controller.submit_clipboard_files(session.id, session.generation, bad),
                Err(Error::InvalidSetting(_))
            ),
            "an offer out of bounds was accepted: {bad:?}"
        );
        assert!(
            matches!(
                host.publish_clipboard_files(hosted.id, bad),
                Err(Error::InvalidSetting(_))
            ),
            "a publication out of bounds was accepted: {bad:?}"
        );
    }
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_none(),
        "no offer reached the host"
    );
    assert!(
        controller.clipboard_files()?.is_none(),
        "no publication reached the controller"
    );
    trace.wait_for_records("clipboard stream opened", 1);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard not delivered direction=to the host kind=files reason=clipboard file selection holds too many entries refused=invalid setting
          clipboard: clipboard not delivered direction=to the host kind=files reason=clipboard file selection lists a path twice refused=invalid setting
          clipboard: clipboard not delivered direction=to the host kind=files reason=the path is not a safe relative path refused=invalid setting
          clipboard: clipboard not delivered direction=to the host kind=files reason=the path is not a safe relative path refused=invalid setting
          clipboard: clipboard not delivered direction=to the host kind=files reason=clipboard directory selection must report size zero refused=invalid setting
          clipboard: clipboard not delivered direction=to the host kind=files reason=clipboard file selection is larger than 64 KiB refused=invalid setting
        host:
          clipboard: clipboard not delivered direction=to the controller kind=files reason=clipboard file selection holds too many entries refused=invalid setting
          clipboard: clipboard not delivered direction=to the controller kind=files reason=clipboard file selection lists a path twice refused=invalid setting
          clipboard: clipboard not delivered direction=to the controller kind=files reason=the path is not a safe relative path refused=invalid setting
          clipboard: clipboard not delivered direction=to the controller kind=files reason=the path is not a safe relative path refused=invalid setting
          clipboard: clipboard not delivered direction=to the controller kind=files reason=clipboard directory selection must report size zero refused=invalid setting
          clipboard: clipboard not delivered direction=to the controller kind=files reason=clipboard file selection is larger than 64 KiB refused=invalid setting
    "#]].assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["same.txt", "outside", "/etc/hosts", "xxxxxxxx"]);
    Ok(())
}

/// Withdrawing the grant withdraws the host's stored offer, while the
/// session carries on and the controller keeps the names it was sent, inert
/// until a paste asks the host again; granting it again starts from a
/// revision never used.
#[test]
fn withdrawing_the_grant_drops_the_hosts_offer_and_the_session_lives() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let from_controller = selection(vec![entry("up.bin", 8)]);
    let from_host = selection(vec![entry("down.bin", 9)]);
    controller.submit_clipboard_files(session.id, session.generation, &from_controller)?;
    publish_once_open(&pair.host, hosted.id, &from_host);
    wait_for_selection(&pair.controller, "the host's selection to arrive");
    host.set_desktop_sharing(sharing(false))?;
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_none(),
        "a selection is a live paste capability, withdrawn with the grant"
    );
    assert_eq!(
        controller.snapshot()?.outgoing.ok_or("the session")?.phase,
        SessionPhase::Active,
        "withdrawing the grant leaves the session up"
    );
    assert_eq!(
        controller
            .clipboard_files()?
            .ok_or("the host's selection")?
            .selection,
        from_host,
        "the controller keeps the names it was sent"
    );
    host.set_desktop_sharing(sharing(true))?;
    controller.submit_clipboard_files(session.id, session.generation, &from_controller)?;
    assert_eq!(
        host.incoming_clipboard_files(hosted.id)?
            .ok_or("the offer after the grant")?
            .revision,
        2,
        "the withdrawn offer's revision is never reused"
    );
    trace.wait_for_records("clipboard received", 3);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["up.bin", "down.bin"]);
    Ok(())
}

/// A peer the host stops trusting loses its session and the offer it made.
#[test]
fn a_revoked_peer_loses_its_offer() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    controller.submit_clipboard_files(
        session.id,
        session.generation,
        &selection(vec![entry("up.bin", 8)]),
    )?;
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_some(),
        "the host stores the offer"
    );
    host.revoke(&pair.controller.endpoint())?;
    assert!(
        matches!(
            host.incoming_clipboard_files(hosted.id),
            Err(Error::NoSession)
        ),
        "a revoked peer's session and offer are gone"
    );
    trace.wait_for_records("clipboard received", 1);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(
        &trace
            .tell(&["clipboard"])
            .only(super::CROSSINGS)
            .to_string(),
    );
    assert_unrecorded(&trace, &["up.bin"]);
    Ok(())
}

/// With two controllers connected, the host's selection goes to the
/// session it admitted last; the older one is busy and is sent nothing.
#[test]
fn the_hosts_selection_reaches_only_the_newest_session() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, first) = (&pair.host, &pair.controller);
    let second = pair.add_viewer("other-controller");
    let _one = first.connect_control(host)?;
    let _two = second.connect_control(host)?;
    let hosted = host.core.hosted_sessions()?;
    assert_eq!(hosted.len(), 2, "the host serves both controllers");
    let (older, newer) = (hosted[0].id, hosted[1].id);
    let offer = selection(vec![entry("new.bin", 1)]);
    assert!(
        matches!(
            host.core.publish_clipboard_files(older, &offer),
            Err(Error::Busy)
        ),
        "the older session does not hold the clipboard"
    );
    publish_once_open(host, newer, &offer);
    wait_for_selection(&second, "the newest controller's selection");
    assert!(
        first.core.clipboard_files()?.is_none(),
        "the older controller is sent nothing"
    );
    trace.wait_for_records("clipboard received", 1);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
        host:
          clipboard: clipboard not delivered direction=to the controller kind=files refused=busy
          clipboard: clipboard sent amount=1 kind=files
        other-controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["new.bin"]);
    Ok(())
}

/// A controller connected to two hosts cannot say whose selection it
/// holds, so it refuses to guess; reads naming a session and generation
/// stay exact.
#[test]
fn a_controller_of_two_hosts_reads_selections_by_session() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host");
    let b = Engine::start("other-host");
    for host in [&a, &b] {
        host.pair_with(&controller)?;
        host.core.set_desktop_sharing(sharing(true))?;
    }
    let to_a = controller.connect_control(&a)?;
    let to_b = controller.connect_control(&b)?;
    assert!(
        matches!(controller.core.clipboard_files(), Err(Error::Busy)),
        "with two sessions the controller refuses to guess"
    );
    assert!(
        controller
            .core
            .session_clipboard_files(to_a.id, to_a.generation)?
            .is_none(),
        "the first session holds no selection"
    );
    assert!(
        controller
            .core
            .session_clipboard_files(to_b.id, to_b.generation)?
            .is_none(),
        "the second session holds no selection"
    );
    assert!(
        matches!(
            controller
                .core
                .session_clipboard_files(to_a.id, to_a.generation + 1),
            Err(Error::StaleSession)
        ),
        "a read for another generation is stale"
    );
    trace.wait_for_records("clipboard stream opened", 2);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard stream opened dial=true
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    Ok(())
}

/// Each side is told a selection arrived, for its own session, and no
/// event names a file.
#[test]
fn each_side_is_told_a_selection_arrived_without_its_names() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let host_events = host.subscribe()?;
    let controller_events = controller.subscribe()?;
    let secret = selection(vec![entry("secret-plan.pdf", 100)]);
    controller.submit_clipboard_files(session.id, session.generation, &secret)?;
    publish_once_open(&pair.host, hosted.id, &secret);
    wait_for_selection(&pair.controller, "the host's selection to arrive");
    let told = |events: &vnilink::EventSubscription, session: u64, side: &str| {
        let mut told = false;
        while let Some(event) = events.try_recv() {
            assert!(
                !format!("{event:?}").contains("secret-plan"),
                "an event names a file: {event:?}"
            );
            if let CoreEvent::ClipboardFilesReceived { id } = event {
                assert_eq!(id, session, "the {side}'s event names its session");
                told = true;
            }
        }
        assert!(told, "the {side} is told a selection arrived");
    };
    told(&host_events, hosted.id, "host");
    told(&controller_events, session.id, "controller");
    trace.wait_for_records("clipboard received", 2);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
          clipboard: clipboard received amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard sent amount=1 kind=files
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["secret-plan"]);
    Ok(())
}
