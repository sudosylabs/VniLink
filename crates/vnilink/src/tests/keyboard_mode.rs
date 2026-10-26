use crate::error::Error;
use crate::keyboard::KeyboardMode;

use super::keyboard_support::*;

#[test]
fn the_keyboard_mode_is_process_local_and_character_by_default() {
    let core = start_direct();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Character);
    core.set_keyboard_mode(KeyboardMode::Map).unwrap();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Map);
    core.set_keyboard_mode(KeyboardMode::Translate).unwrap();
    assert_eq!(core.keyboard_mode().unwrap(), KeyboardMode::Translate);
    core.shutdown();
    assert!(matches!(
        core.set_keyboard_mode(KeyboardMode::Character),
        Err(Error::ShutDown)
    ));
}
