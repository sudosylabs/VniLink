// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Asking each time: a paired computer is admitted to what its host shares
//! only once the person at the host accepts it, and an approval lasts while
//! it uses the host and for a grace after.

use expect_test::expect;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use vnilink::{CoreEvent, Error, EventSubscription, ServiceKind, SessionInfo, SharingPolicyUpdate};
use vnilink_testkit::events::{drain, next_picked};
use vnilink_testkit::{assert_story, traced, Engine, Pair, TestResult, DEFAULT_WAIT};

/// A pair whose host shares Files and asks its person before admitting.
fn asking_pair() -> Result<Pair, Error> {
    let pair = Pair::for_files();
    pair.host.core.change_sharing_policy(SharingPolicyUpdate {
        unattended: Some(false),
        ..Default::default()
    })?;
    Ok(pair)
}

/// `controller` asks to be admitted to `kind` on `connection`; the answer
/// arrives on the receiver.
fn ask(
    controller: &Engine,
    connection: &SessionInfo,
    kind: ServiceKind,
) -> Receiver<Result<(), Error>> {
    let (tx, rx) = std::sync::mpsc::channel();
    controller
        .core
        .admit(connection.id, connection.generation, kind, move |result| {
            // The test may have stopped listening.
            let _ = tx.send(result);
        });
    rx
}

fn answer(rx: &Receiver<Result<(), Error>>) -> Result<(), Error> {
    rx.recv_timeout(DEFAULT_WAIT).expect("an answer")
}

/// The next request the host's person is asked.
fn requested(events: &EventSubscription) -> u64 {
    next_picked(
        events,
        "an admission request",
        DEFAULT_WAIT,
        |event| match event {
            CoreEvent::AdmissionRequested { request_id, .. } => Some(request_id),
            _ => None,
        },
    )
}

