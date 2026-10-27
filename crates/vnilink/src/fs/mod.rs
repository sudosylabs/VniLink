//! Local filesystem service for the file manager.
//!
//! Files shares the whole disk ([`RootSet`]): nothing is browsable until
//! the owner enables filesystem access, and a peer still needs Files
//! sharing for each operation, so no pairing silently widens into disk
//! access. A request's path is resolved with its directory links
//! followed, as a file manager follows them, when the request arrives; the
//! resolved folders are then opened one at a time through handles without
//! following a link, so a folder swapped for a link between the resolution
//! and the walk refuses the request. Folders made, renames and deletes act
//! through those handles on the entry itself, never a link's target, and
//! name the listed entry by its token, so a replacement is refused rather
//! than changed: its identity, or on a volume whose ids are entry
//! positions (FAT, exFAT) a fingerprint of what the listing saw, where
//! deleting a folder is refused outright (`entry.rs`). A move to the
//! Trash checks the entry the same way but hands the system a path, which
//! it resolves again. Directory listings are paged and sorted. Rename is a true same-directory filesystem rename
//! with no-replace semantics and never copies. Delete unlinks a file in
//! place, or renames a folder aside and removes that tree without
//! following links.
//!
//! The legacy single-file transfer directory is a separate setting with its
//! own grant history.

mod delete;
mod entry;
mod list;
mod mkdir;
mod path;
mod places;
mod rename;
mod roots;
mod trash;
mod wire;

pub(crate) use delete::{
    failure_message, finish_delete, stage_delete, DeleteWork, DELETE_FAILED, DELETE_INCOMPLETE,
};
pub(crate) use list::{list_page, list_page_async, ListHooks};
pub(crate) use mkdir::create_dir;
pub(crate) use path::{check_name, check_rel, is_engine_temp};
pub(crate) use places::places;
pub use places::{FolderKind, Place, PlaceRoot, Places};
pub(crate) use rename::rename_entry;
pub(crate) use roots::{PinnedRoot, RootSet, DRIVE_CHANGED};
pub(crate) use trash::{finish_trash, stage_trash, NO_TRASH};
pub use wire::RevisionGate;
pub(crate) use wire::{
    decode_list_body, decode_places_body, decode_request, decode_response, encode_list_body,
    encode_places_body, encode_request, encode_response, Request, Response, STATUS_DENIED,
    STATUS_ERROR, STATUS_EXISTS, STATUS_NO_TRASH, STATUS_STALE, STATUS_UNPROVEN,
};

#[cfg(test)]
pub(crate) use entry::stat;
pub use entry::{EntryKind, EntryToken, FsEntry, TokenKind};
pub use list::{DirPage, MAX_LISTED, MAX_PAGE};
pub use roots::MAX_ROOTS;

/// Largest single name component, in bytes.
pub(crate) const MAX_COMPONENT_BYTES: usize = 255;
/// Largest relative path, in bytes: Linux's `PATH_MAX`, above macOS's and
/// Windows' long-path limits, so any path the disk holds is reachable.
pub(crate) const MAX_REL_BYTES: usize = 4096;
/// Largest depth of a relative path, in components.
pub(crate) const MAX_COMPONENTS: usize = 64;
