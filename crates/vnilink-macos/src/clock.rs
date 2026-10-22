//! Mach host time, the clock ScreenCaptureKit stamps pictures with, as an [`Instant`].

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The instant a host time names, measured back from now. A host time later
/// than now is not a capture time and gives `None`.
pub fn host_time_instant(host_time: u64) -> Option<Instant> {
    // SAFETY: mach_absolute_time takes no arguments and only reads the clock.
    let now_ticks = unsafe { mach_absolute_time() };
    let now = Instant::now();
    let age = ticks_to_duration(now_ticks.checked_sub(host_time)?, timebase());
    now.checked_sub(age)
}

/// The current host time, for tests and measurement tools that compare
/// against ScreenCaptureKit's stamps.
pub fn host_time_now() -> u64 {
    // SAFETY: as in `host_time_instant`.
    unsafe { mach_absolute_time() }
}

/// Ticks to wall duration with the timebase `numer / denom` nanoseconds per tick.
pub(crate) fn ticks_to_duration(ticks: u64, (numer, denom): (u32, u32)) -> Duration {
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1));
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid, writable mach_timebase_info_data_t.
        let status = unsafe { mach_timebase_info(&raw mut info) };
        if status != 0 || info.numer == 0 || info.denom == 0 {
            (1, 1)
        } else {
            (info.numer, info.denom)
        }
    })
}

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}
