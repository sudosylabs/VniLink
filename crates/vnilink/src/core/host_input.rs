// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A host's side of one viewer's desktop input: whether an input may act
//! under the lease it was sent with ([`admit`]), what became of it
//! ([`InputOutcome`], recorded by [`record`] so no input is dropped
//! unrecorded), and what the host applied and has not released
//! ([`HostInputSession`]). The desktop stream reads the facts and calls the
//! backend; a failure that ends the stream is an `Err`, never an outcome.
use super::desktop_stream::DesktopControlState;
use super::input_cleanup::{AdmissionSlot, Reservation};
use super::*;
use crate::input::HeldInput;
use crate::ServiceInfo;

/// What became of one input a viewer sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputOutcome {
    Applied,
    Refused { reason: Refusal },
}

/// Why a host did not apply an input. The first six are the lease: whether
/// this viewer may act now. The rest are the input itself or the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The peer may not control this computer.
    NotPermitted,
    /// Another viewer holds control.
    AnotherViewer,
    /// Nobody, or not this viewer, holds control.
    NotHolding,
    /// The viewer sent it under a lease it no longer holds.
    EarlierLease,
    /// The lease changed between admission and the backend taking it.
    ControlChanged,
    /// The desktop ended before the backend took it.
    Ended,
    /// The pointer names a place the host does not show.
    PointerOutside(&'static str),
    /// Text a viewer may not send, as one over the length limit.
    InvalidText(&'static str),
    /// The host cannot apply this kind of input.
    Unavailable(&'static str),
    /// The backend tried and the operating system refused.
    Failed(&'static str),
}

impl Refusal {
    /// The reason a record and the host's user read.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::NotPermitted => "the peer may not control this computer",
            Self::AnotherViewer => "another viewer controls this computer",
            Self::NotHolding => "this viewer does not hold control",
            Self::EarlierLease => "it was sent under an earlier control lease",
            Self::ControlChanged => "control changed before it could apply",
            Self::Ended => "the service ended before it could apply",
            Self::PointerOutside(reason)
            | Self::InvalidText(reason)
            | Self::Unavailable(reason)
            | Self::Failed(reason) => reason,
        }
    }

    /// Whether the host could not do what a permitted viewer asked, which
    /// its user hears of; a lease refusal is the protocol working.
    fn is_fault(self) -> bool {
        matches!(
            self,
            Self::PointerOutside(_) | Self::InvalidText(_) | Self::Unavailable(_) | Self::Failed(_)
        )
    }

    /// The refusal a backend's failure is, or the failure itself when it
    /// ends the stream.
    pub(super) fn of_backend(error: Error) -> Result<Self, Error> {
        match error {
            Error::InputFailed(reason) => Ok(Self::Failed(reason)),
            Error::Unsupported(reason) => Ok(Self::Unavailable(reason)),
            other => Err(other),
        }
    }
}

/// Records what became of one input of `kind` on a hosted desktop: its kind
/// and reason, never its keys, text or position. A fault also tells the
/// host's user, on the desktop's connection, once for each reason until
/// an input applies: a backend that cannot apply pointer moves would
/// otherwise say so for every move.
pub(super) fn record(
    shared: &Shared,
    service: &ServiceInfo,
    session: &HostInputSession,
    kind: &'static str,
    outcome: InputOutcome,
) {
    match outcome {
        InputOutcome::Applied => {
            *lock(&session.fault) = None;
            tracing::trace!(
                target: crate::trace::INPUT,
                service = service.id,
                kind,
                "input applied"
            )
        }
        InputOutcome::Refused { reason }
            if reason.is_fault() && lock(&session.fault).replace(reason) != Some(reason) =>
        {
            tracing::warn!(
                target: crate::trace::INPUT,
                service = service.id,
                kind,
                reason = reason.label(),
                "input refused"
            );
            shared.events.publish(CoreEvent::InputFailed {
                id: service.connection_id,
                message: reason.label(),
            });
        }
        InputOutcome::Refused { reason } => tracing::debug!(
            target: crate::trace::INPUT,
            service = service.id,
            kind,
            reason = reason.label(),
            "input refused"
        ),
    }
}

/// The lease an input was admitted under: the epoch the viewer sent it
/// with, while this viewer held control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Lease {
    epoch: u64,
}

