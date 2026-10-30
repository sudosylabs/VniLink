//! Takes this PC's own shortcuts, such as Alt-Tab and the Windows key,
//! while it controls another computer, through a low-level keyboard hook on
//! a dedicated message loop. [`vnilink::SystemKeys`] decides which keys are
//! taken; the rest reach the focused window as before.
use std::cell::RefCell;
use vnilink::{Error, OperatingSystem, SystemKeys};
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, WH_KEYBOARD_LL,
        WM_KEYDOWN, WM_SYSKEYDOWN,
    },
};

use crate::hook_thread::HookThread;

struct Context {
    keys: SystemKeys,
    sink: Box<dyn FnMut(u16, bool) + Send>,
}

thread_local! {
    /// The hook's state, on the thread whose message loop runs the hook.
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

unsafe extern "system" fn take(code: i32, message: WPARAM, event: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `event` points at the hook's KBDLLHOOKSTRUCT.
        let key = unsafe { &*(event.0 as *const KBDLLHOOKSTRUCT) };
        // Keys injected by any program, this one as a host included, are not this keyboard's.
        if key.flags.0 & LLKHF_INJECTED.0 == 0 {
            let extended = if key.flags.0 & LLKHF_EXTENDED.0 != 0 {
                0xE000
            } else {
                0
            };
            let usage = u16::try_from(key.scanCode)
                .ok()
                .and_then(|scan| vnilink::usage_for_native_key(scan | extended));
            let down = matches!(message.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let taken = usage.is_some_and(|usage| {
                CONTEXT.with_borrow_mut(|context| {
                    context.as_mut().is_some_and(|context| {
                        let taken = context.keys.take(usage, down);
                        if taken {
                            (context.sink)(usage, down);
                        }
                        taken
                    })
                })
            });
            if taken {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: passes the event on unchanged, as the hook chain expects.
    unsafe { CallNextHookEx(None, code, message, event) }
}

/// Takes this PC's shortcuts until dropped; keys already taken are the
/// receiver's to release.
pub struct WindowsSystemKeys {
    _thread: HookThread,
}

impl WindowsSystemKeys {
    /// Starts taking this PC's shortcuts. `sink` receives each taken key, as
    /// a HID usage and whether it went down, on the hook's thread: it must
    /// return quickly, or Windows skips the hook for that key.
    pub fn start(sink: Box<dyn FnMut(u16, bool) + Send>) -> Result<Self, Error> {
        HookThread::start(
            "vnilink-system-keys",
            &[(WH_KEYBOARD_LL, Some(take))],
            move || {
                CONTEXT.set(Some(Context {
                    keys: SystemKeys::new(OperatingSystem::Windows),
                    sink,
                }));
            },
            || CONTEXT.set(None),
        )
        .map(|thread| Self { _thread: thread })
    }
}
