//! Blocks this PC's own keyboard and mouse while privacy mode is on.
//!
//! `BlockInput` blocks everything but Ctrl+Alt+Delete, and only for a
//! process running as administrator. Otherwise low-level keyboard and mouse
//! hooks swallow every event that was not injected, so the viewer's own
//! input still arrives. Those hooks cannot stop input meant for a window
//! that runs as administrator, which a standard user's process may not
//! intercept.
use vnilink::system::owner::BlockBackend;
use vnilink::Error;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::BlockInput,
        WindowsAndMessaging::{
            CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED,
            MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
        },
    },
};

use crate::hook_thread::HookThread;

unsafe extern "system" fn keyboard(code: i32, message: WPARAM, event: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, `event` points at the hook's KBDLLHOOKSTRUCT.
    if code == HC_ACTION as i32
        && unsafe { &*(event.0 as *const KBDLLHOOKSTRUCT) }.flags.0 & LLKHF_INJECTED.0 == 0
    {
        return LRESULT(1);
    }
    // SAFETY: passes the event on unchanged, as the hook chain expects.
    unsafe { CallNextHookEx(None, code, message, event) }
}

unsafe extern "system" fn mouse(code: i32, message: WPARAM, event: LPARAM) -> LRESULT {
    // SAFETY: for HC_ACTION, `event` points at the hook's MSLLHOOKSTRUCT.
    if code == HC_ACTION as i32
        && unsafe { &*(event.0 as *const MSLLHOOKSTRUCT) }.flags & LLMHF_INJECTED == 0
    {
        return LRESULT(1);
    }
    // SAFETY: passes the event on unchanged, as the hook chain expects.
    unsafe { CallNextHookEx(None, code, message, event) }
}

/// How this PC's own input is blocked, when it is.
#[derive(Default)]
enum Blocked {
    #[default]
    No,
    /// `BlockInput`, on the input owner's thread, which alone may lift it.
    Everything,
    /// Hooks, while their thread lives.
    Hooks { _thread: HookThread },
}

/// The backend the input owner calls, on its own thread, for every change.
#[derive(Default)]
pub(crate) struct InputBlock {
    blocked: Blocked,
}

impl BlockBackend for InputBlock {
    fn set_blocked(&mut self, blocked: bool) -> Result<(), Error> {
        match (blocked, &self.blocked) {
            (true, Blocked::No) => {
                // SAFETY: a plain Win32 call on the owner's thread.
                self.blocked = match unsafe { BlockInput(true) } {
                    Ok(()) => Blocked::Everything,
                    Err(refused) => {
                        tracing::info!(
                            target: vnilink::trace::INPUT,
                            code = refused.code().0,
                            "Windows refused to block all input to a process that is not administrator; hooks block it"
                        );
                        Blocked::Hooks {
                            _thread: HookThread::start(
                                "vnilink-input-block",
                                &[(WH_KEYBOARD_LL, Some(keyboard)), (WH_MOUSE_LL, Some(mouse))],
                                || {},
                                || {},
                            )?,
                        }
                    }
                };
            }
            (false, Blocked::Everything) => {
                // SAFETY: the owner's thread set the block, so it may lift it.
                unsafe { BlockInput(false) }
                    .map_err(|_| Error::Unsupported("input blocking could not be lifted"))?;
                self.blocked = Blocked::No;
            }
            (false, Blocked::Hooks { .. }) => self.blocked = Blocked::No,
            (true, _) | (false, Blocked::No) => {}
        }
        Ok(())
    }
}