impl Lease {
    /// Whether this lease is still the desktop's, as its control state
    /// `now` and whether it is `retired` say.
    pub(super) fn holds(self, now: (DesktopControlState, u64), retired: bool) -> bool {
        !retired && now == (DesktopControlState::Controlling, self.epoch)
    }
}

/// Who holds control of this computer, as seen from one desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Holder {
    This,
    Another,
    Nobody,
}

/// What admission reads, at one moment, of the peer, the computer and the
/// desktop.
#[derive(Debug, Clone, Copy)]
pub(super) struct Facts {
    /// The peer may view and control this computer.
    pub(super) permitted: bool,
    pub(super) holder: Holder,
    /// The epoch the viewer sent the input with.
    pub(super) sent_under: u64,
    /// The desktop's control state and epoch now.
    pub(super) lease: (DesktopControlState, u64),
    /// The desktop has closed.
    pub(super) retired: bool,
}

/// Whether an input may act: the lease it may act under, or why not.
pub(super) fn admit(facts: Facts) -> Result<Lease, Refusal> {
    if facts.retired {
        Err(Refusal::Ended)
    } else if !facts.permitted {
        Err(Refusal::NotPermitted)
    } else if facts.holder == Holder::Another {
        Err(Refusal::AnotherViewer)
    } else if facts.holder == Holder::Nobody || facts.lease.0 != DesktopControlState::Controlling {
        Err(Refusal::NotHolding)
    } else if facts.sent_under != facts.lease.1 {
        Err(Refusal::EarlierLease)
    } else {
        Ok(Lease {
            epoch: facts.sent_under,
        })
    }
}

/// Admission again once the backend is free: any change since is the
/// lease changing, unless the desktop ended.
pub(super) fn readmit(facts: Facts) -> Result<Lease, Refusal> {
    admit(facts).map_err(|reason| match reason {
        Refusal::Ended => Refusal::Ended,
        _ => Refusal::ControlChanged,
    })
}

/// What a host applied for one desktop and has not released, the native
/// calls under way for it, and the release queued for them. The lease
/// itself stays on the desktop, which viewer and host share.
pub(super) struct HostInputSession {
    /// Keys and buttons applied and not yet released: the host's own
    /// account, apart from what the viewer planned.
    held: Arc<Mutex<HeldInput>>,
    /// A release waits on the cleanup worker.
    releasing: Arc<AtomicBool>,
    /// Native calls admitted and not yet returned.
    in_flight: AtomicUsize,
    /// The desktop's admission slot, which its last release holds.
    slot: Mutex<Option<Arc<AdmissionSlot>>>,
    /// The fault the host's user last heard of, until an input applies.
    fault: Mutex<Option<Refusal>>,
}

/// One native call under way, counted until it drops.
pub(super) struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

impl HostInputSession {
    pub(super) fn new() -> Self {
        Self {
            held: Arc::new(Mutex::new(HeldInput::default())),
            releasing: Arc::new(AtomicBool::new(false)),
            in_flight: AtomicUsize::new(0),
            slot: Mutex::new(None),
            fault: Mutex::new(None),
        }
    }

    /// The desktop's admission slot, held until its input is released.
    pub(super) fn keep_slot(&self, slot: Arc<AdmissionSlot>) {
        *lock(&self.slot) = Some(slot);
    }

