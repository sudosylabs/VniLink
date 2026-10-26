use crate::error::Error;

use super::keyboard_support::*;

#[test]
fn the_command_swap_is_process_local_and_off_by_default() {
    let core = start_direct();
    assert!(!core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(true).unwrap();
    assert!(core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(false).unwrap();
    assert!(!core.swap_control_and_command().unwrap());
    core.shutdown();
    assert!(matches!(
        core.set_swap_control_and_command(true),
        Err(Error::ShutDown)
    ));
}
