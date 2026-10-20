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
//! journaled segment without either side reading earlier bytes again.
//!
//! This module owns the protocol: the wire format, sending a held source,
//! receiving into a partial. Which job and file a peer may read, where
//! partials live and when files publish belong to the core (`core/jobs`
//! and `core/transfer`).

mod hash;
mod partial;
mod receive;
mod send;
mod wire;

#[cfg(test)]
pub(crate) use hash::{SegmentHasher, SEGMENT_LEN};
pub(crate) use partial::Partial;
pub(crate) use receive::{ask, receive, Answer, Received};
pub(crate) use send::{read_request, refuse, send, Source};
pub(crate) use wire::{FileRequest, FILES_ALPN, STATUS_CHANGED, STATUS_DENIED};

/// Bytes per read, per stream write and per stream read.
const CHUNK: usize = 256 * 1024;
