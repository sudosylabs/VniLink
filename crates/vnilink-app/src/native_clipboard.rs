// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One native clipboard owner thread. Linux selection ownership survives individual writes.
// trace: records every ignored failure
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
/// What the operating system said when the clipboard failed, by kind only:
/// an unknown failure's description is the system's text and is not kept.
fn failure(error: &arboard::Error) -> &'static str {
    match error {
        arboard::Error::ContentNotAvailable => "content not available",
        arboard::Error::ClipboardNotSupported => "clipboard not supported",
        arboard::Error::ClipboardOccupied => "clipboard occupied",
        arboard::Error::ConversionFailure => "conversion failure",
        _ => "unknown",
    }
}

fn unavailable() -> FfiError {
    FfiError::Unsupported {
        message: "The operating system clipboard is unavailable".into(),
    }
}
impl NativeClipboard {
    pub(crate) fn open() -> Result<Self, FfiError> {
        Self::start(|| {
            let mut clipboard = arboard::Clipboard::new().map_err(|error| {
                tracing::warn!(
                    target: vnilink::trace::CLIPBOARD,
                    failure = failure(&error),
                    "this computer's clipboard could not be opened"
                );
                unavailable()
            })?;
            Ok(move |text: String| {
                let bytes = text.len();
                clipboard.set_text(text).map_err(|error| {
                    tracing::warn!(
                        target: vnilink::trace::CLIPBOARD,
                        bytes,
                        failure = failure(&error),
                        "this computer's clipboard refused the text"
                    );
                    unavailable()
                })
            })
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
            .spawn(vnilink::trace::within(
                tracing::Span::current(),
                move || {
                    let mut write = match factory() {
                        Ok(write) => {
                            if ready.send(Ok(())).is_err() {
                                return;
                            }
                            write
                        }
                        Err(error) => {
                            vnilink::ignored!(
                                vnilink::trace::CLIPBOARD,
                                "the opener stopped waiting for the clipboard",
                                ready.send(Err(error))
                            );
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
                                    tracing::debug!(
                                        target: vnilink::trace::CLIPBOARD,
                                        "a clipboard write was given up before it ran"
                                    );
                                    vnilink::ignored!(
                                        vnilink::trace::CLIPBOARD,
                                        "the writer stopped waiting for its answer",
                                        reply.send(Err(unavailable()))
                                    );
                                    continue;
                                }
                                // Success is acknowledged only after the OS has accepted the write.
                                let result = write(text);
                                vnilink::ignored!(
                                    vnilink::trace::CLIPBOARD,
                                    "the writer stopped waiting for its answer",
                                    reply.send(result)
                                );
                            }
                        }
                    }
                },
            ))
            .map_err(|_| unavailable())?;
        match initialized.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { sender, stopped }),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                tracing::warn!(
                    target: vnilink::trace::CLIPBOARD,
                    "this computer's clipboard did not open within 5 s"
                );
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
            .map_err(|_| {
                tracing::warn!(
                    target: vnilink::trace::CLIPBOARD,
                    "a clipboard write was refused: eight are already waiting"
                );
                unavailable()
            })?;
        match completion.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    target: vnilink::trace::CLIPBOARD,
                    "this computer's clipboard did not answer within 5 s"
                );
                cancelled.store(true, Ordering::Release);
                Err(unavailable())
            }
        }
    }
}
impl Drop for NativeClipboard {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        vnilink::ignored!(
            vnilink::trace::CLIPBOARD,
            "a full or stopped writer ends with its last sender",
            self.sender.try_send(Request::Stop)
        );
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
