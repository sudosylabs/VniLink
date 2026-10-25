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

/// The span that names an engine; see the module documentation.
pub const ENGINE: &str = "vnilink::engine";

/// A new engine's span, named `engine` until [`crate::Core::set_trace_name`].
pub(crate) fn engine_span() -> tracing::Span {
    tracing::error_span!(target: ENGINE, "engine", name = "engine")
}

/// `work`, to run on another thread inside `span`.
pub(crate) fn within<T>(span: tracing::Span, work: impl FnOnce() -> T) -> impl FnOnce() -> T {
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
