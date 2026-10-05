use super::support::*;

#[test]
fn a_live_session_reports_its_selected_direct_path() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let paths = controller.session_paths(session.id).unwrap();
    assert!(
        paths.iter().any(|path| path.selected && !path.relay),
        "expected a selected direct path, got {paths:?}"
    );
    assert!(
        paths
            .iter()
            .map(|path| path.tx_bytes + path.rx_bytes)
            .sum::<u64>()
            > 0,
        "expected handshake bytes on the paths, got {paths:?}"
    );
    let host_session = host.snapshot().unwrap().incoming.unwrap();
    let host_paths = host.session_paths(host_session.id).unwrap();
    assert!(
        host_paths.iter().any(|path| path.selected && !path.relay),
        "expected a selected direct path, got {host_paths:?}"
    );
}

#[test]
fn an_unknown_session_has_no_paths() {
    let core = start_direct();
    assert!(matches!(
        core.session_paths(999),
        Err(crate::error::Error::NoSession)
    ));
}
