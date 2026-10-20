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

/// An entry's identity, used to refuse acting on a replaced item.
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferToken {
    dev: Option<String>,
    ino: Option<String>,
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
                    dev: entry.token.dev.map(|dev| dev.to_string()),
                    ino: entry.token.ino.map(|ino| ino.to_string()),
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
    Ok(vnilink_app::FfiEntryToken {
        dev: part(&token.dev)?,
        ino: part(&token.ino)?,
    })
}
