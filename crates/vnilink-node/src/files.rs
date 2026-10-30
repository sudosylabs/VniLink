// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Files shapes as they cross to JavaScript, shared by the local-disk
//! commands and the remote `files` channel. Every 64-bit count and id is a
//! decimal string, so nothing past 2^53 loses precision on the way.

use napi::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct TransferPlaceRoot {
    id: String,
    name: String,
}

#[derive(Serialize)]
pub(crate) struct TransferPlace {
    kind: &'static str,
    root: String,
    path: String,
}

#[derive(Serialize)]
pub(crate) struct TransferPlaces {
    roots: Vec<TransferPlaceRoot>,
    folders: Vec<TransferPlace>,
}

/// What a listed entry is proven by when it is changed.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TransferTokenKind {
    Identity,
    /// On a volume whose ids are entry positions: `dev` names the volume
    /// and `fingerprint` digests what the listing saw.
    Fingerprint,
}

/// The token a listing gives an entry, used to refuse acting on a
/// replaced item.
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferToken {
    kind: TransferTokenKind,
    dev: Option<String>,
    ino: Option<String>,
    /// Creation time in nanoseconds since 1970, where the filesystem keeps one.
    born: Option<String>,
    fingerprint: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransferEntry {
    name: String,
    kind: &'static str,
    size: String,
    mtime_ms: String,
    token: TransferToken,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TransferDirPage {
    entries: Vec<TransferEntry>,
    total: String,
    next_offset: Option<String>,
    truncated: bool,
    /// Names that are not valid UTF-8, left out of the listing.
    skipped: String,
}

pub(crate) fn transfer_places(places: vnilink_app::FfiPlaces) -> TransferPlaces {
    TransferPlaces {
        roots: places
            .roots
            .into_iter()
            .map(|root| TransferPlaceRoot {
                id: root.id.to_string(),
                name: root.name,
            })
            .collect(),
        folders: places
            .folders
            .into_iter()
            .map(|folder| TransferPlace {
                kind: match folder.kind {
                    vnilink_app::FfiFolderKind::Home => "home",
                    vnilink_app::FfiFolderKind::Desktop => "desktop",
                    vnilink_app::FfiFolderKind::Documents => "documents",
                    vnilink_app::FfiFolderKind::Downloads => "downloads",
                    vnilink_app::FfiFolderKind::Pictures => "pictures",
                    vnilink_app::FfiFolderKind::Volume => "volume",
                },
                root: folder.root.to_string(),
                path: folder.path,
            })
            .collect(),
    }
}

pub(crate) fn transfer_page(page: vnilink_app::FfiDirPage) -> TransferDirPage {
    TransferDirPage {
        entries: page
            .entries
            .into_iter()
            .map(|entry| TransferEntry {
                kind: match entry.kind {
                    vnilink_app::FfiEntryKind::File => "file",
                    vnilink_app::FfiEntryKind::Dir => "dir",
                    vnilink_app::FfiEntryKind::Symlink => "symlink",
                    vnilink_app::FfiEntryKind::Other => "other",
                },
                name: entry.name,
                size: entry.size.to_string(),
                mtime_ms: entry.mtime_ms.to_string(),
                token: TransferToken {
                    kind: match entry.token.kind {
                        vnilink_app::FfiTokenKind::Identity => TransferTokenKind::Identity,
                        vnilink_app::FfiTokenKind::Fingerprint => TransferTokenKind::Fingerprint,
                    },
                    dev: entry.token.dev.map(|dev| dev.to_string()),
                    ino: entry.token.ino.map(|ino| ino.to_string()),
                    born: entry.token.born.map(|born| born.to_string()),
                    fingerprint: entry.token.fingerprint.map(|digest| digest.to_string()),
                },
            })
            .collect(),
        total: page.total.to_string(),
        next_offset: page.next_offset.map(|offset| offset.to_string()),
        truncated: page.truncated,
        skipped: page.non_utf8_skipped.to_string(),
    }
}

/// The identity a listing reported, back from JavaScript.
pub(crate) fn entry_token(token: &TransferToken) -> Result<vnilink_app::FfiEntryToken> {
    let part = |value: &Option<String>| {
        value
            .as_deref()
            .map(|text| {
                text.parse::<u64>()
                    .map_err(|_| Error::from_reason("Invalid entry token"))
            })
            .transpose()
    };
    let (kind, valid) = match token.kind {
        TransferTokenKind::Identity => (
            vnilink_app::FfiTokenKind::Identity,
            token.fingerprint.is_none(),
        ),
        TransferTokenKind::Fingerprint => (
            vnilink_app::FfiTokenKind::Fingerprint,
            token.ino.is_none() && token.born.is_none(),
        ),
    };
    if !valid {
        return Err(Error::from_reason("Invalid entry token"));
    }
    Ok(vnilink_app::FfiEntryToken {
        kind,
        dev: part(&token.dev)?,
        ino: part(&token.ino)?,
        born: part(&token.born)?,
        fingerprint: part(&token.fingerprint)?,
    })
}