    /// Counts a native call admitted under Core state, so a release taken
    /// before it returns still runs.
    pub(super) fn enter(&self) -> InFlight<'_> {
        self.in_flight
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        InFlight(&self.in_flight)
    }

    /// Notes `event` applied. When its lease no longer `holds`, releases
    /// everything held, as [`Self::settle`] does: the viewer that pressed
    /// it can no longer release it. Says whether the lease still holds.
    pub(super) fn applied(
        &self,
        event: crate::InputEvent,
        holds: bool,
        adapter: &mut dyn crate::HostDesktop,
        failed: impl FnMut(&Error),
    ) -> bool {
        lock(&self.held).observe(event);
        !self.settle(holds, adapter, failed)
    }

    /// After a native call whose lease may have ended while it ran: when
    /// it no longer `holds`, releases what is held, telling `failed` of
    /// each release the backend refused. Says whether it ended.
    pub(super) fn settle(
        &self,
        holds: bool,
        adapter: &mut dyn crate::HostDesktop,
        mut failed: impl FnMut(&Error),
    ) -> bool {
        if !holds {
            let mut held = lock(&self.held);
            for event in held.release_events() {
                if let Err(error) = adapter.apply(event) {
                    failed(&error);
                }
            }
            *held = HeldInput::default();
        }
        !holds
    }

    /// Releases what is held on the cleanup worker, after any native call
    /// under way. Called under Core state, so it never waits for the held
    /// set, which a late native release may own; one release is queued at
    /// a time.
    pub(super) fn release(&self, shared: &Shared, connection_id: u64) {
        use std::sync::atomic::Ordering;
        if self.in_flight.load(Ordering::Acquire) == 0
            && self
                .held
                .try_lock()
                .is_ok_and(|held| held.release_events().is_empty())
        {
            return;
        }
        if self.releasing.swap(true, Ordering::AcqRel) {
            return;
        }
        shared.input_cleanup.fetch_add(1, Ordering::AcqRel);
        let reservation = Reservation {
            pending: shared.input_cleanup.clone(),
            releasing: self.releasing.clone(),
        };
        let slot = lock(&self.slot).clone();
        let adapter = shared.desktop.clone();
        let held = self.held.clone();
        let events = shared.events.clone();
        shared.cleanup_executor.submit(move || {
            let mut adapter = lock(&adapter);
            let releases = std::mem::take(&mut *lock(&held));
            if let Some(adapter) = adapter.as_mut() {
                for event in releases.release_events() {
                    if adapter.apply(event).is_err() {
                        events.publish(CoreEvent::InputFailed {
                            id: connection_id,
                            message: "a held key or button could not be released",
                        });
                    }
                }
            }
            // Both reservation and owner slot settle before the adapter is yielded.
            drop(reservation);
            drop(slot);
        });
    }

    /// Marks `event` held as if applied, for a test that needs held input
    /// without a viewer.
    #[cfg(test)]
    pub(super) fn hold(&self, event: crate::InputEvent) {
        lock(&self.held).observe(event);
    }
}

