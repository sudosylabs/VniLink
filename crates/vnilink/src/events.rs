use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

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
    /// A folder watched with [`crate::Core::watch_folder`] changed.
    FolderChanged {
        watch: u64,
    },
    /// A paired computer asked this one to restart or shut down. It runs
    /// after `remaining_ms` unless cancelled; show its user the notice.
    SystemActionScheduled {
        operation_id: u64,
        action: crate::system::SystemAction,
        endpoint_id: String,
        remaining_ms: u32,
    },
    /// A scheduled restart or shutdown ran, failed, was refused or was cancelled.
    SystemActionEnded {
        operation_id: u64,
        state: crate::system::OperationState,
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
    /// The device is gone: later dials fail with `NotPaired`. Any live session
    /// with that peer ends. This is local only; the peer learns nothing
    /// until its next dial.
    DeviceUnpaired {
        endpoint_id: String,
    },
    /// A controller opened this host's pairing file and is connecting.
    ///
    /// `check` is the three digits on the controller's screen. Remember its identity
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
    /// A paired computer became reachable, unreachable, or stopped accepting
    /// connections. Only emitted while presence links run.
    PresenceChanged {
        endpoint_id: String,
        presence: crate::DevicePresence,
    },
    /// A paired computer sent a new report of what it allows; read it with
    /// [`crate::Core::peer_availability`].
    PeerAvailabilityChanged {
        endpoint_id: String,
    },
    SessionChanged {
        id: u64,
        generation: u64,
        role: SessionRole,
        phase: SessionPhase,
        peer_id: String,
    },
    /// Local service ownership changed. No payload bytes cross this event.
    ServiceChanged {
        service: crate::ServiceInfo,
    },
    HostedServiceChanged {
        service: crate::ServiceInfo,
    },
    /// The host could not apply an input event, or the controller was told
    /// that input will not be applied. The message names the reason, not the key.
    InputFailed {
        id: u64,
        message: &'static str,
    },
    /// The host published clipboard text. Read it with [`crate::Core::clipboard_text`].
    ///
    /// The event carries no characters. Clipboard text is not logged.
    ClipboardReceived {
        id: u64,
    },
    /// A file selection offer arrived. Read it with
    /// [`crate::Core::clipboard_files`] or
    /// [`crate::Core::incoming_clipboard_files`].
    ///
    /// The event carries no paths. Selections are not logged.
    ClipboardFilesReceived {
        id: u64,
    },
    /// Host text for an exact Desktop owner. Carries no clipboard contents.
    DesktopClipboardReceived {
        service: crate::ServiceInfo,
    },
    /// A display snapshot was published or received. Read the topology for
    /// the records. `revision` 0 is not used.
    TopologyChanged {
        revision: u64,
    },
    /// The lock-after-control policy fired the workstation lock. `locked`
    /// is false when no backend is installed or the OS call failed; the
    /// edge is consumed either way and re-arms on the next control owner.
    HostLocked {
        locked: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_subscriber_does_not_block_the_publisher() {
        let hub = EventHub::new(1);
        let subscription = hub.subscribe().unwrap();
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        hub.publish(CoreEvent::HostingChanged { hosting: false });
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        assert_eq!(subscription.lagged(), 2);
        assert_eq!(
            subscription.try_recv(),
            Some(CoreEvent::HostingChanged { hosting: true })
        );
        assert!(subscription.try_recv().is_none());
    }

    #[test]
    fn a_subscriber_that_keeps_up_does_not_lag() {
        let hub = EventHub::new(2);
        let subscription = hub.subscribe().unwrap();
        assert!(subscription
            .recv_timeout(Duration::from_millis(0))
            .is_none());
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        hub.publish(CoreEvent::HostingChanged { hosting: false });
        assert_eq!(subscription.lagged(), 0);
        assert_eq!(
            subscription.recv_timeout(Duration::ZERO),
            Some(CoreEvent::HostingChanged { hosting: true })
        );
        assert_eq!(
            subscription.try_recv(),
            Some(CoreEvent::HostingChanged { hosting: false })
        );
        assert!(subscription.recv_timeout(Duration::ZERO).is_none());
    }

    #[test]
    fn the_ninth_subscriber_is_busy_until_one_is_dropped() {
        let hub = EventHub::new(1);
        let mut subscriptions = Vec::new();
        for _ in 0..MAX_SUBSCRIBERS {
            subscriptions.push(hub.subscribe().unwrap());
        }
        assert!(matches!(hub.subscribe(), Err(crate::error::Error::Busy)));
        subscriptions.pop();
        let replacement = hub.subscribe().unwrap();
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        assert_eq!(
            replacement.try_recv(),
            Some(CoreEvent::HostingChanged { hosting: true })
        );
        assert_eq!(replacement.lagged(), 0);
    }
}
