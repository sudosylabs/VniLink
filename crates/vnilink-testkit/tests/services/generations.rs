// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A service across its connection's generations: a redial carries an open
//! owner to the next generation, fences what the old one handed out, and
//! never revives a closed owner or an old lease.

use expect_test::expect;

use vnilink::{CloseCause, Error, ServiceKind, ServicePhase, SessionPhase};
use vnilink_testkit::desktop::{keyframe, InputLog};
use vnilink_testkit::{traced, Engine, Pair, TestResult, DEFAULT_WAIT};

/// A redial carries an open Desktop to the next generation and cancels the
/// old lease for good, keeps a closed Files closed, and leaves a service on
/// another host alone; unpairing the host cancels the new lease, and only
/// shutting down cancels the other host's.
#[test]
fn a_redial_never_revives_old_leases_or_closed_services() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let a = Engine::start("host-a");
    let b = Engine::start("host-b");
    a.pair_for_control(&controller)?;
    b.pair_for_control(&controller)?;
    let sa = controller.connect_services(&a)?;
    let sb = controller.connect_services(&b)?;
    let core = &controller.core;
    let screen = core.attach_controller_service(sa.id, sa.generation, ServiceKind::Desktop)?;
    let other = core.attach_controller_service(sb.id, sb.generation, ServiceKind::Desktop)?;
    let files = core.attach_controller_service(sa.id, sa.generation, ServiceKind::Files)?;
    let old = core.service_lease(screen.id, screen.generation)?;
    let untouched = core.service_lease(other.id, other.generation)?;
    core.close_controller_service(files.id, files.generation)?;
    core.drop_controller_link(sa.id)?;
    vnilink_testkit::wait_for("the old lease to be cancelled", DEFAULT_WAIT, || {
        old.is_cancelled().then_some(())
    });
    controller.redialed(&sa);
    let next = core.controller_service(screen.id)?;
    assert_eq!(next.id, screen.id, "the redial keeps the service's id");
    assert_eq!(
        next.phase,
        ServicePhase::Active,
        "the redial carries the open Desktop"
    );
    assert!(
        next.generation > screen.generation,
        "under the next generation"
    );
    assert!(old.is_cancelled(), "the old lease stays cancelled");
    assert!(
        !untouched.is_cancelled(),
        "another host's lease is untouched"
    );
    assert_eq!(
        core.controller_service(other.id)?,
        other,
        "another host's service is untouched"
    );
    assert_eq!(
        core.controller_service(files.id)?.phase,
        ServicePhase::Closed(CloseCause::OwnerClose),
        "the redial never revives a closed service"
    );
    assert!(
        matches!(
            core.service_lease(screen.id, screen.generation),
            Err(Error::StaleSession)
        ),
        "the old generation gives no lease"
    );
    assert!(
        matches!(
            core.close_controller_service(screen.id, screen.generation),
            Err(Error::StaleSession)
        ),
        "the old generation cannot close the service"
    );
    let fresh = core.service_lease(next.id, next.generation)?;
    assert!(!fresh.is_cancelled(), "the new generation's lease is live");
    core.unpair(&a.endpoint())?;
    assert!(fresh.is_cancelled(), "unpairing the host cancels its lease");
    assert!(
        !untouched.is_cancelled(),
        "unpairing one host leaves another's lease"
    );
    core.shutdown();
    assert!(
        untouched.is_cancelled(),
        "shutting down cancels every lease"
    );
    // The hosts, and the sessions ending after the unpair, record at moments
    // of their own; the services' phases are the story.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=3 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Desktop service=1 to=active
          service: service phase from=active generation=3 kind=Desktop service=1 to=closed: ParentLoss
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: Shutdown
    "#]],
        || trace.engine_story("controller", &["service"]),
    );
    Ok(())
}