#[cfg(any(test, feature = "test-probes"))]
impl Core {
    /// Releases of held input queued on the cleanup worker and not yet
    /// done, for a test that states what a host applied once they settle.
    pub fn input_releases_pending(&self) -> usize {
        let _engine = self.engine();
        self.shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backend_failure_is_refused_and_any_other_error_ends_the_stream() {
        assert_eq!(
            Refusal::of_backend(Error::InputFailed("rejected")).ok(),
            Some(Refusal::Failed("rejected")),
            "an operating system refusal is the input's outcome"
        );
        assert_eq!(
            Refusal::of_backend(Error::Unsupported("no text")).ok(),
            Some(Refusal::Unavailable("no text")),
            "input the host cannot apply is the input's outcome"
        );
        assert!(
            matches!(Refusal::of_backend(Error::Malformed), Err(Error::Malformed)),
            "any other backend error ends the stream as before"
        );
    }

    #[test]
    fn only_what_the_host_could_not_do_is_a_fault() {
        for lease in [
            Refusal::NotPermitted,
            Refusal::AnotherViewer,
            Refusal::NotHolding,
            Refusal::EarlierLease,
            Refusal::ControlChanged,
            Refusal::Ended,
        ] {
            assert!(
                !lease.is_fault(),
                "{lease:?} is the lease working, not a fault"
            );
        }
        for fault in [
            Refusal::PointerOutside("outside"),
            Refusal::InvalidText("long"),
            Refusal::Unavailable("none"),
            Refusal::Failed("rejected"),
        ] {
            assert!(
                fault.is_fault(),
                "{fault:?} is a fault the host's user hears of"
            );
        }
    }

    fn facts() -> Facts {
        Facts {
            permitted: true,
            holder: Holder::This,
            sent_under: 3,
            lease: (DesktopControlState::Controlling, 3),
            retired: false,
        }
    }

    #[test]
    fn input_under_the_current_lease_of_the_holder_is_admitted() {
        assert_eq!(
            admit(facts()).map(|lease| lease.epoch),
            Ok(3),
            "the holder's input under its lease acts under that lease"
        );
    }

    #[test]
    fn each_lease_fact_refuses_with_its_own_reason() {
        let cases = [
            (
                Facts {
                    retired: true,
                    ..facts()
                },
                Refusal::Ended,
            ),
            (
                Facts {
                    permitted: false,
                    ..facts()
                },
                Refusal::NotPermitted,
            ),
            (
                Facts {
                    holder: Holder::Another,
                    ..facts()
                },
                Refusal::AnotherViewer,
            ),
            (
                Facts {
                    holder: Holder::Nobody,
                    ..facts()
                },
                Refusal::NotHolding,
            ),
            (
                Facts {
                    lease: (DesktopControlState::Contended, 3),
                    ..facts()
                },
                Refusal::NotHolding,
            ),
            (
                Facts {
                    sent_under: 2,
                    ..facts()
                },
                Refusal::EarlierLease,
            ),
        ];
        for (facts, reason) in cases {
            assert_eq!(admit(facts), Err(reason), "{facts:?} refuses as {reason:?}");
        }
    }

    #[test]
    fn the_first_fact_that_fails_names_the_refusal() {
        let everything_wrong = Facts {
            permitted: false,
            holder: Holder::Another,
            sent_under: 1,
            lease: (DesktopControlState::Viewing, 2),
            retired: false,
        };
        assert_eq!(
            admit(everything_wrong),
            Err(Refusal::NotPermitted),
            "a peer that may not control hears that, not who holds control"
        );
        assert_eq!(
            admit(Facts {
                retired: true,
                ..everything_wrong
            }),
            Err(Refusal::Ended),
            "an ended desktop refuses before anything else"
        );
    }

    #[test]
    fn readmission_calls_any_change_but_an_end_a_changed_lease() {
        assert_eq!(
            readmit(Facts {
                holder: Holder::Another,
                ..facts()
            }),
            Err(Refusal::ControlChanged),
            "control moving between admission and the backend is a changed lease"
        );
        assert_eq!(
            readmit(Facts {
                retired: true,
                ..facts()
            }),
            Err(Refusal::Ended),
            "a desktop that ended meanwhile says so"
        );
        assert_eq!(
            readmit(facts()),
            admit(facts()),
            "nothing changed, the same lease"
        );
    }

    #[test]
    fn a_lease_holds_only_while_its_epoch_controls_an_open_desktop() {
        let lease = admit(facts()).expect("admitted");
        assert!(
            lease.holds((DesktopControlState::Controlling, 3), false),
            "the same epoch, controlling"
        );
        assert!(
            !lease.holds((DesktopControlState::Controlling, 4), false),
            "a newer lease is not this one"
        );
        assert!(
            !lease.holds((DesktopControlState::Viewing, 3), false),
            "yielded control is not held"
        );
        assert!(
            !lease.holds((DesktopControlState::Controlling, 3), true),
            "a closed desktop holds nothing"
        );
    }

    /// Applies every event and remembers them, for held-input tests.
    #[derive(Default)]
    struct Applied(Vec<crate::InputEvent>);
    impl crate::HostDesktop for Applied {
        fn apply(&mut self, event: crate::InputEvent) -> Result<(), Error> {
            self.0.push(event);
            Ok(())
        }
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
    }

    #[test]
    fn a_key_applied_as_its_lease_ended_is_released_with_everything_held() {
        let session = HostInputSession::new();
        let mut adapter = Applied::default();
        let a = crate::InputEvent::key_down(crate::input::usage::A);
        let b = crate::InputEvent::key_down(crate::input::usage::B);
        assert!(
            session.applied(a, true, &mut adapter, |_| panic!("nothing fails")),
            "the lease holds"
        );
        assert!(
            adapter.0.is_empty(),
            "applying is the caller's; noting releases nothing"
        );
        assert!(
            !session.applied(b, false, &mut adapter, |_| panic!("nothing fails")),
            "the lease ended"
        );
        assert_eq!(
            adapter.0,
            vec![
                crate::InputEvent::key_up(crate::input::usage::B),
                crate::InputEvent::key_up(crate::input::usage::A)
            ],
            "everything held is released, the latest first"
        );
        assert!(
            !session.settle(true, &mut adapter, |_| panic!("nothing fails")),
            "a lease that holds settles nothing"
        );
        assert_eq!(adapter.0.len(), 2, "nothing is held after the release");
    }

    #[test]
    fn text_typed_as_its_lease_ended_releases_what_was_held() {
        let session = HostInputSession::new();
        let mut adapter = Applied::default();
        session.hold(crate::InputEvent::key_down(crate::input::usage::LEFT_SHIFT));
        assert!(
            session.settle(false, &mut adapter, |_| panic!("nothing fails")),
            "the lease ended while the text was typed"
        );
        assert_eq!(
            adapter.0,
            vec![crate::InputEvent::key_up(crate::input::usage::LEFT_SHIFT)],
            "the held modifier is released"
        );
    }
}
