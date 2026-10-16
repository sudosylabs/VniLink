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
    SystemAudio,
    Microphone,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServicePhase {
    /// The parent connection has not completed admission.
    Waiting,
    Active,
    /// The connection dropped; old workers are cancelled, not replayed.
    Suspended,
    Closed(CloseCause),
    Failed(String),
}

/// Why a service record reached [`ServicePhase::Closed`]. Every close
/// carries one; there is no unknown close. Suspend/reconnect is not a
/// close: it parks the record in [`ServicePhase::Suspended`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    /// The local owner closed this service. Siblings and the parent live on.
    OwnerClose,
    /// The parent connection ended or never admitted this generation.
    ParentLoss,
    /// The peer revoked the grant or permission backing this service.
    Revoked,
    /// A terminal parent was observed with a newer epoch than this record.
    /// Establish and the power tick normally adopt the new epoch first, so
    /// this names a pre-adoption race observation, not a routine path.
    Replaced,
    /// The OS backend or adapter behind this service went away.
    DeviceLoss,
    /// The installation shut down.
    Shutdown,
    /// The remote end closed the service stream cleanly.
    PeerClose,
    /// An idle policy reclaimed this service while the parent lived on.
    IdleExpired,
}

impl ServicePhase {
    pub(crate) fn finished(&self) -> bool {
        matches!(self, Self::Closed(_) | Self::Failed(_))
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
    pub recording: bool,
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub system_actions: bool,
}

/// Local transport/backend support, not the remote computer's permission.
/// Remote operations still enforce their existing grants on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceSupport {
    pub desktop: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub system_actions: bool,
}

impl ServiceSupport {
    pub fn supports(self, kind: ServiceKind) -> bool {
        match kind {
            ServiceKind::Desktop => self.desktop,
            ServiceKind::Files => self.files,
            ServiceKind::Terminal => self.terminal,
            ServiceKind::Camera => self.camera,
            ServiceKind::SystemAudio => self.system_audio,
            ServiceKind::Microphone => self.microphone,
            ServiceKind::System => self.system_actions,
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

/// Cancellation and deadline for acquiring a native resource. Factories check
/// between OS calls; they must release partial resources before returning an
/// error. This cannot interrupt an OS call which itself blocks indefinitely.
#[derive(Clone)]
pub struct BackendStartup {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    parents: Vec<watch::Receiver<bool>>,
    consumers: Option<Vec<watch::Receiver<bool>>>,
    deadline: std::time::Instant,
}
impl BackendStartup {
    /// Standalone startup, for locally initiated backend work.
    pub fn new(timeout: std::time::Duration) -> Self {
        Self {
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            parents: Vec::new(),
            consumers: None,
            deadline: std::time::Instant::now() + timeout,
        }
    }
    pub(crate) fn scoped(
        parent: watch::Receiver<bool>,
        owner: watch::Receiver<bool>,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            parents: vec![parent, owner],
            consumers: None,
            deadline: std::time::Instant::now() + timeout,
        }
    }
    pub fn check(&self) -> Result<(), crate::Error> {
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire)
            || self
                .parents
                .iter()
                .any(|stop| *stop.borrow() || stop.has_changed().is_err())
            || self.consumers.as_ref().is_some_and(|owners| {
                owners
                    .iter()
                    .all(|stop| *stop.borrow() || stop.has_changed().is_err())
            })
        {
            return Err(crate::Error::NoSession);
        }
        if std::time::Instant::now() >= self.deadline {
            return Err(crate::Error::Timeout);
        }
        Ok(())
    }
    /// Local native startup tied to an explicit supervisor stop flag.
    pub fn with_stop(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Self {
        let mut startup = Self::new(timeout);
        startup.cancelled = stop;
        startup
    }
    pub(crate) fn for_consumers(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        owners: Vec<watch::Receiver<bool>>,
        timeout: std::time::Duration,
    ) -> Self {
        let mut startup = Self::with_stop(stop, timeout);
        startup.consumers = Some(owners);
        startup
    }
    pub(crate) fn guard(&self) -> StartupGuard {
        StartupGuard(self.cancelled.clone())
    }
    pub fn is_cancelled(&self) -> bool {
        self.check().is_err()
    }
    /// Wait for a native startup acknowledgement without hiding owner loss
    /// behind a multi-second channel timeout.
    pub fn receive<T>(&self, result: &std::sync::mpsc::Receiver<T>) -> Result<T, crate::Error> {
        loop {
            self.check()?;
            let remaining = self
                .deadline
                .saturating_duration_since(std::time::Instant::now());
            match result.recv_timeout(remaining.min(std::time::Duration::from_millis(20))) {
                Ok(value) => {
                    self.check()?;
                    return Ok(value);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(crate::Error::Unsupported("native startup acknowledgement"))
                }
            }
        }
    }
}

/// Dropping the admission future also cancels detached blocking work. Never
/// rely on dropping a spawn_blocking JoinHandle to interrupt a native factory.
pub(crate) struct StartupGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for StartupGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    #[test]
    fn shared_capture_startup_survives_one_consumer_but_never_adopts_replacements() {
        let (a, a_rx) = watch::channel(false);
        let (b, b_rx) = watch::channel(false);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let startup = BackendStartup::for_consumers(
            stop.clone(),
            vec![a_rx, b_rx],
            std::time::Duration::from_secs(5),
        );
        a.send_replace(true);
        assert!(startup.check().is_ok());
        drop(b);
        assert!(matches!(startup.check(), Err(crate::Error::NoSession)));
        let (_replacement, replacement_rx) = watch::channel(false);
        let fresh = BackendStartup::for_consumers(
            stop.clone(),
            vec![replacement_rx],
            std::time::Duration::from_secs(5),
        );
        assert!(fresh.check().is_ok());
        assert!(matches!(startup.check(), Err(crate::Error::NoSession)));
        stop.store(true, std::sync::atomic::Ordering::Release);
        assert!(matches!(fresh.check(), Err(crate::Error::NoSession)));
    }
    #[test]
    fn dropped_admission_signals_detached_worker_without_parent_close() {
        let (parent, parent_rx) = watch::channel(false);
        let (owner, owner_rx) = watch::channel(false);
        let startup =
            BackendStartup::scoped(parent_rx, owner_rx, std::time::Duration::from_secs(5));
        let worker = startup.clone();
        let guard = startup.guard();
        assert!(worker.check().is_ok());
        drop(guard);
        assert!(matches!(worker.check(), Err(crate::Error::NoSession)));
        assert!(!*parent.borrow() && !*owner.borrow());
    }
    #[test]
    fn startup_wait_expires_without_acknowledgement() {
        let startup = BackendStartup::new(std::time::Duration::from_millis(20));
        let (_tx, rx) = std::sync::mpsc::channel::<()>();
        assert!(matches!(startup.receive(&rx), Err(crate::Error::Timeout)));
    }
    #[test]
    fn expired_or_closed_startup_refuses_a_ready_resource_and_drops_it() {
        #[derive(Debug)]
        struct Resource(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        for expired in [true, false] {
            let (parent, parent_rx) = watch::channel(false);
            let (_owner, owner_rx) = watch::channel(false);
            let startup = BackendStartup::scoped(
                parent_rx,
                owner_rx,
                if expired {
                    std::time::Duration::ZERO
                } else {
                    std::time::Duration::from_secs(5)
                },
            );
            let (tx, rx) = std::sync::mpsc::channel();
            let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            tx.send(Resource(dropped.clone())).unwrap();
            if !expired {
                drop(parent);
            }
            assert!(matches!(
                startup.receive(&rx),
                Err(crate::Error::Timeout) | Err(crate::Error::NoSession)
            ));
            drop(rx);
            assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        }
    }
}

/// Installation-wide sharing consent for every paired device. OS consent and
/// backend availability remain independent checks. Enabling never starts work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharingPolicy {
    pub view: bool,
    pub control: bool,
    pub clipboard: bool,
    pub files: bool,
    pub terminal: bool,
    pub camera: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub recording: bool,
    pub system_actions: bool,
}

impl SharingPolicy {
    pub(crate) fn grants(self) -> crate::Grants {
        crate::Grants {
            view: self.view,
            control: self.control,
            clipboard: self.clipboard,
            transfer: self.files,
        }
    }

