//! Privacy: one controller at a time hides the host's screen and blocks its
//! input, under the host's policy and grant, and gives both back when it
//! closes, loses the grant or the policy narrows.

use expect_test::expect;
use std::sync::Arc;

use vnilink::{Error, PrivacyState, ServiceKind, SessionPhase, SystemAction, SystemPolicy};
use vnilink_testkit::store::{Fault, Installed};
use vnilink_testkit::system::FakeSystem;
use vnilink_testkit::{pair_cores_for_control, traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

use super::{sharing, PRIVACY};

/// Privacy is one controller's at a time: while one holds it, another is
/// refused; closing the holder's service gives the screen and input back,
/// and the other may then take it.
#[test]
fn privacy_is_one_controller_s_until_its_service_closes() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    let second = pair.add_viewer("other");
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    let other_connection = second.connect_services(&pair.host)?;
    let other = second.open_system(&other_connection)?;
    let held = system.set_privacy(true, true)?;
    assert!(
        held.display_private && held.input_blocked,
        "privacy hides the screen and blocks input"
    );
    assert_eq!(
        held.holder_connection,
        Some(connection.id),
        "privacy names the connection holding it"
    );
    {
        let calls = fake.calls();
        assert!(
            calls.display.restricted && calls.input.restricted,
            "the host's OS hides the screen and blocks input"
        );
    }
    assert!(
        matches!(other.set_privacy(true, false), Err(Error::Busy)),
        "another controller cannot take privacy while it is held"
    );
    assert_eq!(
        host.system_privacy()?,
        held,
        "the host reports the privacy held"
    );
    system.close();
    wait_for("privacy to be given back", DEFAULT_WAIT, || {
        let calls = fake.calls();
        (!calls.display.restricted && !calls.input.restricted).then_some(())
    });
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "the host reports privacy given back"
    );
    let retaken = other.set_privacy(true, false)?;
    assert!(
        retaken.display_private && !retaken.input_blocked,
        "the other controller takes the privacy given back"
    );
    drop(other);
    wait_for("the other's privacy to be given back", DEFAULT_WAIT, || {
        (!fake.calls().display.restricted).then_some(())
    });
    trace.wait_for_records("privacy restored", 2);
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy refused connection=2 display_private=true input_blocked=false why=another viewer holds privacy
          system: privacy restored connection=1 display=true input=true
          system: privacy applied connection=2 display_ok=true display_private=true input_blocked=false input_ok=true
          system: privacy restored connection=2 display=true input=false
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// Withdrawing the System grant gives privacy back and refuses the
/// service's later requests.
#[test]
fn revoke_restores_privacy_and_fences_late_requests() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(false, true)?;
    assert!(fake.calls().input.restricted, "the host's input is blocked");
    host.set_service_sharing(ServiceKind::System, false)?;
    wait_for("input to be given back", DEFAULT_WAIT, || {
        (!fake.calls().input.restricted).then_some(())
    });
    assert!(
        system.set_privacy(false, false).is_err(),
        "a withdrawn service cannot change privacy"
    );
    assert!(
        system.execute(SystemAction::Lock).is_err(),
        "a withdrawn service cannot lock"
    );
    trace.wait_for_records("privacy restored", 1);
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=false input_blocked=true input_ok=true
          system: privacy restored connection=1 display=false input=true
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// Privacy the host's policy does not allow is refused, while the host
/// can still read what its OS supports and that nothing is private.
#[test]
fn privacy_without_policy_is_unauthorized_and_local_state_is_pollable() -> TestResult {
    let _trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    assert!(
        matches!(system.set_privacy(true, false), Err(Error::Unauthorized)),
        "privacy the policy does not allow is refused"
    );
    let (support, session) = host.system_support()?;
    assert!(
        support.lock && support.privacy && support.input_block,
        "the host reads what its OS supports"
    );
    assert_eq!(session.session, "fake", "the host reads its OS's session");
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "nothing is private"
    );
    Ok(())
}

