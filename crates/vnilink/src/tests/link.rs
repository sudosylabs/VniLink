use super::support::*;

#[test]
fn an_unknown_session_has_no_paths() {
    let core = start_direct();
    assert!(matches!(
        core.session_paths(999),
        Err(crate::error::Error::NoSession)
    ));
}
