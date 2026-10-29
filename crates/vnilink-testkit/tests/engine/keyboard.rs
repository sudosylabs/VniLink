//! The keyboard settings this computer keeps for its own windows: how keys
//! are sent and whether Control and Command swap. Neither is a record, and
//! neither outlives the engine.

use vnilink::{Error, KeyboardMode};
use vnilink_testkit::{traced, Engine, TestResult};

#[test]
fn the_keyboard_mode_is_process_local_and_character_by_default() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    assert_eq!(
        engine.core.keyboard_mode()?,
        KeyboardMode::Character,
        "a new engine sends characters"
    );
    engine.core.set_keyboard_mode(KeyboardMode::Map)?;
    assert_eq!(
        engine.core.keyboard_mode()?,
        KeyboardMode::Map,
        "the mode set is the mode read"
    );
    engine.core.set_keyboard_mode(KeyboardMode::Translate)?;
    assert_eq!(
        engine.core.keyboard_mode()?,
        KeyboardMode::Translate,
        "the mode set is the mode read"
    );
    engine.core.shutdown();
    assert!(
        matches!(
            engine.core.set_keyboard_mode(KeyboardMode::Character),
            Err(Error::ShutDown)
        ),
        "a shut down engine takes no keyboard mode"
    );
    Ok(())
}

#[test]
fn the_command_swap_is_process_local_and_off_by_default() -> TestResult {
    let _trace = traced();
    let engine = Engine::start("engine");
    assert!(
        !engine.core.swap_control_and_command()?,
        "a new engine does not swap Control and Command"
    );
    engine.core.set_swap_control_and_command(true)?;
    assert!(
        engine.core.swap_control_and_command()?,
        "the swap set is the swap read"
    );
    engine.core.set_swap_control_and_command(false)?;
    assert!(
        !engine.core.swap_control_and_command()?,
        "the swap cleared is the swap read"
    );
    engine.core.shutdown();
    assert!(
        matches!(
            engine.core.set_swap_control_and_command(true),
            Err(Error::ShutDown)
        ),
        "a shut down engine takes no swap"
    );
    Ok(())
}