/// Files open only once the host's person accepts the controller; the
/// controller hears that it waits, and an approval admits what follows.
#[test]
fn a_service_opens_once_the_person_at_the_host_accepts() -> TestResult {
    let trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let connection = pair.connect_services();
    assert!(
        matches!(controller.open_files(&connection), Err(Error::NotAdmitted)),
        "a service opened without asking is refused"
    );
    let host_events = host.core.subscribe()?;
    let controller_events = controller.core.subscribe()?;
    let asked = ask(controller, &connection, ServiceKind::Files);
    let request = requested(&host_events);
    assert_eq!(
        host.core.admission_requests()?.len(),
        1,
        "the request waits"
    );
    next_picked(
        &controller_events,
        "the wait to start",
        DEFAULT_WAIT,
        |event| match event {
            CoreEvent::AdmissionWaiting { waiting: true, .. } => Some(()),
            _ => None,
        },
    );
    host.core.answer_admission_request(request, true)?;
    answer(&asked)?;
    next_picked(
        &controller_events,
        "the wait to end",
        DEFAULT_WAIT,
        |event| match event {
            CoreEvent::AdmissionWaiting { waiting: false, .. } => Some(()),
            _ => None,
        },
    );
    assert!(
        host.core.admission_requests()?.is_empty(),
        "the answer ended it"
    );
    drop(controller.open_files(&connection)?);
    assert_story(
        expect![[r#"
            host:
              hosting: service refused connection=1 kind=Files status=not admitted
              hosting: admission asked connection=1 request=1
              hosting: admission answered accept=true request=1
              hosting: service opened connection=1 kind=Some(Files) service=1
              hosting: service closed kind=Files phase=closed: PeerClose service=1
              hosting: service released service=1
        "#]],
        || trace.story(&["hosting"]),
    );
    Ok(())
}

/// A declined request and one nobody answers both refuse the controller,
/// each saying which; neither approves it.
#[test]
fn a_declined_or_unanswered_request_refuses_the_service() -> TestResult {
    let _trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let connection = pair.connect_services();
    let host_events = host.core.subscribe()?;
    let asked = ask(controller, &connection, ServiceKind::Files);
    host.core
        .answer_admission_request(requested(&host_events), false)?;
    assert!(matches!(answer(&asked), Err(Error::Declined)), "declined");
    assert!(
        matches!(controller.open_files(&connection), Err(Error::NotAdmitted)),
        "a decline approves nothing"
    );
    host.core
        .set_admission_timing(Duration::from_millis(200), Duration::from_secs(60));
    let asked = ask(controller, &connection, ServiceKind::Files);
    let request = requested(&host_events);
    assert!(
        matches!(answer(&asked), Err(Error::Unanswered)),
        "unanswered"
    );
    assert!(
        drain(&host_events).iter().any(|event| matches!(
            event,
            CoreEvent::AdmissionRequestEnded { request_id } if *request_id == request
        )),
        "the unanswered request ended"
    );
    assert!(
        matches!(
            host.core.answer_admission_request(request, true),
            Err(Error::NoAdmissionRequest)
        ),
        "an ended request takes no answer"
    );
    Ok(())
}

/// Asks that arrive together, as a desktop's video and input do, are one
/// question, and its one answer is theirs.
#[test]
fn asks_from_one_computer_are_one_question() -> TestResult {
    let trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    pair.host
        .core
        .set_service_sharing(ServiceKind::Desktop, true)?;
    let connection = pair.connect_services();
    let host_events = host.core.subscribe()?;
    let files = ask(controller, &connection, ServiceKind::Files);
    let request = requested(&host_events);
    let desktop = ask(controller, &connection, ServiceKind::Desktop);
    trace.wait_for_records("admission joined", 1);
    host.core.answer_admission_request(request, false)?;
    assert!(
        matches!(answer(&files), Err(Error::Declined)),
        "the first ask"
    );
    assert!(
        matches!(answer(&desktop), Err(Error::Declined)),
        "the one that joined it"
    );
    assert!(
        !drain(&host_events)
            .iter()
            .any(|event| matches!(event, CoreEvent::AdmissionRequested { .. })),
        "the second ask asked nothing more"
    );
    Ok(())
}

/// An approval lasts while the computer uses a service, across a dropped
/// link and its redial, and for the grace after its last service ends;
/// past the grace it asks again.
#[test]
fn an_approval_lasts_through_use_and_its_grace() -> TestResult {
    let _trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let connection = pair.connect_services();
    let host_events = host.core.subscribe()?;
    let asked = ask(controller, &connection, ServiceKind::Files);
    host.core
        .answer_admission_request(requested(&host_events), true)?;
    answer(&asked)?;
    let (first, pipe) = controller.open_files(&connection)?;
    controller.core.drop_controller_link(connection.id)?;
    let redialed = controller.redialed(&connection);
    drop(pipe);
    let first = controller.core.controller_service(first.id)?;
    controller
        .core
        .close_controller_service(first.id, first.generation)?;
    controller.core.forget_controller_service(first.id)?;
    answer(&ask(controller, &redialed, ServiceKind::Files))?;
    assert!(
        !drain(&host_events)
            .iter()
            .any(|event| matches!(event, CoreEvent::AdmissionRequested { .. })),
        "the redial asked nothing"
    );
    let (service, pipe) = controller.open_files(&redialed)?;
    // From now on an ended service's grace is over at once.
    host.core
        .set_admission_timing(Duration::from_secs(30), Duration::ZERO);
    drop(pipe);
    controller
        .core
        .close_controller_service(service.id, service.generation)?;
    vnilink_testkit::wait_for("the host to end the service", DEFAULT_WAIT, || {
        host.core.hosted_services().ok()?.is_empty().then_some(())
    });
    let again = ask(controller, &redialed, ServiceKind::Files);
    host.core
        .answer_admission_request(requested(&host_events), false)?;
    assert!(
        matches!(answer(&again), Err(Error::Declined)),
        "asked again"
    );
    Ok(())
}

/// A screenshot never asks: it is refused while no approval is live and
/// taken while one is.
#[test]
fn a_screenshot_never_asks() -> TestResult {
    let _trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    pair.host
        .core
        .set_service_sharing(ServiceKind::Desktop, true)?;
    let connection = pair.connect_services();
    let host_events = host.core.subscribe()?;
    assert!(
        matches!(
            answer(&ask(controller, &connection, ServiceKind::ScreenSnapshot)),
            Err(Error::NotAdmitted)
        ),
        "refused without asking"
    );
    assert!(
        !drain(&host_events)
            .iter()
            .any(|event| matches!(event, CoreEvent::AdmissionRequested { .. })),
        "the person was not asked"
    );
    let asked = ask(controller, &connection, ServiceKind::Files);
    host.core
        .answer_admission_request(requested(&host_events), true)?;
    answer(&asked)?;
    answer(&ask(controller, &connection, ServiceKind::ScreenSnapshot))?;
    Ok(())
}

/// Unattended sharing admits without asking, and turning it off forgets
/// what it admitted.
#[test]
fn unattended_sharing_asks_nothing_until_turned_off() -> TestResult {
    let _trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let unattended = |on| {
        host.core.change_sharing_policy(SharingPolicyUpdate {
            unattended: Some(on),
            ..Default::default()
        })
    };
    unattended(true)?;
    let connection = pair.connect_services();
    answer(&ask(controller, &connection, ServiceKind::Files))?;
    let (service, pipe) = controller.open_files(&connection)?;
    drop(pipe);
    controller
        .core
        .close_controller_service(service.id, service.generation)?;
    vnilink_testkit::wait_for("the host to end the service", DEFAULT_WAIT, || {
        host.core.hosted_services().ok()?.is_empty().then_some(())
    });
    unattended(false)?;
    assert!(
        matches!(controller.open_files(&connection), Err(Error::NotAdmitted)),
        "the grace its use left is forgotten"
    );
    Ok(())
}

/// Forgetting the computer ends its request; nothing is left to answer.
#[test]
fn forgetting_the_computer_ends_its_request() -> TestResult {
    let _trace = traced();
    let pair = asking_pair()?;
    let (host, controller) = (&pair.host, &pair.controller);
    let connection = pair.connect_services();
    let host_events = host.core.subscribe()?;
    let asked = ask(controller, &connection, ServiceKind::Files);
    let request = requested(&host_events);
    host.core.unpair(&controller.endpoint())?;
    assert!(answer(&asked).is_err(), "the ask is refused");
    assert!(
        host.core.admission_requests()?.is_empty(),
        "no request is left"
    );
    assert!(
        matches!(
            host.core.answer_admission_request(request, true),
            Err(Error::NoAdmissionRequest)
        ),
        "and none can be answered"
    );
    Ok(())
}
