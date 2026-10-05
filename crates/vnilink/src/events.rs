use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::protocol::Quality;
use crate::session::{SessionPhase, SessionRole};

pub(crate) const MAX_SUBSCRIBERS: usize = 8;

/// Changes since the last [`crate::Core::snapshot`].
///
/// The core does not call back onto a UI thread. Poll [`EventSubscription`]
/// on the thread that owns the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    HostingChanged {
        hosting: bool,
    },
    DevicePaired {
        endpoint_id: String,
    },
    DeviceAuthorized {
        endpoint_id: String,
    },
    DeviceRevoked {
        endpoint_id: String,
    },
    /// A paired device was forgotten with [`crate::Core::unpair`].
    ///
    /// Unlike revocation, which keeps the device with empty grants, the
    /// device is gone: later dials fail with `NotPaired`. Any live session
    /// with that peer ends. This is local only; the peer learns nothing
    /// until its next dial.
    DeviceUnpaired {
        endpoint_id: String,
    },
    /// A controller opened this host's pairing file and is connecting.
    ///
    /// `check` is the three digits on the controller's screen. Grant control
    /// with [`crate::Core::confirm_control_request`] only when the person
    /// types those digits.
    ControlRequested {
        endpoint_id: String,
        check: String,
    },
    /// A paired device replaced its saved relay URLs and direct addresses.
    ///
    /// Grants are unchanged. The list is what the next dial uses.
    AddressesUpdated {
        endpoint_id: String,
    },
    /// A paired device's displayed name changed, through a peer hostname
    /// refresh or a local pet-name edit. Read the snapshot for both names;
    /// grants are unchanged.
    PeerRenamed {
        endpoint_id: String,
        hostname: Option<String>,
    },
    SessionChanged {
        id: u64,
        generation: u64,
        role: SessionRole,
        phase: SessionPhase,
        peer_id: String,
    },
    QualityUpdated {
        id: u64,
        quality: Quality,
    },
    /// The host could not apply an input event, or the controller was told
    /// that input will not be applied. The message names the reason, not the key.
    InputFailed {
        id: u64,
        message: &'static str,
    },
    /// The host should encode a keyframe for this session.
    ///
    /// Predicted frames were dropped, or the controller asked for a keyframe.
    KeyframeRequested {
        id: u64,
    },
    /// The host published clipboard text. Read it with [`crate::Core::clipboard_text`].
    ///
    /// The event carries no characters. Clipboard text is not logged.
    ClipboardReceived {
        id: u64,
    },
    /// A file was written for this session.
    ///
    /// The event carries no path and no bytes. File contents are not logged.
    TransferCompleted {
        id: u64,
    },
}

pub struct EventSubscription {
    rx: Receiver<CoreEvent>,
    lagged: Arc<AtomicU64>,
    /// Keeps this subscriber counted until the subscription is dropped.
    _alive: Arc<()>,
}

impl EventSubscription {
    pub fn try_recv(&self) -> Option<CoreEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<CoreEvent> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Events dropped because this subscriber was behind the bounded queue.
    pub fn lagged(&self) -> u64 {
        self.lagged.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub(crate) struct EventHub {
    capacity: usize,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

struct Subscriber {
    tx: SyncSender<CoreEvent>,
    lagged: Arc<AtomicU64>,
    alive: Weak<()>,
}

impl EventHub {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn subscribe(&self) -> Result<EventSubscription, crate::error::Error> {
        let (tx, rx) = std::sync::mpsc::sync_channel(self.capacity);
        let lagged = Arc::new(AtomicU64::new(0));
        let alive = Arc::new(());
        let mut subscribers = self.subscribers.lock().expect("event subscribers");
        subscribers.retain(|subscriber| subscriber.alive.upgrade().is_some());
        if subscribers.len() >= MAX_SUBSCRIBERS {
            return Err(crate::error::Error::Busy);
        }
        subscribers.push(Subscriber {
            tx,
            lagged: Arc::clone(&lagged),
            alive: Arc::downgrade(&alive),
        });
        Ok(EventSubscription {
            rx,
            lagged,
            _alive: alive,
        })
    }

    pub(crate) fn publish(&self, event: CoreEvent) {
        let mut subscribers = self.subscribers.lock().expect("event subscribers");
        subscribers.retain(|subscriber| match subscriber.tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                subscriber.lagged.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }
}