/// A Files pipe open across a redial: the owner moves to the next
/// generation, the old pipe is cancelled and its generation refused, the
/// new pipe opens, and closing the old one later does not touch it.
#[test]
fn a_redial_keeps_the_files_owner_and_fences_its_old_pipe() -> TestResult {
    let trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Files]);
    let controller = &pair.controller.core;
    let session = pair.connect_services();
    let owner = pair.attach(&session, ServiceKind::Files);
    let old = controller.open_files_service(owner.id, owner.generation)?;
    controller.drop_controller_link(session.id)?;
    vnilink_testkit::wait_for("the old pipe to be cancelled", DEFAULT_WAIT, || {
        old.is_cancelled().then_some(())
    });
    pair.controller.redialed(&session);
    let next = controller.controller_service(owner.id)?;
    assert_eq!(
        next.phase,
        ServicePhase::Active,
        "the redial carries the owner"
    );
    assert!(
        next.generation > owner.generation,
        "under the next generation"
    );
    assert!(
        matches!(
            controller.open_files_service(owner.id, owner.generation),
            Err(Error::StaleSession)
        ),
        "the old generation opens no pipe"
    );
    let fresh = controller.open_files_service(next.id, next.generation)?;
    old.close();
    drop(old);
    assert!(
        !fresh.is_cancelled(),
        "closing the old pipe leaves the new one"
    );
    fresh.close();
    pair.host.wait_released(None);
    trace.wait_for_records("service released", 2);
    assert_eq!(
        controller.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "closing the pipe leaves the connection"
    );
    assert_eq!(
        trace.hosted_ends("host"),
        [
            (1, "closed: ParentLoss".to_owned()),
            (2, "closed: PeerClose".to_owned())
        ],
        "the host ends the old generation's Files as its lost parent, and names the \
         next one's close by its viewer"
    );
    // The host names that end on either of two paths, at a moment of its
    // own; the controller's side is the story.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Files service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Files service=1 to=active
          service: service phase from=active generation=3 kind=Files service=1 to=closed: OwnerClose
    "#]],
        || trace.engine_story("controller", &["service"]),
    );
    Ok(())
}

/// A Desktop across a redial: the old handle is fenced, closing it does not
/// retire the replacement, which carries the host's pictures, and the old
/// generation cannot close the owner.
#[test]
fn a_redial_fences_the_old_desktop_handle_without_retiring_its_replacement() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    InputLog::install(&pair.host, (1920, 1080))?;
    let session = pair.connect_services();
    let owner = pair.attach(&session, ServiceKind::Desktop);
    let old = controller.open_desktop_service(owner.id, owner.generation)?;
    controller.drop_controller_link(session.id)?;
    pair.controller.redialed(&session);
    let fresh = controller.controller_service(owner.id)?;
    assert_eq!(
        fresh.phase,
        ServicePhase::Active,
        "the redial carries the Desktop"
    );
    let replacement = controller.open_desktop_service(fresh.id, fresh.generation)?;
    old.close();
    assert!(
        !replacement.is_cancelled(),
        "closing the old handle leaves the replacement"
    );
    let video = replacement.subscribe_video()?;
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    host.publish_desktop_video(remote.id, remote.generation, keyframe(9, vec![9]))?;
    assert_eq!(
        video
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok_or("a picture")?
            .payload,
        vec![9],
        "the replacement carries the host's pictures"
    );
    assert!(
        matches!(
            controller.close_controller_service(owner.id, owner.generation),
            Err(Error::StaleSession)
        ),
        "the old generation cannot close the owner"
    );
    replacement.close();
    pair.host.wait_released(None);
    assert_eq!(
        controller.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "closing the Desktop leaves the connection"
    );
    assert_eq!(
        trace.hosted_ends("host"),
        [
            (1, "closed: ParentLoss".to_owned()),
            (2, "closed: PeerClose".to_owned())
        ],
        "the host ends the old generation's Desktop as its lost parent, and names the \
         next one's close by its viewer"
    );
    // The host names that end on either of two paths, at a moment of its
    // own; the controller's side is the story.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=suspended
          service: service phase from=suspended generation=3 kind=Desktop service=1 to=active
          service: service phase from=active generation=3 kind=Desktop service=1 to=closed: OwnerClose
    "#]],
        || trace.engine_story("controller", &["service"]),
    );
    Ok(())
}
