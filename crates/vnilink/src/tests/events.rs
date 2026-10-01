use std::time::Duration;

use crate::events::{CoreEvent, EventHub, MAX_SUBSCRIBERS};

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
