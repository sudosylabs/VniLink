//! Giving privacy back when the host's OS does not do as asked: a backend
//! removed or replaced while privacy is held, restores that fail or stick,
//! and applies refused halfway. Privacy held is always what the OS
//! acknowledged, restored once, on the backend that applied it.

use expect_test::expect;
use std::sync::Arc;

use vnilink::{HostSystemActions, PrivacyState};
use vnilink_testkit::system::FakeSystem;
use vnilink_testkit::{traced, wait_for, TestResult, DEFAULT_WAIT};

use super::{sharing, PRIVACY};

/// Removing the host's backend while privacy is held gives privacy back
/// on that backend, exactly once per facility.
#[test]
fn backend_removal_restores_original_privacy_exactly_once() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    host.set_system_actions(None)?;
    wait_for("privacy to be given back", DEFAULT_WAIT, || {
        (host.system_privacy().ok()? == PrivacyState::default()).then_some(())
    });
    let calls = fake.calls();
    assert!(
        !calls.display.restricted && !calls.input.restricted,
        "the removed backend still restricts: display={}, input={}",
        calls.display.restricted,
        calls.input.restricted
    );
    assert_eq!(
        calls.display.done(),
        2,
        "the display is restricted once and restored once"
    );
    assert_eq!(
        calls.input.done(),
        2,
        "input is blocked once and restored once"
    );
    drop(calls);
    trace.wait_for_records("privacy restored", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy restored connection=1 display=true input=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// Replacing the host's backend while privacy is held gives privacy back
/// on the backend that applied it, exactly once, and never asks the new
/// one.
#[test]
fn backend_replacement_restores_original_and_leaves_new_untouched() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    let replacement = Arc::new(FakeSystem::full());
    host.set_system_actions(Some(Arc::clone(&replacement) as Arc<dyn HostSystemActions>))?;
    wait_for("privacy to be given back", DEFAULT_WAIT, || {
        (host.system_privacy().ok()? == PrivacyState::default()).then_some(())
    });
    let calls = fake.calls();
    assert!(
        !calls.display.restricted && !calls.input.restricted,
        "the original backend still restricts: display={}, input={}",
        calls.display.restricted,
        calls.input.restricted
    );
    assert_eq!(calls.display.done(), 2, "original display restore count");
    assert_eq!(calls.input.done(), 2, "original input restore count");
    drop(calls);
    let fresh = replacement.calls();
    assert_eq!(
        fresh.display.done(),
        0,
        "the replacement's display is touched"
    );
    assert_eq!(fresh.input.done(), 0, "the replacement's input is touched");
    drop(fresh);
    trace.wait_for_records("privacy restored", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          system: privacy restored connection=1 display=true input=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// A restore the host's OS refuses keeps privacy held, and the host keeps
/// reporting it rather than claiming it was given back; once the OS
/// allows it, narrowing the policy restores it.
#[test]
fn failed_native_restore_keeps_lease_and_reports_held() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::full());
    {
        let mut calls = fake.calls();
        calls.display.refuse_restore = true;
        calls.input.refuse_restore = true;
    }
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    host.set_system_actions(None)?;
    // The teardown restore fails: wait for the attempt, then the host must
    // not report privacy given back.
    wait_for("the host to try to restore", DEFAULT_WAIT, || {
        let calls = fake.calls();
        (calls.display.restores_asked() + calls.input.restores_asked() >= 1).then_some(())
    });
    pair.host.wait_released(None);
    let held = host.system_privacy()?;
    assert!(
        held.display_private && held.input_blocked,
        "the host claims privacy given back after its OS refused: {held:?}"
    );
    assert!(
        held.holder_connection.is_some(),
        "privacy not given back still has its holder"
    );
    {
        let calls = fake.calls();
        assert!(calls.display.restricted, "the screen stays hidden");
        assert!(calls.input.restricted, "input stays blocked");
    }
    {
        let mut calls = fake.calls();
        calls.display.refuse_restore = false;
        calls.input.refuse_restore = false;
    }
    host.set_system_policy(vnilink::SystemPolicy {
        allow_privacy: false,
        ..PRIVACY
    })?;
    wait_for("privacy to be given back", DEFAULT_WAIT, || {
        (host.system_privacy().ok()? == PrivacyState::default()).then_some(())
    });
    let calls = fake.calls();
    assert!(!calls.display.restricted && !calls.input.restricted);
    assert_eq!(
        (calls.display.done(), calls.input.done()),
        (2, 2),
        "each facility is restricted once and restored once"
    );
    drop(calls);
    trace.wait_for_records("privacy restored", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          warn system: privacy not restored: it stays held until a restore works connection=1 display_held=true failure=unsupported input_held=true
          system: privacy restored connection=1 display=true input=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// When the OS refuses to block input, privacy applies nothing: the
/// screen hidden on the way is shown again, and the host reports nothing
/// held. The next request starts clean on the same backend.
#[test]
fn refused_enable_applies_nothing_and_reports_default() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::unobserved(|calls| {
        calls.input.refuse_apply = true;
        calls.input.refuse_restore = true;
    }));
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    assert!(
        system.set_privacy(true, true).is_err(),
        "privacy the OS refuses in part fails"
    );
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "the host reports no privacy it could not establish"
    );
    assert!(
        !fake.calls().display.restricted,
        "the screen hidden on the way is shown again"
    );
    {
        let mut calls = fake.calls();
        calls.input.refuse_apply = false;
        calls.input.refuse_restore = false;
    }
    let state = system.set_privacy(true, true)?;
    assert!(
        state.display_private && state.input_blocked,
        "the next request starts clean"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=false
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// When the OS refuses to block input and then to show the screen again,
/// privacy keeps exactly what the OS acknowledged, the hidden screen, never
/// what was asked; once the OS allows it, releasing gives it back.
#[test]
fn stuck_fresh_apply_retains_acknowledged_remainder() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::unobserved(|calls| {
        calls.display.refuse_restore = true;
        calls.input.refuse_apply = true;
    }));
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    assert!(
        system.set_privacy(true, true).is_err(),
        "privacy the OS refuses in part fails"
    );
    let held = host.system_privacy()?;
    assert!(
        held.display_private && !held.input_blocked,
        "privacy held is the display alone: {held:?}"
    );
    assert!(
        held.holder_connection.is_some(),
        "the display alone has its holder"
    );
    assert!(fake.calls().display.restricted, "the screen stays hidden");
    fake.calls().display.refuse_restore = false;
    let state = system.set_privacy(false, false)?;
    assert_eq!(state, PrivacyState::default(), "releasing gives it back");
    assert!(
        !fake.calls().display.restricted,
        "the screen is shown again"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=false
          system: privacy applied connection=1 display_ok=true display_private=false input_blocked=false input_ok=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// On a host whose OS cannot hide its screen, closing the service gives
/// back the input it blocked and never asks the display facility.
#[test]
fn input_only_owner_close_restores_without_touching_display() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::windows_like());
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(false, true)?;
    system.close();
    wait_for("input to be given back", DEFAULT_WAIT, || {
        (fake.calls().input.calls.len() > 1).then_some(())
    });
    assert_eq!(
        fake.calls().display.calls.len(),
        0,
        "the display facility is never asked"
    );
    assert_eq!(
        fake.calls().input.asked(),
        vec![true, false],
        "input is blocked once and given back once"
    );
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "the host reports privacy given back"
    );
    trace.wait_for_records("privacy restored", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=false input_blocked=true input_ok=true
          system: privacy restored connection=1 display=false input=true
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}

