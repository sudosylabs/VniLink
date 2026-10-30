// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Local filesystem service for the file manager.
//!
//! Files shares the whole disk (`RootSet`): nothing is browsable until
//! the owner enables filesystem access, and a peer still needs Files
//! sharing for each operation, so no pairing silently widens into disk
//! access. A request's path is resolved with its directory links
//! followed, as a file manager follows them, when the request arrives; the
//! resolved folders are then opened one at a time through handles without
//! following a link, so a folder swapped for a link between the resolution
//! and the walk refuses the request. Folders made, renames and deletes act
//! through those handles on the entry itself, never a link's target.
//! Directory listings are paged and sorted. Rename is a true
//! same-directory filesystem rename with no-replace semantics and never
//! copies. Delete unlinks a file in place, or renames a folder aside and
//! removes that tree without following links. A move to the Trash checks
//! the entry like a delete but hands the system a path, which it resolves
//! again.
//!
//! A change names the listed entry by its [`EntryToken`], so a replacement
//! is refused as stale rather than changed. Where file ids are object
//! identities the token is that identity. Where they are entry positions
//! (FAT, exFAT, a Windows volume reporting no usable id; the volume decides,
//! see `vnilink_rename::VolumeIds`) an identity proves nothing, so the token
//! is a fingerprint of what the listing saw, and the rules tighten:
//!
//! - a file renamed, deleted or trashed, or a folder renamed, goes ahead
//!   while its fingerprint still matches the entry as read through its
//!   folder's handle at that moment; a file delete checks it once more on
//!   the object it removes;
//! - deleting or trashing a folder, or any change that arrives without a
//!   fingerprint, is refused as [`Error::UnprovenEntry`](crate::Error),
//!   since no fingerprint covers what a folder holds;
//! - on a macOS volume whose driver refuses an exclusive rename, a rename
//!   checks the new name is free and then takes it, so a file made under
//!   that name in the same instant is replaced.
//!
//! Elsewhere a change without a token is allowed: the token guards against a
//! stale listing, not against the peer.

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
