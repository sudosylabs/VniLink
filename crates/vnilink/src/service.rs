//! Local service ownership, separate from the connection to a computer.
//!
//! These handles do not grant remote permissions or start a backend. They
//! provide cancellation and generation fencing for backend tasks. Existing
//! session APIs remain the transport layer during migration.

use tokio::sync::watch;

/// A feature that can own work on a peer connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Desktop,
    Files,
    Terminal,
    Camera,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServicePhase {
    /// The parent connection has not completed admission.
    Waiting,
    Active,
    /// The connection dropped; old workers are cancelled, not replayed.
    Suspended,
    Closed,
    Failed(String),
}

impl ServicePhase {
    pub(crate) fn finished(&self) -> bool {
        matches!(self, Self::Closed | Self::Failed(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    /// Local service identity. Not a protocol/session identity.
    pub id: u64,
    pub connection_id: u64,
    /// Parent connection generation. Always pass this back with actions.
    pub generation: u64,
    pub peer_id: String,
    pub kind: ServiceKind,
    pub phase: ServicePhase,
}

/// This installation's directional permissions for a paired computer.
/// Backend support is reported separately by `ServiceSupport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ServicePermissions {
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
}

/// Local transport/backend support, not the remote computer's permission.
/// Remote operations still enforce their existing grants on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSupport {
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
}

impl ServiceSupport {
    pub fn supports(self, kind: ServiceKind) -> bool {
        match kind {
            ServiceKind::Desktop => self.desktop,
            ServiceKind::Files => self.files,
            ServiceKind::Terminal => self.terminal,
            ServiceKind::Camera => self.camera,
        }
    }
}

/// Cancellation for one service on one connection generation.
///
/// Workers select `cancelled()` alongside their I/O. Closing a service,
/// stopping the parent, or reconnecting cancels all leases for that epoch.
/// Reconnection creates a fresh lease; an old one never becomes usable again.
/// Dropping a lease does not close the service or the parent connection.
#[derive(Clone)]
pub struct ServiceLease {
    pub(crate) info: ServiceInfo,
    pub(crate) stop: watch::Receiver<bool>,
}

impl ServiceLease {
    pub fn info(&self) -> &ServiceInfo {
        &self.info
    }

    pub fn is_cancelled(&self) -> bool {
        *self.stop.borrow() || self.stop.has_changed().is_err()
    }

    pub async fn cancelled(&mut self) {
        loop {
            if self.is_cancelled() || self.stop.changed().await.is_err() {
                return;
            }
        }
    }

    /// Runs cancellable backend I/O. Cancellation wins when both branches are
    /// ready, drops the future (and its owned streams), and refuses a result
    /// completed after close. Already committed remote mutations are not undone.
    pub async fn run<T>(
        &mut self,
        work: impl std::future::Future<Output = Result<T, crate::Error>>,
    ) -> Result<T, crate::Error> {
        let result = tokio::select! {
            biased;
            _ = self.cancelled() => return Err(crate::Error::NoSession),
            result = work => result,
        };
        if self.is_cancelled() {
            return Err(crate::Error::NoSession);
        }
        result
    }
}

/// Includes closed records until explicitly forgotten; bounds UI retention.
pub const MAX_SERVICE_RECORDS: usize = 256;
