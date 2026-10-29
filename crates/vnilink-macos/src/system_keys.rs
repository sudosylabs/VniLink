//! Takes this Mac's own shortcuts, such as Command-Tab, while it controls
//! another computer, through a keyboard event tap on a dedicated run loop.
//! [`vnilink::SystemKeys`] decides which keys are taken; the rest reach the
//! focused window as before.
// trace: records every ignored failure
use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
    time::Duration,
};
use vnilink::{Error, OperatingSystem, SystemKeys};

type Ref = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
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
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: isize) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRunLoopAddSource(runloop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRemoveSource(runloop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRunInMode(mode: Ref, seconds: f64, once: bool) -> i32;
    static kCFRunLoopDefaultMode: Ref;
}

const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
/// The system disabled the tap, by timeout or by user input.
const TAP_DISABLED: u32 = 0xffff_fffe;
const KEYCODE_FIELD: u32 = 9;
const SOURCE_USER_DATA_FIELD: u32 = 42;
const SOURCE_PID_FIELD: u32 = 41;

/// The device-dependent flag of one modifier key, which says whether that
/// key, not its twin on the other side, is down.
fn modifier_flag(keycode: u16) -> Option<u64> {
    Some(match keycode {
        0x3B => 0x0000_0001, // left Control
        0x38 => 0x0000_0002, // left Shift
        0x3C => 0x0000_0004, // right Shift
        0x37 => 0x0000_0008, // left Command
        0x36 => 0x0000_0010, // right Command
        0x3A => 0x0000_0020, // left Option
        0x3D => 0x0000_0040, // right Option
        0x3E => 0x0000_2000, // right Control
        0x39 => 0x0001_0000, // Caps Lock
        _ => return None,
    })
}

struct Context {
    keys: SystemKeys,
    sink: Box<dyn FnMut(u16, bool) + Send>,
    port: Ref,
}

extern "C" fn take(_proxy: Ref, kind: u32, event: Ref, data: *mut c_void) -> Ref {
    // SAFETY: `data` is the `Context` boxed by `run`, which outlives the tap:
    // the tap is invalidated before the box is dropped, on this same thread.
    let context = unsafe { &mut *data.cast::<Context>() };
    if kind >= TAP_DISABLED {
        // A slow callback or user input disabled the tap; taking resumes.
        unsafe { CGEventTapEnable(context.port, true) };
        return event;
    }
    // SAFETY: the tap only delivers live keyboard events to its callback.
    let field = |id| unsafe { CGEventGetIntegerValueField(event, id) };
    // Keys this process injected as a host are not this keyboard's.
    if field(SOURCE_USER_DATA_FIELD) == 100
        && field(SOURCE_PID_FIELD) == i64::from(std::process::id())
    {
        return event;
    }
    let Ok(keycode) = u16::try_from(field(KEYCODE_FIELD)) else {
        return event;
    };
    let Some(usage) = vnilink::usage_for_native_key(keycode) else {
        return event;
    };
    let down = match kind {
        KEY_DOWN => true,
        KEY_UP => false,
        FLAGS_CHANGED => match modifier_flag(keycode) {
            Some(flag) => (unsafe { CGEventGetFlags(event) } & flag) != 0,
            None => return event,
        },
        _ => return event,
    };
    if context.keys.take(usage, down) {
        (context.sink)(usage, down);
        std::ptr::null()
    } else {
        event
    }
}

/// Owns the tap's run loop thread. Dropping it stops taking keys; keys
/// already taken are the receiver's to release.
pub struct MacSystemKeys {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MacSystemKeys {
    /// Starts taking this Mac's shortcuts. `sink` receives each taken key,
    /// as a HID usage and whether it went down, on the tap's thread: it
    /// must return quickly, or the system disables the tap for a moment.
    /// Without Accessibility permission nothing is taken and this fails.
    pub fn start(sink: Box<dyn FnMut(u16, bool) + Send>) -> Result<Self, Error> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, started) = mpsc::sync_channel(1);
        let running = stop.clone();
        let thread = std::thread::Builder::new()
            .name("vnilink-system-keys".into())
            .spawn(move || run(sink, &running, &ready))
            .map_err(|_| Error::Unsupported("system shortcut thread"))?;
        match started.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                vnilink::ignored!(
                    vnilink::trace::INPUT,
                    "a tap that never started has nothing to stop",
                    thread.join()
                );
                Err(error)
            }
            Err(_) => Err(Error::Unsupported("system shortcut thread")),
        }
    }
}

