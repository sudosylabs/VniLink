//! Native macOS actions and owned privacy restrictions. OS consent may reject a request.
use std::process::Command;
use vnilink::system::{HostSessionState, HostSystemActions, SystemSupport};
use vnilink::Error;

/// Locks the screen as Control-Command-Q does. macOS removed the
/// `CGSession -suspend` tool; the login framework's entry point is what the
/// system's own lock uses. It is private, so it is looked up at run time and
/// Lock is offered only where it exists.
const LOGIN_FRAMEWORK: &std::ffi::CStr =
    c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login";
const LOCK_SYMBOL: &std::ffi::CStr = c"SACLockScreenImmediate";

extern "C" {
    fn dlopen(path: *const std::ffi::c_char, mode: std::ffi::c_int) -> *mut std::ffi::c_void;
    fn dlsym(
        handle: *mut std::ffi::c_void,
        symbol: *const std::ffi::c_char,
    ) -> *mut std::ffi::c_void;
}
const RTLD_LAZY: std::ffi::c_int = 1;

type LockScreen = unsafe extern "C" fn() -> std::ffi::c_int;

/// The system's lock entry point, if this macOS has it.
fn lock_screen() -> Option<LockScreen> {
    static LOCK: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let address = LOCK.get_or_init(|| unsafe {
        let handle = dlopen(LOGIN_FRAMEWORK.as_ptr(), RTLD_LAZY);
        if handle.is_null() {
            return None;
        }
        let symbol = dlsym(handle, LOCK_SYMBOL.as_ptr());
        (!symbol.is_null()).then_some(symbol as usize)
    });
    let address = (*address)?;
    // SAFETY: the symbol is `int SACLockScreenImmediate(void)` in the system's login framework.
    Some(unsafe { std::mem::transmute::<usize, LockScreen>(address) })
}

#[derive(Default)]
pub struct MacSystemActions {
    privacy: crate::privacy::Privacy,
}

impl HostSystemActions for MacSystemActions {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: lock_screen().is_some(),
            restart: true,
            shutdown: true,
            privacy: true,
            input_block: true,
        }
    }

    fn lock_workstation(&self) -> Result<(), Error> {
        let lock = lock_screen().ok_or(Error::Unsupported("workstation lock is not available"))?;
        // The login window performs the transition; the call returns at once.
        unsafe { lock() };
        Ok(())
    }

    fn restart(&self) -> Result<(), Error> {
        request_session_action("restart")
    }

    fn shutdown(&self) -> Result<(), Error> {
        request_session_action("shut down")
    }

    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.privacy.display(private)
    }

    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        self.privacy.input(blocked)
    }

    fn observed_privacy(&self) -> Option<(bool, bool)> {
        Some(self.privacy.observed())
    }
    fn session_state(&self) -> HostSessionState {
        HostSessionState::unknown("aqua")
    }
}

// Fixed script and executable only. An OS acceptance is a request, not proof
// that the machine completed the reboot. No command text comes from a peer.
fn request_session_action(action: &str) -> Result<(), Error> {
    let script = match action {
        "restart" => "tell application \"System Events\" to restart",
        "shut down" => "tell application \"System Events\" to shut down",
        _ => return Err(Error::Malformed),
    };
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| Error::Unsupported("macOS session action"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::Unsupported("macOS rejected the session action"))
                }
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Unsupported("macOS session action"));
            }
            Ok(None) => {}
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
