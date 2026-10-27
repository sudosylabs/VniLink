//! The host's clipboard adapter, which each Desktop's clipboard pipe
//! applies a controller's text through.

use super::*;

use crate::clipboard::HostClipboard;
use crate::error::Error;

impl Core {
    /// Installs the adapter that receives clipboard text on this host.
    ///
    /// The library does not read or write the operating-system clipboard.
    /// An idle connection may configure it before a Desktop opens; while a
    /// Desktop streams it is refused as busy.
    pub fn set_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Result<(), Error> {
        let _engine = self.engine();
        let mut slot = lock(&self.shared.clipboard);
        let _state = self.alive_state()?;
        if self
            .shared
            .desktop_streams
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *slot = Some(clipboard);
        self.shared
            .clipboard_available
            .store(true, std::sync::atomic::Ordering::Release);
        tracing::info!(target: crate::trace::CLIPBOARD, "clipboard adapter installed");
        Ok(())
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::sync::mpsc;

    struct WaitingClipboard {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl HostClipboard for WaitingClipboard {
        fn set_text(&mut self, _: &str) -> Result<(), Error> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
    }

    /// A snapshot reads clipboard availability without the adapter, so the
    /// UI never waits for a native clipboard callback a paste is inside.
    #[test]
    fn snapshot_does_not_wait_for_a_native_clipboard_callback() {
        let config = || CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![
                crate::CustomRelay::new("http://127.0.0.1:9").unwrap()
            ])
            .unwrap(),
            ..CoreConfig::default()
        };
        let host = Arc::new(Core::start(config()).unwrap());
        assert!(!host.snapshot().unwrap().capabilities.clipboard);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        host.set_clipboard(Box::new(WaitingClipboard {
            entered: entered_tx,
            release: release_rx,
        }))
        .unwrap();
        let writer = host.clone();
        // A Desktop's pipe holds the adapter across the native callback.
        let write = std::thread::spawn(move || {
            lock(&writer.shared.clipboard)
                .as_mut()
                .expect("the installed adapter")
                .set_text("fixture")
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let observer = host.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let read = std::thread::spawn(move || {
            let _ = done_tx.send(observer.snapshot().unwrap().capabilities.clipboard);
        });
        let available = done_rx.recv_timeout(Duration::from_millis(200));
        // Always release the adapter, including the failing implementation.
        release_tx.send(()).unwrap();
        write.join().unwrap().unwrap();
        read.join().unwrap();
        host.shutdown();
        assert_eq!(
            available,
            Ok(true),
            "the UI snapshot blocked the native clipboard writer"
        );
    }
}