impl Drop for MacSystemKeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            vnilink::ignored!(
                vnilink::trace::INPUT,
                "the tap's thread ended on its own",
                thread.join()
            );
        }
    }
}

fn run(
    sink: Box<dyn FnMut(u16, bool) + Send>,
    stop: &AtomicBool,
    ready: &mpsc::SyncSender<Result<(), Error>>,
) {
    let mut context = Box::new(Context {
        keys: SystemKeys::new(OperatingSystem::Macos),
        sink,
        port: std::ptr::null(),
    });
    let mask = [KEY_DOWN, KEY_UP, FLAGS_CHANGED]
        .into_iter()
        .fold(0u64, |mask, kind| mask | (1u64 << kind));
    // SAFETY: the callback and its context stay valid until the port is
    // invalidated below, on this thread.
    let port =
        unsafe { CGEventTapCreate(0, 0, 0, mask, take, (&mut *context as *mut Context).cast()) };
    if port.is_null() {
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the caller stopped waiting for the tap",
            ready.send(Err(Error::Unsupported("system shortcut permission")))
        );
        return;
    }
    context.port = port;
    // SAFETY: `port` is a live tap; its source is released below.
    let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0) };
    if source.is_null() {
        unsafe {
            CFMachPortInvalidate(port);
            CFRelease(port);
        }
        vnilink::ignored!(
            vnilink::trace::INPUT,
            "the caller stopped waiting for the tap",
            ready.send(Err(Error::Unsupported("system shortcut run loop")))
        );
        return;
    }
    // SAFETY: the run loop is this thread's; source and port are live.
    let runloop = unsafe { CFRunLoopGetCurrent() };
    unsafe {
        CFRunLoopAddSource(runloop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(port, true);
    }
    vnilink::ignored!(
        vnilink::trace::INPUT,
        "the caller stopped waiting for the tap",
        ready.send(Ok(()))
    );
    while !stop.load(Ordering::Acquire) {
        unsafe {
            CFRunLoopRunInMode(
                kCFRunLoopDefaultMode,
                Duration::from_millis(50).as_secs_f64(),
                false,
            );
        }
    }
    // SAFETY: tear down in reverse before `context` is dropped.
    unsafe {
        CGEventTapEnable(port, false);
        CFRunLoopRemoveSource(runloop, source, kCFRunLoopDefaultMode);
        CFMachPortInvalidate(port);
        CFRelease(source);
        CFRelease(port);
    }
    drop(context);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_modifier_reads_its_own_side() {
        for (keycode, usage) in [
            (0x3B, vnilink::usage::LEFT_CONTROL),
            (0x3E, vnilink::usage::RIGHT_CONTROL),
            (0x37, vnilink::usage::LEFT_META),
            (0x36, vnilink::usage::RIGHT_META),
        ] {
            assert!(modifier_flag(keycode).is_some(), "{keycode:#x} has a flag");
            assert_eq!(vnilink::usage_for_native_key(keycode), Some(usage));
        }
        let flags: Vec<_> = [0x3B, 0x38, 0x3C, 0x37, 0x36, 0x3A, 0x3D, 0x3E]
            .into_iter()
            .filter_map(modifier_flag)
            .collect();
        assert!(
            flags
                .iter()
                .enumerate()
                .all(|(i, a)| flags[i + 1..].iter().all(|b| a & b == 0)),
            "no two modifiers share a flag"
        );
    }

    /// Opt-in: needs Accessibility permission for the test binary. Without
    /// it, starting fails and says why; with it, the tap starts and stops.
    #[test]
    #[ignore = "creates a real keyboard event tap"]
    fn a_real_tap_starts_and_stops_or_says_it_has_no_permission() {
        match MacSystemKeys::start(Box::new(|_, _| {})) {
            Ok(keys) => drop(keys),
            Err(error) => assert!(
                matches!(error, Error::Unsupported("system shortcut permission")),
                "{error:?}"
            ),
        }
    }
}