    pub(crate) fn permissions(self) -> ServicePermissions {
        ServicePermissions {
            desktop: self.view,
            files: self.files,
            terminal: self.terminal,
            camera: self.camera,
            system_audio: self.system_audio,
            microphone: self.microphone,
            recording: self.recording,
            system_actions: self.system_actions,
        }
    }

    pub(crate) fn bits(self) -> u16 {
        let fields = [
            self.view,
            self.control,
            self.clipboard,
            self.files,
            self.terminal,
            self.camera,
            self.system_audio,
            self.microphone,
            self.recording,
            self.system_actions,
        ];
        fields
            .into_iter()
            .enumerate()
            .fold(0, |bits, (index, allowed)| {
                bits | (u16::from(allowed) << index)
            })
    }

    pub(crate) fn from_bits(bits: u16) -> Result<Self, crate::Error> {
        if bits & !1023 != 0 {
            return Err(crate::Error::CorruptRecord);
        }
        Ok(Self {
            view: bits & 1 != 0,
            control: bits & 2 != 0,
            clipboard: bits & 4 != 0,
            files: bits & 8 != 0,
            terminal: bits & 16 != 0,
            camera: bits & 32 != 0,
            system_audio: bits & 64 != 0,
            microphone: bits & 128 != 0,
            recording: bits & 256 != 0,
            system_actions: bits & 512 != 0,
        })
    }
}

/// Atomic changes to global consent; omitted permissions retain their saved value.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharingPolicyUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub view: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub control: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub clipboard: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub files: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub terminal: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub camera: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub system_audio: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub microphone: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub recording: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "sharing_boolean"
    )]
    pub system_actions: Option<bool>,
}

impl SharingPolicyUpdate {
    pub(crate) fn apply(self, policy: &mut SharingPolicy) {
        if let Some(value) = self.view {
            policy.view = value;
        }
        if let Some(value) = self.control {
            policy.control = value;
        }
        if let Some(value) = self.clipboard {
            policy.clipboard = value;
        }
        if let Some(value) = self.files {
            policy.files = value;
        }
        if let Some(value) = self.terminal {
            policy.terminal = value;
        }
        if let Some(value) = self.camera {
            policy.camera = value;
        }
        if let Some(value) = self.system_audio {
            policy.system_audio = value;
        }
        if let Some(value) = self.microphone {
            policy.microphone = value;
        }
        if let Some(value) = self.recording {
            policy.recording = value;
        }
        if let Some(value) = self.system_actions {
            policy.system_actions = value;
        }
    }
}

fn sharing_boolean<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    <bool as serde::Deserialize>::deserialize(deserializer).map(Some)
}
