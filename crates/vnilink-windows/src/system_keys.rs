//! Takes this PC's own shortcuts, such as Alt-Tab and the Windows key,
//! while it controls another computer, through a low-level keyboard hook on
//! a dedicated message loop. [`vnilink::SystemKeys`] decides which keys are
//! taken; the rest reach the focused window as before.
// trace: records every ignored failure
use std::{cell::RefCell, sync::mpsc, thread::JoinHandle};
use vnilink::{Error, OperatingSystem, SystemKeys};
use windows::Win32::{
    Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG,
        PM_NOREMOVE, WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN,
    },
};

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

/// Owns the hook's message loop thread. Dropping it stops taking keys; keys
/// already taken are the receiver's to release.
pub struct WindowsSystemKeys {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl WindowsSystemKeys {
    /// Starts taking this PC's shortcuts. `sink` receives each taken key, as
    /// a HID usage and whether it went down, on the hook's thread: it must
    /// return quickly, or Windows skips the hook for that key.
    pub fn start(sink: Box<dyn FnMut(u16, bool) + Send>) -> Result<Self, Error> {
        let (ready, started) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("vnilink-system-keys".into())
            .spawn(move || run(sink, &ready))
            .map_err(|_| Error::Unsupported("system shortcut thread"))?;
        match started.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                vnilink::ignored!(
                    vnilink::trace::INPUT,
                    "a hook that never started has nothing to stop",
                    thread.join()
                );
                Err(error)
            }
            Err(_) => Err(Error::Unsupported("system shortcut thread")),
        }
    }
}

impl Drop for WindowsSystemKeys {
    fn drop(&mut self) {
        // SAFETY: the thread made its message queue before reporting its id.
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the hook's thread ended on its own",
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        );
        if let Some(thread) = self.thread.take() {
            vnilink::ignored!(
                vnilink::trace::INPUT,
                "the hook's thread ended on its own",
                thread.join()
            );
        }
    }
}

fn run(sink: Box<dyn FnMut(u16, bool) + Send>, ready: &mpsc::SyncSender<Result<u32, Error>>) {
    CONTEXT.set(Some(Context {
        keys: SystemKeys::new(OperatingSystem::Windows),
        sink,
    }));
    let mut message = MSG::default();
    // SAFETY: plain Win32 calls on this thread; the hook is removed before it ends.
    let hook = unsafe {
        // Peeking makes this thread's message queue, so a WM_QUIT posted from
        // now on arrives. Whether a message already waits is not a failure.
        let _waiting = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        GetModuleHandleW(None).and_then(|module| {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(take), Some(HINSTANCE(module.0)), 0)
        })
    };
    let Ok(hook) = hook else {
        CONTEXT.set(None);
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the caller stopped waiting for the hook",
            ready.send(Err(Error::Unsupported("system shortcut hook")))
        );
        return;
    };
    vnilink::ignored!(
        vnilink::trace::INPUT,
        "the caller stopped waiting for the hook",
        ready.send(Ok(unsafe { GetCurrentThreadId() }))
    );
    // SAFETY: the loop ends on WM_QUIT or an error; the hook runs inside it.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
    vnilink::ignored!(
        vnilink::trace::INPUT,
        "a hook Windows already removed needs no removing",
        unsafe { UnhookWindowsHookEx(hook) }
    );
    CONTEXT.set(None);
}
