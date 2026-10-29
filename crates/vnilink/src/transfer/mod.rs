//! The file transfer protocol, `vnilink/files/1`.
//!
//! Jobs move file bytes over a connection of their own: the receiver dials
//! the sender under [`FILES_ALPN`], keeps the connection for the files that
//! follow and opens one stream per file. Control stays on `vnilink/1`.
//!
//! Neither side hashes ahead of time. A file moves in segments that are
//! subtrees of its BLAKE3 tree (`hash`): the sender reads a held source
//! once, hashing as it reads, and follows each segment with its chaining
//! value; the receiver checks each segment as it lands, syncs it and
//! journals its value beside the partial (`partial`). A bad segment costs
//! that segment again, not the file, and a resume continues after the last
//! journaled segment without either side reading earlier bytes again. The
//! bytes of the next segment that already arrived are kept and named in the
//! request, so an interrupted attempt costs only what was in flight.
//!
//! Through a relay the receiver holds its connection's receive window to
//! what the path carries, so a transfer yields to the desktop session
//! instead of queueing seconds ahead of it (`window`).
//!
//! This module owns the protocol: the wire format, sending a held source,
//! receiving into a partial. Which job and file a peer may read, where
//! partials live and when files publish belong to the core (`core/jobs`
//! and `core/transfer`).

mod counters;
mod hash;
mod partial;
mod receive;
mod send;
mod window;
mod wire;

pub(crate) use counters::TransferCounters;
#[cfg(test)]
pub(crate) use hash::SegmentHasher;
pub(crate) use hash::SEGMENT_LEN;
pub(crate) use partial::{Partial, NO_SPACE, PARTIAL_FAILED};
pub(crate) use receive::{ask, receive, Answer, Received};
#[cfg(feature = "fuzzing")]
pub(crate) use receive::{land, stream_of};
pub(crate) use send::{read_request, refuse, send, Source};
pub(crate) use window::{Window, INITIAL_WINDOW};
pub(crate) use wire::{FileRequest, FILES_ALPN, STATUS_BUSY, STATUS_CHANGED, STATUS_DENIED};

/// Bytes per read, per stream write and per stream read.
const CHUNK: usize = 256 * 1024;

/// How long a receiver waits for a byte before it gives the stream up as
/// stalled. Short in tests, which provoke stalls.
const STALL_WITHIN: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(500)
} else {
    std::time::Duration::from_secs(20)
};
