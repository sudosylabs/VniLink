//! Handle-scoped Power Requests, independent from the worker thread's lifetime.
// trace: records every ignored failure
use vnilink::power::{HostPowerFactory, PowerInhibitor, PowerKind};
use vnilink::{BackendStartup, Error};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
    PowerSetRequest, POWER_REQUEST_TYPE,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
struct Inhibitor {
    handle: isize,
    kind: POWER_REQUEST_TYPE,
    active: bool,
}
impl PowerInhibitor for Inhibitor {}
impl Drop for Inhibitor {
    fn drop(&mut self) {
        let handle = HANDLE(self.handle as *mut std::ffi::c_void);
        unsafe {
            if self.active {
                ignored_hr!(
                    vnilink::trace::POWER,
                    "a request that cannot be cleared ends with its handle",
                    PowerClearRequest(handle, self.kind)
                );
            }
            ignored_hr!(
                vnilink::trace::POWER,
                "a handle that cannot close ends with the process",
                CloseHandle(handle)
            );
        }
    }
}
#[derive(Default)]
pub struct WindowsPowerFactory;
impl HostPowerFactory for WindowsPowerFactory {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        let mut reason: Vec<u16> = "VniLink remote desktop session"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let context = REASON_CONTEXT {
            Version: 0,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(reason.as_mut_ptr()),
            },
        };
        let handle = unsafe { PowerCreateRequest(&context) }
            .map_err(|_| Error::Unsupported("Windows rejected the power request"))?;
        let mut owned = Inhibitor {
            handle: handle.0 as isize,
            kind: match kind {
                PowerKind::Display => PowerRequestDisplayRequired,
                PowerKind::System => PowerRequestSystemRequired,
            },
            active: false,
        };
        startup.check()?;
        unsafe { PowerSetRequest(handle, owned.kind) }
            .map_err(|_| Error::Unsupported("Windows rejected power inhibition"))?;
        owned.active = true;
        startup.check()?;
        Ok(Box::new(owned))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_power_startup_does_not_allocate_a_request() {
        let startup = BackendStartup::new(std::time::Duration::ZERO);
        assert!(WindowsPowerFactory
            .acquire(PowerKind::Display, &startup)
            .is_err());
        assert!(WindowsPowerFactory
            .acquire(PowerKind::System, &startup)
            .is_err());
    }
}
