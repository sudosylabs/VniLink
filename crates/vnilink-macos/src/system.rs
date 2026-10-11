//! macOS OS actions. Workstation lock uses the shipped `CGSession`
//! suspend entry point, which requires a graphical session. Restart,
//! shutdown, privacy mode and input blocking have no supported programmatic
//! facility and stay honestly unsupported: callers see `Refused`, never a
//! fake success.

use std::process::Command;
use vnilink::system::{HostSessionState, HostSystemActions, SystemSupport};
use vnilink::Error;

/// `CGSession -suspend` returns immediately after requesting the lock; the
/// login window performs the actual transition.
const CGSESSION: &str =
    "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession";

pub struct MacSystemActions;

impl HostSystemActions for MacSystemActions {
    fn support(&self) -> SystemSupport {
        SystemSupport {
            lock: true,
            restart: false,
            shutdown: false,
            privacy: false,
            input_block: false,
        }
    }

    fn lock_workstation(&self) -> Result<(), Error> {
        let status = Command::new(CGSESSION)
            .arg("-suspend")
            .status()
            .map_err(|_| Error::Unsupported("workstation lock is not available"))?;
        if status.success() {
            Ok(())
        } else {
            Err(Error::Unsupported("workstation lock was rejected"))
        }
    }

    fn restart(&self) -> Result<(), Error> {
        Err(Error::Unsupported("restart needs a privileged helper"))
    }

    fn shutdown(&self) -> Result<(), Error> {
        Err(Error::Unsupported("shutdown needs a privileged helper"))
    }

    fn set_display_private(&self, _private: bool) -> Result<(), Error> {
        Err(Error::Unsupported("privacy mode is not available"))
    }

    fn set_input_blocked(&self, _blocked: bool) -> Result<(), Error> {
        Err(Error::Unsupported("input blocking is not available"))
    }

    fn session_state(&self) -> HostSessionState {
        HostSessionState::unknown("aqua")
    }
}
