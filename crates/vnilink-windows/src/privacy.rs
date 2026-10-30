// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A local black window excluded from Desktop Duplication. The window and its
//! message pump live on one owner thread and are destroyed on revocation/drop.
// trace: records every ignored failure
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use vnilink::Error;
use windows::{
    core::w,
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::*,
        },
    },
};
#[repr(C)]
struct Version {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service: [u16; 128],
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut Version) -> i32;
}
pub(crate) fn supported() -> bool {
    let mut v = Version {
        size: std::mem::size_of::<Version>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        service: [0; 128],
    };
    unsafe { RtlGetVersion(&mut v) == 0 && (v.major > 10 || v.major == 10 && v.build >= 19041) }
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, w, l) }
}
struct Overlay(HWND);
impl Overlay {
    fn open() -> Result<Self, Error> {
        if !supported() {
            return Err(Error::Unsupported(
                "Windows capture exclusion requires build 19041",
            ));
        }
        unsafe {
            // This dedicated GUI owner uses physical coordinates on mixed-DPI desktops.
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = HINSTANCE(
                GetModuleHandleW(None)
                    .map_err(|_| Error::Unsupported("privacy window module"))?
                    .0,
            );
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("VniLinkPrivacyOverlay"),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            // Class registration is process-wide; another owner may have registered it already.
            RegisterClassW(&class);
            let overlay = Self(
                CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("VniLinkPrivacyOverlay"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    Some(instance),
                    None,
                )
                .map_err(|_| Error::Unsupported("privacy window"))?,
            );
            SetWindowDisplayAffinity(overlay.0, WDA_EXCLUDEFROMCAPTURE)
                .map_err(|_| Error::Unsupported("capture exclusion"))?;
            let mut affinity = 0;
            GetWindowDisplayAffinity(overlay.0, &mut affinity)
                .map_err(|_| Error::Unsupported("capture exclusion"))?;
            if affinity != WDA_EXCLUDEFROMCAPTURE.0 {
                return Err(Error::Unsupported("capture exclusion"));
            }
            // Do not show a covering window until exclusion was accepted.
            overlay.cover()?;
            Ok(overlay)
        }
    }
    fn cover(&self) -> Result<(), Error> {
        unsafe {
            SetWindowPos(
                self.0,
                Some(HWND_TOPMOST),
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .map_err(|_| Error::Unsupported("privacy display geometry"))
        }
    }
}
impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            ignored_hr!(
                vnilink::trace::SYSTEM,
                "an overlay that cannot be destroyed goes with its thread",
                DestroyWindow(self.0)
            );
        }
    }
}
struct Command {
    on: bool,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<(), Error>>,
}
struct Worker {
    send: mpsc::SyncSender<Command>,
    thread: std::thread::JoinHandle<()>,
}
#[derive(Default)]
pub(crate) struct Privacy {
    worker: Mutex<Option<Worker>>,
    active: Arc<AtomicBool>,
}
impl Privacy {
    pub(crate) fn observed(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub(crate) fn set(&self, on: bool) -> Result<(), Error> {
        let mut worker = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        if worker.is_none() && !on {
            return Ok(());
        }
        if worker.is_none() {
            let (send, recv) = mpsc::sync_channel::<Command>(4);
            let active = self.active.clone();
            let thread = std::thread::Builder::new()
                .name("vnilink-win-privacy".into())
                .spawn(move || {
                    let mut overlay = None;
                    loop {
                        match recv.recv_timeout(Duration::from_millis(100)) {
                            Ok(command) => {
                                if Instant::now() >= command.deadline {
                                    vnilink::ignored!(
                                        vnilink::trace::SYSTEM,
                                        "the caller stopped waiting for its answer",
                                        command.reply.send(Err(Error::Timeout))
                                    );
                                    continue;
                                }
                                let result = if command.on && overlay.is_none() {
                                    Overlay::open().map(|window| overlay = Some(window))
                                } else {
                                    if !command.on {
                                        overlay = None;
                                    }
                                    Ok(())
                                };
                                active.store(overlay.is_some(), Ordering::Release);
                                if Instant::now() >= command.deadline
                                    || command.reply.send(result).is_err()
                                {
                                    overlay = None;
                                    active.store(false, Ordering::Release);
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        unsafe {
                            let mut message = MSG::default();
                            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                                // Whether it was translated changes nothing here.
                                let _translated = TranslateMessage(&message);
                                DispatchMessageW(&message);
                            }
                        }
                        if overlay
                            .as_ref()
                            .is_some_and(|window| window.cover().is_err())
                        {
                            overlay = None;
                            active.store(false, Ordering::Release);
                        }
                    }
                    drop(overlay);
                    active.store(false, Ordering::Release);
                })
                .map_err(|_| Error::Unsupported("privacy owner"))?;
            *worker = Some(Worker { send, thread });
        }
        let (reply, result) = mpsc::sync_channel(1);
        worker
            .as_ref()
            .unwrap()
            .send
            .try_send(Command {
                on,
                deadline: Instant::now() + Duration::from_secs(4),
                reply,
            })
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
#[cfg(test)]
mod tests {
    #[test]
    fn idle_owner_has_no_side_effects() {
        let owner = super::Privacy::default();
        assert!(!owner.observed());
        owner.set(false).unwrap();
    }
}
