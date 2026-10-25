//! What VniLink records about itself, for its log file and the flight
//! recorder that Export Diagnostics includes, so a session can be explained
//! after it went wrong.
//!
//! Events go through `tracing`, each under one of the targets here, which
//! name its area. An event that belongs to a desktop service carries its id
//! as the field `service`: the recorder keeps each service's recent events
//! apart, so a busy session elsewhere does not push them out.
//!
//! Each engine runs in a span named `engine` whose field `name` says which
//! engine it is (`engine` in the application, which has one; `host` and
//! `controller` in tests, which run two in one process). Its runtime enters
//! it on every thread, and threads it starts run in it ([`within`]), so
//! every event recorded there says which engine recorded it. A public call
//! that records on its caller's thread enters it too.
//!
//! The filter (`vnilink-log`) chooses areas by these targets, for example
//! `vnilink=info,vnilink::files=debug`. Spans name what an event belongs
//! to: a session, a service and its generation, a job or an operation.
//!
//! A record never holds what a person sees or types: no screen pixels,
//! typed text, key identities (usages or characters), pointer positions,
//! clipboard or terminal content, secrets, tokens or absolute paths, and a
//! peer only as [`crate::diagnostics::fingerprint`]. Counts, sizes, timings,
//! codecs, roles and reasons are what it holds.

#[doc(hidden)]
pub use tracing as __tracing;

/// Records a failure this code chose not to act on, instead of `let _ =`
/// or `.ok()`: `ignored!(trace::FILES, "the viewer already left", reply.send(x))`.
/// The record names the area (a target here), why ignoring it is right,
/// and what failed by name only ([`failure`]), never its text; and it is
/// counted ([`ignored_counts`]). It evaluates to the result's `Ok` value
/// as an `Option`.
#[macro_export]
macro_rules! ignored {
    ($target:expr, $why:literal, $result:expr $(,)?) => {
        match $result {
            Ok(value) => Some(value),
            Err(error) => {
                #[allow(unused_imports)]
                use $crate::trace::{ByFailure as _, ByType as _};
                let named = &$crate::trace::Named(&error);
                $crate::trace::__tracing::debug!(
                    target: $target,
                    why = $why,
                    failure = named.failure_name(),
                    reason = named.failure_reason(),
                    "ignored"
                );
                $crate::trace::count_ignored($target, $why);
                None
            }
        }
    };
}

/// A failure that names itself for records: by a fixed name and, where it
/// has one, a fixed product reason; never by text a library or the system
/// wrote. VniLink's error types implement it.
pub trait Failure {
    fn failure_name(&self) -> &'static str;
    fn failure_reason(&self) -> Option<&'static str> {
        None
    }
}

impl Failure for crate::Error {
    fn failure_name(&self) -> &'static str {
        self.name()
    }
    fn failure_reason(&self) -> Option<&'static str> {
        self.reason()
    }
}

#[doc(hidden)]
pub struct Named<'a, T>(pub &'a T);

/// [`ignored!`] names a failure by [`Failure`] when its type has it…
#[doc(hidden)]
pub trait ByFailure {
    fn failure_name(&self) -> &'static str;
    fn failure_reason(&self) -> Option<&'static str>;
}
impl<T: Failure> ByFailure for Named<'_, T> {
    fn failure_name(&self) -> &'static str {
        self.0.failure_name()
    }
    fn failure_reason(&self) -> Option<&'static str> {
        self.0.failure_reason()
    }
}

/// …and otherwise by [`failure`].
#[doc(hidden)]
pub trait ByType {
    fn failure_name(&self) -> &'static str;
    fn failure_reason(&self) -> Option<&'static str>;
}
impl<T: 'static> ByType for &Named<'_, T> {
    fn failure_name(&self) -> &'static str {
        failure(self.0)
    }
    fn failure_reason(&self) -> Option<&'static str> {
        None
    }
}

/// What failed, by name: a VniLink [`crate::Error`]'s [`crate::Error::name`],
/// otherwise the failure's type, such as `SendError` or `Elapsed`. Never
/// its text, which can carry a library's addresses or a path.
pub fn failure<E: 'static>(error: &E) -> &'static str {
    if let Some(error) = (error as &dyn std::any::Any).downcast_ref::<crate::Error>() {
        return error.name();
    }
    if let Some(error) = (error as &dyn std::any::Any).downcast_ref::<std::io::Error>() {
        return io_failure(error.kind());
    }
    let full = std::any::type_name::<E>();
    let outer = full.split('<').next().unwrap_or(full);
    outer.rsplit("::").next().unwrap_or(outer)
}

/// An I/O failure by its kind.
pub fn io_failure(kind: std::io::ErrorKind) -> &'static str {
    use std::io::ErrorKind;
    match kind {
        ErrorKind::NotFound => "not found",
        ErrorKind::PermissionDenied => "permission denied",
        ErrorKind::AlreadyExists => "already exists",
        ErrorKind::WouldBlock => "would block",
        ErrorKind::InvalidInput => "invalid input",
        ErrorKind::InvalidData => "invalid data",
        ErrorKind::TimedOut => "timed out",
        ErrorKind::Interrupted => "interrupted",
        ErrorKind::UnexpectedEof => "unexpected end",
        ErrorKind::BrokenPipe => "broken pipe",
        ErrorKind::ConnectionReset => "connection reset",
        ErrorKind::ConnectionRefused => "connection refused",
        ErrorKind::StorageFull => "storage full",
        ErrorKind::ReadOnlyFilesystem => "read-only file system",
        ErrorKind::CrossesDevices => "crosses devices",
        ErrorKind::DirectoryNotEmpty => "directory not empty",
        ErrorKind::IsADirectory => "is a directory",
        ErrorKind::NotADirectory => "not a directory",
        ErrorKind::Unsupported => "unsupported",
        _ => "other I/O",
    }
}

