//! A thread that owns low-level input hooks and pumps the messages they
//! need: Windows calls a low-level hook on the thread that installed it,
//! from that thread's message loop, and skips a hook whose thread does not
//! answer in time.
// trace: records every ignored failure
use std::{sync::mpsc, thread::JoinHandle};
use vnilink::Error;
use windows::Win32::{
    Foundation::{HINSTANCE, LPARAM, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        HHOOK, HOOKPROC, MSG, PM_NOREMOVE, WINDOWS_HOOK_ID, WM_QUIT,
    },
};

/// Owns the thread; dropping it removes the hooks and ends the thread.
pub(crate) struct HookThread {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HookThread {
    /// Starts a thread named `name` that runs `prepare`, installs `hooks`
    /// and pumps messages until dropped, then removes the hooks and runs
    /// `finish`. `prepare` and `finish` set and clear what the hook
    /// procedures read on that thread. Fails, having run `finish`, when a
    /// hook cannot be installed.
    pub(crate) fn start(
        name: &str,
        hooks: &'static [(WINDOWS_HOOK_ID, HOOKPROC)],
        prepare: impl FnOnce() + Send + 'static,
        finish: impl FnOnce() + Send + 'static,
    ) -> Result<Self, Error> {
        let (ready, started) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || run(hooks, prepare, finish, &ready))
            .map_err(|_| Error::Unsupported("input hook thread"))?;
        match started.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                vnilink::ignored!(
                    vnilink::trace::INPUT,
                    "a hook thread that never started has nothing to stop",
                    thread.join()
                );
                Err(error)
            }
            Err(_) => Err(Error::Unsupported("input hook thread")),
        }
    }
}

impl Drop for HookThread {
    fn drop(&mut self) {
        // SAFETY: the thread made its message queue before reporting its id.
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the hook thread ended on its own",
            unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
        );
        if let Some(thread) = self.thread.take() {
            vnilink::ignored!(
                vnilink::trace::INPUT,
                "the hook thread ended on its own",
                thread.join()
            );
        }
    }
}

fn unhook(installed: Vec<HHOOK>) {
    for hook in installed {
        // SAFETY: each handle came from SetWindowsHookExW on this thread.
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "a hook Windows already removed needs no removing",
            unsafe { UnhookWindowsHookEx(hook) }
        );
    }
}

fn run(
    hooks: &'static [(WINDOWS_HOOK_ID, HOOKPROC)],
    prepare: impl FnOnce(),
    finish: impl FnOnce(),
    ready: &mpsc::SyncSender<Result<u32, Error>>,
) {
    prepare();
    let mut message = MSG::default();
    // SAFETY: plain Win32 calls on this thread; every hook is removed before it ends.
    let module = unsafe {
        // Peeking makes this thread's message queue, so a WM_QUIT posted from
        // now on arrives. Whether a message already waits is not a failure.
        let _waiting = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        GetModuleHandleW(None)
    };
    let mut installed = Vec::with_capacity(hooks.len());
    let outcome = module
        .map_err(|_| Error::Unsupported("input hook module"))
        .and_then(|module| {
            for (kind, procedure) in hooks {
                // SAFETY: `procedure` is a static hook procedure of `kind`.
                let hook =
                    unsafe { SetWindowsHookExW(*kind, *procedure, Some(HINSTANCE(module.0)), 0) }
                        .map_err(|_| Error::Unsupported("input hook"))?;
                installed.push(hook);
            }
            Ok(())
        });
    if let Err(error) = outcome {
        unhook(installed);
        finish();
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the caller stopped waiting for the hook thread",
            ready.send(Err(error))
        );
        return;
    }
    vnilink::ignored!(
        vnilink::trace::INPUT,
        "the caller stopped waiting for the hook thread",
        ready.send(Ok(unsafe { GetCurrentThreadId() }))
    );
    // SAFETY: the loop ends on WM_QUIT or an error; the hooks run inside it.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
    unhook(installed);
    finish();
}
