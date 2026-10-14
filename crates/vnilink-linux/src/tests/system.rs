use crate::LinuxSystemActions;
use vnilink::system::HostSystemActions;

#[test]
fn system_support_reports_actions_and_never_mutates_physical_privacy() {
    // Never invokes restart/shutdown/lock here: those act on the machine.
    let actions = LinuxSystemActions::default();
    let support = actions.support();
    assert!(support.lock && support.restart && support.shutdown);
    assert!(!support.input_block);
    // No physical privacy mutation in automated tests.
    actions.set_display_private(false).unwrap();
    assert!(actions.set_input_blocked(true).is_err());
    let session = actions.session_state();
    assert!(["wayland", "x11", "linux"].contains(&session.session.as_str()));
    assert_eq!(session.locked, None);
}