fn ignored_map(
) -> &'static std::sync::Mutex<std::collections::BTreeMap<(&'static str, &'static str), u64>> {
    static IGNORED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<(&'static str, &'static str), u64>>,
    > = std::sync::OnceLock::new();
    IGNORED.get_or_init(Default::default)
}

#[doc(hidden)]
pub fn count_ignored(target: &'static str, why: &'static str) {
    *ignored_map()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .entry((target, why))
        .or_default() += 1;
}

/// How many failures each area ignored, by reason, in this process.
pub fn ignored_counts() -> Vec<(&'static str, &'static str, u64)> {
    ignored_map()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .map(|((target, why), count)| (*target, *why, *count))
        .collect()
}

/// The span that names an engine; see the module documentation.
pub const ENGINE: &str = "vnilink::engine";

/// A new engine's span, named `engine` until [`crate::Core::set_trace_name`].
pub(crate) fn engine_span() -> tracing::Span {
    tracing::error_span!(target: ENGINE, "engine", name = "engine")
}

/// `work`, to run on another thread inside `span`.
pub fn within<T>(span: tracing::Span, work: impl FnOnce() -> T) -> impl FnOnce() -> T {
    move || span.in_scope(work)
}

// Connections and their services.
/// A session's connection, its services and how each ended.
pub const SESSION: &str = "vnilink::session";
/// Serving a connected peer: what it asked for and what was refused.
pub const HOSTING: &str = "vnilink::hosting";
/// A service's lifecycle: opened, its generation, closed and why.
pub const SERVICE: &str = "vnilink::service";

// The remote desktop.
/// The desktop picture stream: encoders, quality, pictures sent and received.
pub const STREAM: &str = "vnilink::stream";
/// Still pictures: asked for, made, discarded and sent.
pub const STILL: &str = "vnilink::still";
/// Screen capture on the host: pictures delivered, dropped and what changed.
pub const CAPTURE: &str = "vnilink::capture";
/// Encoders: chosen, opened, their rate and failures.
pub const ENCODE: &str = "vnilink::encode";
/// Decoders: chosen, opened and failures.
pub const DECODE: &str = "vnilink::decode";
/// Displays: their layout and its changes.
pub const TOPOLOGY: &str = "vnilink::topology";
/// Remote input: what was applied, refused or failed, and how long it took.
pub const INPUT: &str = "vnilink::input";

// The other services.
/// File transfers and their jobs: phases, resumes, conflicts and failures.
pub const FILES: &str = "vnilink::files";
/// Remote terminals: opened, resized, closed, never their content.
pub const TERMINAL: &str = "vnilink::terminal";
/// Camera streams.
pub const CAMERA: &str = "vnilink::camera";
/// Audio streams: system sound and microphones.
pub const AUDIO: &str = "vnilink::audio";
/// The shared clipboard: offers and transfers, never their content.
pub const CLIPBOARD: &str = "vnilink::clipboard";
/// System actions: lock, privacy and the like.
pub const SYSTEM: &str = "vnilink::system";
/// Power: keeping the computer awake, sleep and wake.
pub const POWER: &str = "vnilink::power";
/// Screenshots.
pub const SNAPSHOT: &str = "vnilink::snapshot";
/// Session recordings.
pub const RECORDING: &str = "vnilink::recording";

// The installation.
/// Pairing: invitations, confirmations and revocations.
pub const PAIRING: &str = "vnilink::pairing";
/// Relays and presence: which peers are reachable, and how.
pub const RELAY: &str = "vnilink::relay";
/// The encrypted store.
pub const STORAGE: &str = "vnilink::storage";
/// The application's commands to the engine, and their failures.
pub const BRIDGE: &str = "vnilink::bridge";
/// The log itself: its filter and its file.
pub const LOG: &str = "vnilink::log";

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure is named by what it is, never by its text.
    #[test]
    fn a_failure_is_named_without_its_text() {
        assert_eq!(
            failure(&crate::Error::Network("192.0.2.1:4433 refused".into())),
            "network"
        );
        let (sender, receiver) = std::sync::mpsc::channel::<u8>();
        drop(receiver);
        let sent = sender.send(7).unwrap_err();
        assert_eq!(failure(&sent), "SendError");
        let missing = std::fs::read("/no/such/file/for/vnilink").unwrap_err();
        assert_eq!(failure(&missing), "not found");
    }

    /// An ignored failure is counted under its area and reason, and the
    /// value of a success passes through.
    #[test]
    fn an_ignored_failure_is_counted_and_a_success_passes() {
        let before = |why| {
            ignored_counts()
                .into_iter()
                .find(|(target, reason, _)| *target == FILES && *reason == why)
                .map_or(0, |(_, _, count)| count)
        };
        let earlier = before("the test ignores it");
        assert_eq!(
            crate::ignored!(FILES, "the test ignores it", Ok::<u8, crate::Error>(3)),
            Some(3)
        );
        assert_eq!(
            crate::ignored!(
                FILES,
                "the test ignores it",
                Err::<u8, _>(crate::Error::Busy)
            ),
            None
        );
        // A type without `Failure` is named by its type.
        assert_eq!(
            crate::ignored!(FILES, "the test ignores it", "x".parse::<u8>()),
            None
        );
        assert_eq!(before("the test ignores it"), earlier + 2);
    }
}
