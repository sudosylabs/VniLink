// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Owned macOS privacy restrictions. A dedicated run loop owns native pointers;
//! commands acknowledge OS outcomes, and teardown restores the exact displays.
// trace: records every ignored failure
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};
use vnilink::Error;
type Ref = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max: u32, ids: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayGammaTableCapacity(id: u32) -> u32;
    fn CGGetDisplayTransferByTable(
        id: u32,
        cap: u32,
        r: *mut f32,
        g: *mut f32,
        b: *mut f32,
        count: *mut u32,
    ) -> i32;
    fn CGSetDisplayTransferByTable(
        id: u32,
        count: u32,
        r: *const f32,
        g: *const f32,
        b: *const f32,
    ) -> i32;
    fn CGDisplayCreateUUIDFromDisplayID(id: u32) -> Ref;
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        mask: u64,
        callback: extern "C" fn(Ref, u32, Ref, *mut c_void) -> Ref,
        data: *mut c_void,
    ) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    fn CGEventGetFlags(event: Ref) -> u64;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ref);
    fn CFEqual(a: Ref, b: Ref) -> bool;
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: isize) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRunLoopAddSource(runloop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRemoveSource(runloop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRunInMode(mode: Ref, seconds: f64, once: bool) -> i32;
    static kCFRunLoopDefaultMode: Ref;
}
struct Uuid(Ref);
impl Uuid {
    fn new(id: u32) -> Result<Self, Error> {
        let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
        if uuid.is_null() {
            Err(Error::Unsupported("display identity"))
        } else {
            Ok(Self(uuid))
        }
    }
}
impl Drop for Uuid {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
struct Gamma {
    uuid: Uuid,
    r: Vec<f32>,
    g: Vec<f32>,
    b: Vec<f32>,
}
fn displays() -> Result<Vec<u32>, Error> {
    let mut ids = [0u32; 64];
    let mut count = 0;
    if unsafe { CGGetActiveDisplayList(64, ids.as_mut_ptr(), &mut count) } != 0
        || count == 0
        || count >= 64
    {
        return Err(Error::Unsupported("active displays"));
    }
    Ok(ids[..count as usize].to_vec())
}
fn snapshot(id: u32) -> Result<Gamma, Error> {
    let cap = unsafe { CGDisplayGammaTableCapacity(id) };
    if cap == 0 || cap > 4096 {
        return Err(Error::Unsupported("display gamma table"));
    }
    let mut r = vec![0f32; cap as usize];
    let mut g = r.clone();
    let mut b = r.clone();
    let mut count = 0;
    if unsafe {
        CGGetDisplayTransferByTable(
            id,
            cap,
            r.as_mut_ptr(),
            g.as_mut_ptr(),
            b.as_mut_ptr(),
            &mut count,
        )
    } != 0
        || count == 0
        || count > cap
    {
        return Err(Error::Unsupported("display gamma snapshot"));
    }
    r.truncate(count as usize);
    g.truncate(count as usize);
    b.truncate(count as usize);
    Ok(Gamma {
        uuid: Uuid::new(id)?,
        r,
        g,
        b,
    })
}
fn blackout(id: u32, count: usize) -> Result<(), Error> {
    let zeros = vec![0f32; count];
    if unsafe {
        CGSetDisplayTransferByTable(
            id,
            count as u32,
            zeros.as_ptr(),
            zeros.as_ptr(),
            zeros.as_ptr(),
        )
    } != 0
    {
        Err(Error::Unsupported("display blackout"))
    } else {
        Ok(())
    }
}
#[derive(Default)]
struct DisplayPrivacy {
    saved: Vec<Gamma>,
}
impl DisplayPrivacy {
    fn apply(&mut self) -> Result<(), Error> {
        let ids = displays()?;
        // Snapshot all newly seen display identities before modifying any of them.
        let mut fresh = Vec::new();
        for id in &ids {
            let uuid = Uuid::new(*id)?;
            if !self
                .saved
                .iter()
                .any(|g| unsafe { CFEqual(g.uuid.0, uuid.0) })
            {
                fresh.push(snapshot(*id)?);
            }
        }
        if self.saved.len() + fresh.len() > 64 {
            return Err(Error::Unsupported("privacy display history limit"));
        }
        self.saved.extend(fresh);
        for id in ids {
            let uuid = Uuid::new(id)?;
            let gamma = self
                .saved
                .iter()
                .find(|g| unsafe { CFEqual(g.uuid.0, uuid.0) })
                .ok_or(Error::Unsupported("display privacy identity"))?;
            blackout(id, gamma.r.len())?;
        }
        Ok(())
    }
    fn restore(&mut self) -> Result<(), Error> {
        if self.saved.is_empty() {
            return Ok(());
        }
        let ids = displays()?;
        let mut failed = false;
        self.saved.retain(|gamma| {
            let target = ids.iter().find(|id| {
                Uuid::new(**id).is_ok_and(|uuid| unsafe { CFEqual(gamma.uuid.0, uuid.0) })
            });
            let Some(id) = target else {
                return true;
            };
            let rejected = unsafe {
                CGSetDisplayTransferByTable(
                    *id,
                    gamma.r.len() as u32,
                    gamma.r.as_ptr(),
                    gamma.g.as_ptr(),
                    gamma.b.as_ptr(),
                )
            } != 0;
            failed |= rejected;
            rejected
        });
        if failed {
            Err(Error::Unsupported("display privacy restoration"))
        } else {
            Ok(())
        }
    }
}
impl Drop for DisplayPrivacy {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::warn!(
                target: vnilink::trace::SYSTEM,
                failure = vnilink::trace::Failure::failure_name(&error),
                reason = vnilink::trace::Failure::failure_reason(&error),
                "the display could not be restored: it may stay dark"
            );
        }
    }
}
struct TapContext {
    escape: bool,
}
extern "C" fn filter(_proxy: Ref, kind: u32, event: Ref, data: *mut c_void) -> Ref {
    let context = unsafe { &mut *data.cast::<TapContext>() };
    // Timeout, OS disable, or the local recovery chord releases restrictions.
    if kind >= 0xffff_fffe
        || kind == 10
            && unsafe { CGEventGetIntegerValueField(event, 9) } == 53
            && unsafe { CGEventGetFlags(event) } & ((1 << 18) | (1 << 19))
                == ((1 << 18) | (1 << 19))
    {
        context.escape = true;
        return event;
    }
    // Enigo's marked events from this process remain usable by the remote viewer.
    if unsafe { CGEventGetIntegerValueField(event, 42) } == 100
        && unsafe { CGEventGetIntegerValueField(event, 41) } == std::process::id() as i64
    {
        event
    } else {
        std::ptr::null()
    }
}
struct InputTap {
    port: Ref,
    source: Ref,
    runloop: Ref,
    context: Box<TapContext>,
}
impl InputTap {
    fn open() -> Result<Self, Error> {
        let mut context = Box::new(TapContext { escape: false });
        let mask = [1u32, 2, 3, 4, 5, 6, 7, 10, 11, 12, 22, 25, 26, 27]
            .into_iter()
            .fold(0u64, |mask, kind| mask | (1u64 << kind));
        let port = unsafe {
            CGEventTapCreate(
                0,
                0,
                0,
                mask,
                filter,
                (&mut *context as *mut TapContext).cast(),
            )
        };
        if port.is_null() {
            return Err(Error::Unsupported("local input blocking consent"));
        }
        let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0) };
        if source.is_null() {
            unsafe {
                CFMachPortInvalidate(port);
                CFRelease(port);
            }
            return Err(Error::Unsupported("input blocking run loop"));
        }
        let runloop = unsafe { CFRunLoopGetCurrent() };
        unsafe {
            CFRunLoopAddSource(runloop, source, kCFRunLoopDefaultMode);
            CGEventTapEnable(port, true);
        }
        Ok(Self {
            port,
            source,
            runloop,
            context,
        })
    }
}
impl Drop for InputTap {
    fn drop(&mut self) {
        unsafe {
            CGEventTapEnable(self.port, false);
            CFRunLoopRemoveSource(self.runloop, self.source, kCFRunLoopDefaultMode);
            CFMachPortInvalidate(self.port);
            CFRelease(self.source);
            CFRelease(self.port);
        }
    }
}
enum Command {
    Display(
        bool,
        std::time::Instant,
        mpsc::SyncSender<Result<(), Error>>,
    ),
    Input(
        bool,
        std::time::Instant,
        mpsc::SyncSender<Result<(), Error>>,
    ),
}
struct Worker {
    send: mpsc::SyncSender<Command>,
    thread: std::thread::JoinHandle<()>,
}
#[derive(Default)]
pub(crate) struct Privacy {
    worker: Mutex<Option<Worker>>,
    observed: Arc<AtomicU8>,
}
impl Privacy {
    pub(crate) fn observed(&self) -> (bool, bool) {
        let flags = self.observed.load(Ordering::Acquire);
        (flags & 1 != 0, flags & 2 != 0)
    }
    pub(crate) fn display(&self, on: bool) -> Result<(), Error> {
        self.command(on, |reply| {
            Command::Display(
                on,
                std::time::Instant::now() + Duration::from_secs(4),
                reply,
            )
        })
    }
    pub(crate) fn input(&self, on: bool) -> Result<(), Error> {
        self.command(on, |reply| {
            Command::Input(
                on,
                std::time::Instant::now() + Duration::from_secs(4),
                reply,
            )
        })
    }
    fn command(
        &self,
        on: bool,
        command: impl FnOnce(mpsc::SyncSender<Result<(), Error>>) -> Command,
    ) -> Result<(), Error> {
        let mut worker = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        if worker.is_none() && !on {
            return Ok(());
        }
        if worker.is_none() {
            let (send, recv) = mpsc::sync_channel(4);
            let observed = self.observed.clone();
            let thread = std::thread::Builder::new()
                .name("vnilink-mac-privacy".into())
                .spawn(move || run(recv, observed))
                .map_err(|_| Error::Unsupported("privacy worker"))?;
            *worker = Some(Worker { send, thread });
        }
        let (reply, result) = mpsc::sync_channel(1);
        worker
            .as_ref()
            .unwrap()
            .send
            .try_send(command(reply))
            .map_err(|_| Error::Busy)?;
        result
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| Error::Timeout)?
    }
}
impl Drop for Privacy {
    fn drop(&mut self) {
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            drop(worker.send);
            if worker.thread.join().is_err() {
                tracing::warn!(target: vnilink::trace::SYSTEM, "the privacy worker panicked");
            }
        }
    }
}
fn run(recv: mpsc::Receiver<Command>, observed: Arc<AtomicU8>) {
    let mut display = DisplayPrivacy::default();
    let mut private = false;
    let mut tap: Option<InputTap> = None;
    let mut refresh = std::time::Instant::now();
    loop {
        match recv.recv_timeout(Duration::from_millis(20)) {
            Ok(Command::Display(on, deadline, reply)) => {
                if std::time::Instant::now() >= deadline {
                    vnilink::ignored!(
                        vnilink::trace::SYSTEM,
                        "the caller stopped waiting for its answer",
                        reply.send(Err(Error::Timeout))
                    );
                    continue;
                }
                let result = if on {
                    display.apply()
                } else {
                    display.restore()
                };
                if result.is_ok() {
                    private = on;
                } else if on {
                    private = display.restore().is_err();
                }
                observed.store(
                    u8::from(private) | (u8::from(tap.is_some()) << 1),
                    Ordering::Release,
                );
                if std::time::Instant::now() >= deadline || reply.send(result).is_err() {
                    tap = None;
                    private = display.restore().is_err();
                    observed.store(u8::from(private), Ordering::Release);
                }
            }
            Ok(Command::Input(on, deadline, reply)) => {
                if std::time::Instant::now() >= deadline {
                    vnilink::ignored!(
                        vnilink::trace::SYSTEM,
                        "the caller stopped waiting for its answer",
                        reply.send(Err(Error::Timeout))
                    );
                    continue;
                }
                let result = if on && tap.is_none() {
                    InputTap::open().map(|opened| tap = Some(opened))
                } else {
                    if !on {
                        tap = None;
                    }
                    Ok(())
                };
                observed.store(
                    u8::from(private) | (u8::from(tap.is_some()) << 1),
                    Ordering::Release,
                );
                if std::time::Instant::now() >= deadline || reply.send(result).is_err() {
                    tap = None;
                    private = display.restore().is_err();
                    observed.store(u8::from(private), Ordering::Release);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
        }
        let escaped = tap.as_ref().is_some_and(|tap| tap.context.escape);
        let refresh_due = refresh.elapsed() >= Duration::from_millis(250);
        if refresh_due {
            refresh = std::time::Instant::now();
        }
        if escaped || private && refresh_due && display.apply().is_err() {
            tap = None;
            private = display.restore().is_err();
            observed.store(u8::from(private), Ordering::Release);
        }
        if !private && refresh_due && !display.saved.is_empty() {
            if let Err(error) = display.restore() {
                tracing::warn!(
                    target: vnilink::trace::SYSTEM,
                    failure = vnilink::trace::Failure::failure_name(&error),
                    reason = vnilink::trace::Failure::failure_reason(&error),
                    "the display could not be restored: it may stay dark"
                );
            }
        }
    }
    drop(tap);
    if let Err(error) = display.restore() {
        tracing::warn!(
            target: vnilink::trace::SYSTEM,
            failure = vnilink::trace::Failure::failure_name(&error),
            reason = vnilink::trace::Failure::failure_reason(&error),
            "the display could not be restored: it may stay dark"
        );
    }
    observed.store(0, Ordering::Release);
}
