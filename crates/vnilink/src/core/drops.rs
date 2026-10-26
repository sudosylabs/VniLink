//! Losses a test asks for at named points of a desktop's stream, so a suite
//! can prove what the engine does with what never arrives: a picture or an
//! input lost where a real link, a full queue or a busy host loses one. A
//! shipped engine has no test's plan, and every point reads as kept.
use super::*;

/// Where on a desktop's stream a test may lose something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropPoint {
    /// Viewer: an input or typed text queued to send, before it leaves.
    Outbox,
    /// Host: a picture the send gate would admit, as if the link were behind.
    Gate,
    /// Viewer: one datagram of a picture, as if the network lost it.
    Datagram,
    /// Viewer: a whole picture as it reaches its queue.
    Inbox,
    /// Host: an input as it arrives, before it is admitted under its lease.
    BeforeAdmission,
    /// Host: an input admitted under its lease, before the backend takes it.
    AfterAdmission,
}

impl DropPoint {
    /// The point's name in what an engine records.
    pub fn label(self) -> &'static str {
        match self {
            Self::Outbox => "outbox",
            Self::Gate => "gate",
            Self::Datagram => "datagram",
            Self::Inbox => "inbox",
            Self::BeforeAdmission => "before admission",
            Self::AfterAdmission => "after admission",
        }
    }
}

/// A test's plan of what to lose: asked once for each thing that passes a
/// point, in order, and says whether to lose it.
#[cfg(any(test, feature = "test-probes"))]
pub trait Drops: Send + Sync {
    fn drops(&self, point: DropPoint) -> bool;
}

#[cfg(any(test, feature = "test-probes"))]
impl Core {
    /// Loses what `drops` says at each [`DropPoint`] from now on; `None`
    /// keeps everything again.
    pub fn set_drops(&self, drops: Option<Arc<dyn Drops>>) {
        let _engine = self.engine();
        *lock(&self.shared.drops) = drops;
    }
}

/// Whether the test's plan loses what passes `point` now. Always false in a
/// shipped engine.
#[cfg(any(test, feature = "test-probes"))]
pub(super) fn dropped(shared: &Shared, point: DropPoint) -> bool {
    let plan = lock(&shared.drops).clone();
    plan.is_some_and(|plan| plan.drops(point))
}

#[cfg(not(any(test, feature = "test-probes")))]
#[inline(always)]
pub(super) fn dropped(_shared: &Shared, _point: DropPoint) -> bool {
    false
}

/// Records an input a test's plan lost on `service`: its kind and where,
/// never its keys, text or position.
pub(super) fn input_dropped(service: u64, kind: &'static str, point: DropPoint) {
    tracing::debug!(
        target: crate::trace::INPUT,
        service,
        kind,
        at = point.label(),
        "input dropped for a test"
    );
}
