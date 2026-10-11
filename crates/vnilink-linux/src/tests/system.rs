use crate::LinuxSystemActions;
use vnilink::system::HostSystemActions;

#[test]
fn system_support_reports_actions_without_privacy_or_input_block() {
    // Never invokes restart/shutdown/lock here: those act on the machine.
    let actions = LinuxSystemActions;
    let support = actions.support();
    assert!(support.lock && support.restart && support.shutdown);
    assert!(!support.privacy && !support.input_block);
    assert!(actions.set_display_private(true).is_err());
    assert!(actions.set_input_blocked(true).is_err());
    let session = actions.session_state();
    assert!(["wayland", "x11", "linux"].contains(&session.session.as_str()));
    assert_eq!(session.locked, None);
}
