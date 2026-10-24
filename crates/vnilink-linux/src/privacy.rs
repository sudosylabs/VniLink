//! X11 display privacy changes scanout gamma, leaving capture pixels intact.
//! Wayland has no compositor-independent gamma API; never claim support there.
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use vnilink::Error;
use x11rb::{
    connection::Connection,
    protocol::randr::{ConnectionExt, GetCrtcGammaReply},
    rust_connection::RustConnection,
};
struct Saved {
    crtc: u32,
    outputs: Vec<(u32, Vec<u8>)>,
    gamma: GetCrtcGammaReply,
}
struct Native {
    connection: RustConnection,
    root: u32,
    saved: Vec<Saved>,
}
impl Native {
    fn open() -> Result<Self, Error> {
        if crate::session::is_wayland() {
            return Err(Error::Unsupported(
                "Wayland display privacy requires compositor support",
            ));
        }
        let (connection, screen) =
            x11rb::connect(None).map_err(|_| Error::Unsupported("X11 display privacy"))?;
        let root = connection.setup().roots[screen].root;
        Ok(Self {
            connection,
            root,
            saved: Vec::new(),
        })
    }
    fn current(&self) -> Result<Vec<Saved>, Error> {
        let result = (|| {
            let resources = self
                .connection
                .randr_get_screen_resources_current(self.root)?
                .reply()?;
            let mut current = Vec::new();
            for crtc in resources.crtcs {
                let info = self
                    .connection
                    .randr_get_crtc_info(crtc, resources.config_timestamp)?
                    .reply()?;
                if info.outputs.is_empty() {
                    continue;
                }
                let mut outputs = Vec::new();
                for output in info.outputs {
                    let identity = self
                        .connection
                        .randr_get_output_info(output, resources.config_timestamp)?
                        .reply()?;
                    outputs.push((output, identity.name));
                }
                outputs.sort();
                let gamma = self.connection.randr_get_crtc_gamma(crtc)?.reply()?;
                if gamma.red.is_empty()
                    || gamma.red.len() > 4096
                    || gamma.red.len() != gamma.green.len()
                    || gamma.red.len() != gamma.blue.len()
                {
                    return Err(Box::<dyn std::error::Error>::from("unsupported gamma ramp"));
                }
                current.push(Saved {
                    crtc,
                    outputs,
                    gamma,
                });
            }
            if current.is_empty() {
                return Err(Box::<dyn std::error::Error>::from("no active scanout"));
            }
            Ok::<_, Box<dyn std::error::Error>>(current)
        })();
        result.map_err(|_| Error::Unsupported("X11 gamma ramps"))
    }
    fn write(&self, crtc: u32, r: &[u16], g: &[u16], b: &[u16]) -> Result<(), Error> {
        self.connection
            .randr_set_crtc_gamma(crtc, r, g, b)
            .map_err(|_| Error::Unsupported("X11 gamma request"))?
            .check()
            .map_err(|_| Error::Unsupported("X11 gamma request"))
    }
    fn apply(&mut self) -> Result<(), Error> {
        let current = self.current()?;
        for display in current {
            let count = display.gamma.red.len();
            let crtc = display.crtc;
            if !self
                .saved
                .iter()
                .any(|s| s.crtc == display.crtc && s.outputs == display.outputs)
            {
                if self.saved.len() >= 64 {
                    return Err(Error::Unsupported("privacy display history limit"));
                }
                self.saved.push(display);
            }
            let zeros = vec![0; count];
            self.write(crtc, &zeros, &zeros, &zeros)?;
        }
        Ok(())
    }
    fn restore(&mut self) -> Result<(), Error> {
        if self.saved.is_empty() {
            return Ok(());
        }
        let current = self.current()?;
        let mut pending = Vec::new();
        let mut failed = false;
        for saved in std::mem::take(&mut self.saved) {
            let attached = current
                .iter()
                .any(|s| s.crtc == saved.crtc && s.outputs == saved.outputs);
            if !attached
                || self
                    .write(
                        saved.crtc,
                        &saved.gamma.red,
                        &saved.gamma.green,
                        &saved.gamma.blue,
                    )
                    .is_err()
            {
                failed |= attached;
                pending.push(saved);
            }
        }
        self.saved = pending;
        if failed {
            Err(Error::Unsupported("X11 gamma restoration"))
        } else {
            Ok(())
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
pub(crate) fn supported() -> bool {
    Native::open().and_then(|n| n.current()).is_ok()
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
                .name("vnilink-x11-privacy".into())
                .spawn(move || {
                    let mut native = Native::open();
                    let mut private = false;
                    loop {
                        match recv.recv_timeout(Duration::from_millis(250)) {
                            Ok(command) => {
                                if Instant::now() >= command.deadline {
                                    let _ = command.reply.send(Err(Error::Timeout));
                                    continue;
                                }
                                let result = match native.as_mut() {
                                    Ok(n) => {
                                        if command.on {
                                            n.apply()
                                        } else {
                                            n.restore()
                                        }
                                    }
                                    Err(_) => Err(Error::Unsupported("X11 display privacy")),
                                };
                                if result.is_ok() {
                                    private = command.on;
                                } else if command.on {
                                    if let Ok(n) = native.as_mut() {
                                        private = n.restore().is_err();
                                    }
                                }
                                active.store(private, Ordering::Release);
                                if Instant::now() >= command.deadline
                                    || command.reply.send(result).is_err()
                                {
                                    if let Ok(n) = native.as_mut() {
                                        private = n.restore().is_err();
                                    }
                                    active.store(private, Ordering::Release);
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        if let Ok(n) = native.as_mut() {
                            if private && n.apply().is_err() {
                                private = n.restore().is_err();
                                active.store(private, Ordering::Release);
                            }
                            if !private {
                                let _ = n.restore();
                            }
                        }
                    }
                    drop(native);
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
            let _ = worker.thread.join();
        }
    }
}
