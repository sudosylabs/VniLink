use crate::MacSystemActions;
use vnilink::system::HostSystemActions;

#[test]
fn system_support_reports_lock_only_with_unknown_session_state() {
    // Never invokes lock_workstation here: that would lock the test machine.
    let actions = MacSystemActions;
    let support = actions.support();
    assert!(support.lock);
    assert!(!support.restart && !support.shutdown);
    assert!(!support.privacy && !support.input_block);
    let session = actions.session_state();
    assert_eq!(session.session, "aqua");
    assert_eq!(session.locked, None);
    assert_eq!(session.login_screen, None);
    assert!(!session.headless);
    assert!(actions.restart().is_err());
    assert!(actions.shutdown().is_err());
    assert!(actions.set_display_private(true).is_err());
    assert!(actions.set_input_blocked(true).is_err());
}
