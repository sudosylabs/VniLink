//! One native clipboard owner thread. Linux selection ownership survives individual writes.
use crate::{ClipboardWriter, FfiError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

type WriteResult = Result<(), FfiError>;
enum Request {
    Write {
        text: String,
        reply: mpsc::SyncSender<WriteResult>,
        cancelled: Arc<AtomicBool>,
    },
    Stop,
}
pub(crate) struct NativeClipboard {
    sender: mpsc::SyncSender<Request>,
    stopped: Arc<AtomicBool>,
}
fn unavailable() -> FfiError {
    FfiError::Unsupported {
        message: "The operating system clipboard is unavailable".into(),
    }
}
impl NativeClipboard {
    pub(crate) fn open() -> Result<Self, FfiError> {
        Self::start(|| {
            let mut clipboard = arboard::Clipboard::new().map_err(|_| unavailable())?;
            Ok(move |text: String| clipboard.set_text(text).map_err(|_| unavailable()))
        })
    }
    fn start<F, W>(factory: F) -> Result<Self, FfiError>
    where
        F: FnOnce() -> Result<W, FfiError> + Send + 'static,
        W: FnMut(String) -> WriteResult + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(8);
        let (ready, initialized) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = stopped.clone();
        std::thread::Builder::new()
            .name("vnilink-clipboard".into())
            .spawn(move || {
                let mut write = match factory() {
                    Ok(write) => {
                        if ready.send(Ok(())).is_err() {
                            return;
                        }
                        write
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Stop => break,
                        Request::Write {
                            text,
                            reply,
                            cancelled,
                        } => {
                            if worker_stop.load(Ordering::Acquire)
                                || cancelled.load(Ordering::Acquire)
                            {
                                let _ = reply.send(Err(unavailable()));
                                continue;
                            }
                            // Success is acknowledged only after the OS has accepted the write.
                            let result = write(text);
                            let _ = reply.send(result);
                        }
                    }
                }
            })
            .map_err(|_| unavailable())?;
        match initialized.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { sender, stopped }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                stopped.store(true, Ordering::Release);
                Err(unavailable())
            }
        }
    }
}
impl ClipboardWriter for NativeClipboard {
    fn set_text(&self, text: String) -> WriteResult {
        let (reply, completion) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.sender
            .try_send(Request::Write {
                text,
                reply,
                cancelled: cancelled.clone(),
            })
            .map_err(|_| unavailable())?;
        match completion.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => {
                cancelled.store(true, Ordering::Release);
                Err(unavailable())
            }
        }
    }
}
impl Drop for NativeClipboard {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.sender.try_send(Request::Stop);
        // The last sender is dropped next; no destructor waits on an OS operation.
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[test]
    fn writes_use_one_owner_thread_and_os_failures_are_not_acknowledged_as_success() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let output = seen.clone();
        let clipboard = NativeClipboard::start(move || {
            Ok(move |text: String| {
                if text == "refused" {
                    return Err(unavailable());
                }
                output
                    .lock()
                    .unwrap()
                    .push((std::thread::current().id(), text));
                Ok(())
            })
        })
        .unwrap();
        clipboard.set_text("first".into()).unwrap();
        assert!(clipboard.set_text("refused".into()).is_err());
        clipboard.set_text("second".into()).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, seen[1].0);
        assert_ne!(seen[0].0, std::thread::current().id());
        assert_eq!(seen[1].1, "second");
    }
}
