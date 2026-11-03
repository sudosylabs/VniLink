// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::time::Duration;

use crate::session::{SessionPhase, SessionRole};
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Arc, Mutex, MutexGuard, Weak};

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
    /// The digits are on the controller's screen, not here. Remember its
    /// identity with [`crate::Core::confirm_control_request`] only when the
    /// person types those digits.
    ControlRequested {
        endpoint_id: String,
    },
    /// This computer, as the controller, connected to `endpoint_id` and waits
    /// for its owner to type `check` there. The digits come from this
    /// connection and both computers' ids, so no other connection shows them.
    ControlCheck {
        endpoint_id: String,
        check: String,
    },
    /// A paired computer asks to start using what this computer shares, and
    /// waits `remaining_ms` for the person here to answer with
    /// [`crate::Core::answer_admission_request`].
    AdmissionRequested {
        request_id: u64,
        endpoint_id: String,
        remaining_ms: u64,
    },
    /// Admission request `request_id` was answered, timed out, or its
    /// computer stopped waiting: nothing is left to answer.
    AdmissionRequestEnded {
        request_id: u64,
    },
    /// This computer, as a controller, starts or stops waiting for the person
    /// at `endpoint_id` to accept it.
    AdmissionWaiting {
        endpoint_id: String,
        waiting: bool,
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

    /// Events dropped because this subscriber was behind the bounded queue
    /// since the previous call. Reading resets the count, so a caller that
    /// resynchronises on a non-zero value does so once per overflow.
    pub fn take_lagged(&self) -> u64 {
        self.lagged.swap(0, Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub(crate) struct EventHub {
    capacity: usize,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
    /// Woken when a session or a service changes, for work that waits for
    /// one instead of polling for it.
    changes: Arc<tokio::sync::Notify>,
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
            changes: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Wakes, once, after any session or service change.
    pub(crate) fn changes(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.changes)
    }

    /// The subscriber list is a set of channels, valid whatever a panicking
    /// thread was doing to it, so a poisoned lock is still usable.
    fn subscribers(&self) -> MutexGuard<'_, Vec<Subscriber>> {
        self.subscribers
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn subscribe(&self) -> Result<EventSubscription, crate::error::Error> {
        let (tx, rx) = std::sync::mpsc::sync_channel(self.capacity);
        let lagged = Arc::new(AtomicU64::new(0));
        let alive = Arc::new(());
        let mut subscribers = self.subscribers();
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
        if matches!(
            event,
            CoreEvent::SessionChanged { .. }
                | CoreEvent::ServiceChanged { .. }
                | CoreEvent::HostedServiceChanged { .. }
        ) {
            self.changes.notify_one();
        }
        let mut subscribers = self.subscribers();
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
    fn a_panic_in_a_subscriber_list_user_does_not_stop_events() {
        let hub = EventHub::new(4);
        let subscription = hub.subscribe().unwrap();
        let poisoner = hub.subscribers.clone();
        let _ = std::thread::spawn(move || {
            let _held = poisoner.lock().unwrap();
            panic!("a thread dies holding the list");
        })
        .join();
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        assert!(hub.subscribe().is_ok());
        drop(subscription);
    }

    #[test]
    fn a_slow_subscriber_does_not_block_the_publisher() {
        let hub = EventHub::new(1);
        let subscription = hub.subscribe().unwrap();
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        hub.publish(CoreEvent::HostingChanged { hosting: false });
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        assert_eq!(subscription.take_lagged(), 2);
        assert_eq!(
            subscription.try_recv(),
            Some(CoreEvent::HostingChanged { hosting: true })
        );
        assert!(subscription.try_recv().is_none());
    }

    #[test]
    fn an_overflow_is_reported_once() {
        let hub = EventHub::new(1);
        let subscription = hub.subscribe().unwrap();
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        hub.publish(CoreEvent::HostingChanged { hosting: false });
        assert_eq!(subscription.take_lagged(), 1);
        assert_eq!(subscription.take_lagged(), 0, "reading resets the count");
        assert!(subscription.try_recv().is_some());
        hub.publish(CoreEvent::HostingChanged { hosting: true });
        assert_eq!(subscription.take_lagged(), 0, "a later event fits");
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
        assert_eq!(subscription.take_lagged(), 0);
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
        assert_eq!(replacement.take_lagged(), 0);
    }
}

#[cfg(all(test, vnilink_loom))]
mod loom_events {
    use super::*;

    const ON: CoreEvent = CoreEvent::HostingChanged { hosting: true };
    const OFF: CoreEvent = CoreEvent::HostingChanged { hosting: false };

    /// Drains what the subscription holds and returns how many events that was.
    fn drain(subscription: &EventSubscription) -> u64 {
        let mut received = 0;
        while subscription.try_recv().is_some() {
            received += 1;
        }
        received
    }

    #[test]
    fn an_overflow_read_while_events_are_published_is_counted_once() {
        loom::model(|| {
            let hub = EventHub::new(1);
            let subscription = hub.subscribe().unwrap();
            let publisher = {
                let hub = hub.clone();
                loom::thread::spawn(move || {
                    hub.publish(ON);
                    hub.publish(OFF);
                    hub.publish(ON);
                })
            };
            let mut lagged = subscription.take_lagged();
            let mut received = drain(&subscription);
            publisher.join().unwrap();
            lagged += subscription.take_lagged();
            received += drain(&subscription);
            assert_eq!(
                received + lagged,
                3,
                "every event is received or counted as lagged, once"
            );
            assert_eq!(subscription.take_lagged(), 0, "reading resets the count");
        });
    }

    #[test]
    fn publishers_on_two_threads_lose_no_count() {
        loom::model(|| {
            let hub = EventHub::new(1);
            let subscription = hub.subscribe().unwrap();
            let publishers: Vec<_> = [ON, OFF]
                .into_iter()
                .map(|event| {
                    let hub = hub.clone();
                    loom::thread::spawn(move || {
                        hub.publish(event.clone());
                        hub.publish(event);
                    })
                })
                .collect();
            for publisher in publishers {
                publisher.join().unwrap();
            }
            assert_eq!(drain(&subscription), 1, "the queue holds one event");
            assert_eq!(
                subscription.take_lagged(),
                3,
                "each event the full queue turned away is counted"
            );
        });
    }
}