/// Narrowing the host's policy gives privacy back before the change
/// returns.
#[test]
fn narrowing_the_policy_gives_privacy_back_at_once() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    host.set_system_policy(SystemPolicy::default())?;
    assert!(
        !host.system_privacy()?.input_blocked,
        "a narrowed policy keeps no input blocked"
    );
    assert!(
        !fake.calls().display.restricted,
        "a narrowed policy keeps no screen hidden"
    );
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy restored connection=1 display=true input=true
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// A policy change that fails to save leaves the policy and the held
/// privacy as they were; a saved narrowing restores each facility exactly
/// once, and both controllers' services and connections survive it, with
/// privacy refused to them again.
#[test]
fn privacy_narrowing_restores_exactly_once_and_failed_save_preserves_lease() -> TestResult {
    let trace = traced();
    let stored = Installed::faulty("host");
    let host = &stored.core;
    let fake = Arc::new(FakeSystem::full());
    host.set_system_actions(Some(
        Arc::clone(&fake) as Arc<dyn vnilink::HostSystemActions>
    ))?;
    let holder = Engine::start("holder");
    pair_cores_for_control(host, &holder.core)?;
    let sibling = Engine::start("sibling");
    pair_cores_for_control(host, &sibling.core)?;
    host.set_service_sharing(ServiceKind::System, true)?;
    host.set_system_policy(PRIVACY)?;
    let connection = holder.connect_services_to(&host.endpoint_id())?;
    let system = holder.open_system(&connection)?;
    let sibling_connection = sibling.connect_services_to(&host.endpoint_id())?;
    let sibling_system = sibling.open_system(&sibling_connection)?;
    system.set_privacy(true, true)?;
    let done = |fake: &FakeSystem| {
        let calls = fake.calls();
        (calls.display.done(), calls.input.done())
    };
    assert_eq!(done(&fake), (1, 1), "each facility was restricted once");

    stored.store.fail_next_save(Fault::Locked);
    assert!(
        matches!(
            host.set_system_policy(SystemPolicy::default()),
            Err(Error::StoreLocked)
        ),
        "a policy that fails to save says why"
    );
    assert!(
        host.system_policy()?.allow_privacy,
        "a policy that failed to save is not applied"
    );
    let live = host.system_privacy()?;
    assert!(
        live.display_private && live.input_blocked,
        "a policy that failed to save leaves privacy held"
    );
    assert_eq!(
        done(&fake),
        (1, 1),
        "a policy that failed to save restores nothing"
    );

    host.set_system_policy(SystemPolicy::default())?;
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "a saved narrowing gives privacy back"
    );
    {
        let calls = fake.calls();
        assert_eq!(
            (calls.display.done(), calls.input.done()),
            (2, 2),
            "a saved narrowing restores each facility exactly once"
        );
        assert!(!calls.display.restricted && !calls.input.restricted);
    }
    assert_eq!(system.privacy()?, PrivacyState::default());
    assert_eq!(sibling_system.privacy()?, PrivacyState::default());
    assert!(
        matches!(system.set_privacy(true, false), Err(Error::Unauthorized)),
        "privacy stays refused under the narrowed policy"
    );
    assert_eq!(
        holder.core.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "the holder's connection survives the narrowing"
    );
    assert_eq!(
        sibling
            .core
            .controller_session(sibling_connection.id)?
            .phase,
        SessionPhase::Active,
        "the sibling's connection survives the narrowing"
    );
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy restored connection=1 display=true input=true
          system: privacy refused connection=1 display_private=true input_blocked=false why=the host's policy does not allow privacy
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}

/// Privacy released on the host itself, as its user can, is what the host
/// and the controller then read, and enabling it again restricts the host
/// again.
#[test]
fn local_privacy_release_is_observed_and_reenable_reapplies_native_restrictions() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    {
        let mut calls = fake.calls();
        calls.display.restricted = false;
        calls.input.restricted = false;
    }
    let observed = host.system_privacy()?;
    assert!(
        !observed.display_private && !observed.input_blocked,
        "the host reads privacy released locally"
    );
    assert!(
        !system.privacy()?.display_private,
        "the controller reads privacy released locally"
    );
    system.set_privacy(true, true)?;
    assert!(
        fake.calls().display.restricted,
        "enabling again hides the screen again"
    );
    assert!(
        host.system_privacy()?.input_blocked,
        "enabling again blocks input again"
    );
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
    "#]]
    .assert_eq(&trace.story(&["system"]));
    system.close();
    Ok(())
}

/// On a host whose OS cannot hide its screen, as Windows's, display
/// privacy stays refused while input blocking alone is applied and given
/// back, the display facility never asked.
#[test]
fn input_only_enable_skips_unsupported_display_facility() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::windows_like());
    let pair = sharing(&fake)?;
    pair.host.core.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    assert!(
        system.set_privacy(true, false).is_err(),
        "display privacy the OS cannot give is refused"
    );
    let state = system.set_privacy(false, true)?;
    assert!(
        !state.display_private && state.input_blocked,
        "input blocking alone is applied"
    );
    assert_eq!(
        fake.calls().display.calls.len(),
        0,
        "the display facility is never asked"
    );
    assert_eq!(fake.calls().input.asked(), vec![true]);
    let state = system.set_privacy(false, false)?;
    assert_eq!(state, PrivacyState::default(), "input is given back");
    assert_eq!(
        fake.calls().display.calls.len(),
        0,
        "giving input back never asks the display facility"
    );
    assert_eq!(
        fake.calls().input.asked(),
        vec![true, false],
        "input is restored exactly once"
    );
    expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=false input_blocked=true input_ok=true
          system: privacy applied connection=1 display_ok=true display_private=false input_blocked=false input_ok=true
    "#]].assert_eq(&trace.story(&["system"]));
    Ok(())
}
