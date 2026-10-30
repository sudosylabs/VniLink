// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Owned IOPM assertions; never change the machine's saved energy preferences.
use std::ffi::{c_char, c_void};
use vnilink::power::{HostPowerFactory, PowerInhibitor, PowerKind};
use vnilink::{BackendStartup, Error};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFRelease(value: *const c_void);
}
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: *const c_void,
        level: u32,
        reason: *const c_void,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}
struct Text(*const c_void);
impl Text {
    fn new(value: &std::ffi::CStr) -> Result<Self, Error> {
        // Static UTF-8 strings only; CF returns a retained object.
        let ptr =
            unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x0800_0100) };
        if ptr.is_null() {
            return Err(Error::Unsupported("power assertion string allocation"));
        }
        Ok(Self(ptr))
    }
}
impl Drop for Text {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
struct Inhibitor(u32);
impl PowerInhibitor for Inhibitor {}
impl Drop for Inhibitor {
    fn drop(&mut self) {
        unsafe {
            IOPMAssertionRelease(self.0);
        }
    }
}
#[derive(Default)]
pub struct MacPowerFactory;
impl HostPowerFactory for MacPowerFactory {
    fn acquire(
        &self,
        kind: PowerKind,
        startup: &BackendStartup,
    ) -> Result<Box<dyn PowerInhibitor>, Error> {
        startup.check()?;
        let kind = Text::new(match kind {
            PowerKind::Display => c"PreventUserIdleDisplaySleep",
            PowerKind::System => c"PreventUserIdleSystemSleep",
        })?;
        let reason = Text::new(c"VniLink remote desktop session")?;
        startup.check()?;
        let mut id = 0;
        if unsafe { IOPMAssertionCreateWithName(kind.0, 255, reason.0, &mut id) } != 0 {
            return Err(Error::Unsupported("macOS rejected the power assertion"));
        }
        let owned = Inhibitor(id);
        startup.check()?;
        Ok(Box::new(owned))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_power_startup_does_not_request_an_assertion() {
        let startup = BackendStartup::new(std::time::Duration::ZERO);
        assert!(MacPowerFactory
            .acquire(PowerKind::Display, &startup)
            .is_err());
        assert!(MacPowerFactory
            .acquire(PowerKind::System, &startup)
            .is_err());
    }
}