/// A narrowing whose display restore sticks gives input back and keeps the
/// display alone held; narrowing again retries the display without asking
/// input again, and once the OS allows it, gives everything back.
#[test]
fn partial_restore_keeps_remainder_and_repeated_narrowing_retries() -> TestResult {
    let trace = traced();
    let fake = Arc::new(FakeSystem::unobserved(|calls| {
        calls.display.refuse_restore = true;
    }));
    let pair = sharing(&fake)?;
    let host = &pair.host.core;
    host.set_system_policy(PRIVACY)?;
    let connection = pair.connect_services();
    let system = pair.controller.open_system(&connection)?;
    system.set_privacy(true, true)?;
    let narrowed = vnilink::SystemPolicy {
        allow_privacy: false,
        ..PRIVACY
    };
    assert!(
        host.set_system_policy(narrowed).is_err(),
        "a narrowing that could not restore everything fails"
    );
    let held = host.system_privacy()?;
    assert!(
        held.display_private && !held.input_blocked,
        "privacy held is the display alone: {held:?}"
    );
    assert!(!fake.calls().input.restricted, "input is given back");
    assert_eq!(
        fake.calls().input.calls.len(),
        2,
        "input is blocked once and given back once"
    );
    assert!(
        host.set_system_policy(narrowed).is_err(),
        "narrowing again while the display sticks fails"
    );
    assert_eq!(
        fake.calls().display.calls.len(),
        3,
        "narrowing again retries the display"
    );
    assert_eq!(
        fake.calls().input.calls.len(),
        2,
        "narrowing again leaves input alone"
    );
    fake.calls().display.refuse_restore = false;
    host.set_system_policy(narrowed)?;
    assert_eq!(
        host.system_privacy()?,
        PrivacyState::default(),
        "once the OS allows it, privacy is given back"
    );
    assert!(
        !fake.calls().display.restricted,
        "the screen is shown again"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          system: privacy applied connection=1 display_ok=true display_private=true input_blocked=true input_ok=true
          warn system: privacy not restored: it stays held until a restore works connection=1 display_held=true failure=unsupported input_held=false
          warn system: privacy not restored: it stays held until a restore works connection=1 display_held=true failure=unsupported input_held=false
          system: privacy restored connection=1 display=true input=false
    "#]],
        || trace.story(&["system"]),
    );
    Ok(())
}
