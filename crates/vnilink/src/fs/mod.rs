//! Local filesystem service for the file manager (Stage B).
//!
//! Owner-approved roots bound every operation: a relative path under one
//! root can never escape it, and a parent directory replaced by a symlink
//! between listing and mutation is refused. Directory listings are paged and
//! sorted. Rename is a true same-directory filesystem rename with
//! no-replace semantics and never copies. Delete unlinks a file in place,
//! or renames a folder aside and removes that tree without following links.
//!
//! The legacy single-file transfer directory is a separate setting with its
//! own grant history. These roots start empty: nothing is browsable until
//! the owner sets them, so no pairing or transfer grant silently widens
//! into whole-filesystem access.

mod delete;
mod entry;
mod list;
mod mkdir;
mod path;
mod rename;
mod roots;
mod wire;

pub(crate) use delete::{
    failure_message, finish_delete, stage_delete, DELETE_FAILED, DELETE_INCOMPLETE,
};
pub(crate) use list::{list_page, list_page_async, ListHooks};
pub(crate) use mkdir::create_dir;
pub(crate) use path::{check_name, check_rel};
pub(crate) use rename::rename_entry;
#[cfg(test)]
pub(crate) use rename::STALE_ENTRY;
pub(crate) use roots::{PinnedRoot, RootSet, ROOTS_REBOUND};
pub use wire::RevisionGate;
pub(crate) use wire::{
    decode_list_body, decode_request, decode_response, encode_list_body, encode_request,
    encode_response, Request, Response, STATUS_DENIED, STATUS_ERROR, STATUS_EXISTS, STATUS_STALE,
};

#[cfg(test)]
pub(crate) use entry::stat;
pub use entry::{EntryKind, EntryToken, FsEntry};
pub use list::{DirPage, MAX_LISTED, MAX_PAGE};
pub use roots::MAX_ROOTS;

/// Largest single name component, in bytes.
pub(crate) const MAX_COMPONENT_BYTES: usize = 255;
/// Largest relative path, in bytes.
pub(crate) const MAX_REL_BYTES: usize = 1024;
/// Largest depth of a relative path, in components.
pub(crate) const MAX_COMPONENTS: usize = 16;
