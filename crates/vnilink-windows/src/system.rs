//! Windows OS actions. `LockWorkStation` needs an interactive session;
//! restart and shutdown enable the shutdown privilege on this process token
//! first. `BlockInput` needs elevation and never blocks Ctrl+Alt+Delete, so
//! it is reported as available but can still fail per call. There is no
//! display privacy uses an owned window excluded from capture on supported Windows.
//!
//! Blocking binds to its thread: only the blocking thread may unblock or
//! inject while blocked. Every `set_input_blocked` call runs on the shared
//! input owner, and blocked-time injection joins it there, so no caller —
//! Tokio worker, application thread or teardown path — can split the pair.

use vnilink::system::owner::{make_os_inject_backend, set_blocked, BlockBackend};
use vnilink::system::{HostSessionState, HostSystemActions, SystemSupport};
use vnilink::Error;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Shutdown::{
    ExitWindowsEx, LockWorkStation, EWX_FORCEIFHUNG, EWX_REBOOT, EWX_SHUTDOWN, EXIT_WINDOWS_FLAGS,
    SHTDN_REASON_FLAG_PLANNED,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Input::KeyboardAndMouse::BlockInput;

#[derive(Default)]
pub struct WindowsSystemActions {
    privacy: crate::privacy::Privacy,
}

/// Native blocking backend. Every call runs on the shared input owner.
struct BlockInputBackend;

fn make_block_backend() -> BlockInputBackend {
    BlockInputBackend
}

impl BlockBackend for BlockInputBackend {
    fn set_blocked(&mut self, blocked: bool) -> Result<(), Error> {
        unsafe { BlockInput(blocked).map_err(|_| Error::Unsupported("input blocking was refused")) }
    }
}

impl WindowsSystemActions {
    fn enable_shutdown_privilege() -> Result<(), Error> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
            .map_err(|_| Error::Unsupported("shutdown privilege is not available"))?;
            let result = (|| {
                let mut luid = Default::default();
                LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid).ok()?;
                let privileges = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None).ok()
            })();
            let _ = CloseHandle(token);
            result.ok_or(Error::Unsupported("shutdown privilege was refused"))
        }
    }

    fn exit(&self, flags: EXIT_WINDOWS_FLAGS) -> Result<(), Error> {
        Self::enable_shutdown_privilege()?;
        unsafe {
            ExitWindowsEx(flags | EWX_FORCEIFHUNG, SHTDN_REASON_FLAG_PLANNED)
                .map_err(|_| Error::Unsupported("Windows refused the request"))
        }
    }
}

impl HostSystemActions for WindowsSystemActions {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: true,
            shutdown: true,
            privacy: crate::privacy::supported(),
            input_block: true,
        }
    }

    fn lock_workstation(&self) -> Result<(), Error> {
        unsafe {
            LockWorkStation().map_err(|_| Error::Unsupported("workstation lock was rejected"))
        }
    }

    fn restart(&self) -> Result<(), Error> {
        self.exit(EWX_REBOOT)
    }

    fn shutdown(&self) -> Result<(), Error> {
        self.exit(EWX_SHUTDOWN)
    }

    fn set_display_private(&self, private: bool) -> Result<(), Error> {
        self.privacy.set(private)
    }

    fn set_input_blocked(&self, blocked: bool) -> Result<(), Error> {
        // Runs on the shared owner thread with an acknowledged answer; a
        // failed restore retains ownership for the same thread to retry.
        set_blocked(make_block_backend, make_os_inject_backend, blocked)
    }

    fn observed_privacy(&self) -> Option<(bool, bool)> {
        Some((
            self.privacy.observed(),
            vnilink::system::owner::is_blocked(),
        ))
    }

    fn session_state(&self) -> HostSessionState {
        HostSessionState {
            locked: None,
            login_screen: None,
            headless: false,
            session: "win32".into(),
        }
    }
}
