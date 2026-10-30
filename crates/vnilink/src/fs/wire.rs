// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Versioned filesystem request/response codec and the caller-side
//! revision gate.
//!
//! Every request carries a protocol version and a caller-chosen id; every
//! response echoes both, so a delayed reply can never be mistaken for the
//! current navigation. Native panes additionally track their own navigation
//! revision with [`RevisionGate`]: only the newest outstanding listing may
//! paint.
//!
//! Bounds: root ids, path/name lengths and page limits are capped here, so
//! a malformed peer fails decoding instead of reaching the filesystem.

use crate::error::Error;

use super::entry::{EntryKind, EntryToken};
use super::list::{DirPage, MAX_PAGE};
use super::places::{FolderKind, Place, PlaceRoot, Places};
use super::{MAX_COMPONENT_BYTES, MAX_REL_BYTES};

/// Filesystem wire version. Bump on any incompatible change; peers reject
/// what they do not speak.
pub const FS_VERSION: u8 = 1;

/// A request holds one path and at most two names.
const MAX_WIRE_BYTES: usize = 8 * 1024;
const TAG_LIST: u8 = 1;
const TAG_MKDIR: u8 = 2;
const TAG_RENAME: u8 = 3;
const TAG_DELETE: u8 = 4;
const TAG_PLACES: u8 = 5;
const TAG_WATCH: u8 = 6;

const STATUS_OK: u8 = 0;
/// Machine-readable response codes. The message line stays human-only.
pub(crate) const STATUS_EXISTS: u8 = 1;
pub(crate) const STATUS_DENIED: u8 = 2;
pub(crate) const STATUS_STALE: u8 = 3;
pub(crate) const STATUS_ERROR: u8 = 4;
/// A delete to the Trash that the place has no Trash for.
pub(crate) const STATUS_NO_TRASH: u8 = 5;
/// The drive cannot prove the change acts on the listed entry.
pub(crate) const STATUS_UNPROVEN: u8 = 6;

/// A decoded request. Response payloads are rendered by the transport into
/// [`Response`]; only the status line is decoded here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    List {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        offset: u64,
        limit: u16,
    },
    Mkdir {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        name: String,
    },
    Rename {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        name: String,
        expected: Option<EntryToken>,
        new_name: String,
    },
    /// Deletes an entry: to the Trash when `trash`, else permanently.
    Delete {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        name: String,
        expected: Option<EntryToken>,
        trash: bool,
    },
    /// The host's roots, home folder and usual folders.
    Places { id: u32, generation: u64 },
    /// Answers with the folder's fingerprint once it differs from `since`,
    /// or after a while if it does not change: a wait the controller makes
    /// on a stream of its own, so the folder its pane shows stays current.
    Watch {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        since: u64,
    },
}

/// A decoded response header. Entry payloads follow on the accepted stream
/// and are framed by the transport, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub id: u32,
    pub status: u8,
    pub message: String,
}

impl Response {
    pub fn ok(id: u32) -> Self {
        Self {
            id,
            status: STATUS_OK,
            message: String::new(),
        }
    }

    pub fn coded(id: u32, status: u8) -> Self {
        Self {
            id,
            status,
            message: String::new(),
        }
    }

    #[cfg(test)]
    pub fn is_ok(&self) -> bool {
        self.status == STATUS_OK
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(&'static str);

pub(crate) fn encode_request(request: &Request) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.push(FS_VERSION);
    match request {
        Request::List {
            id,
            generation,
            root,
            path,
            offset,
            limit,
        } => {
            if *limit == 0 || *limit > MAX_PAGE {
                return Err(Error::InvalidSetting("the page limit is out of range"));
            }
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_LIST);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_u64(&mut bytes, *offset);
            push_u16(&mut bytes, *limit);
        }
        Request::Mkdir {
            id,
            generation,
            root,
            path,
            name,
        } => {
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_MKDIR);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_text(&mut bytes, name, MAX_COMPONENT_BYTES)?;
        }
        Request::Rename {
            id,
            generation,
            root,
            path,
            name,
            expected,
            new_name,
        } => {
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_RENAME);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_text(&mut bytes, name, MAX_COMPONENT_BYTES)?;
            push_expected(&mut bytes, *expected);
            push_text(&mut bytes, new_name, MAX_COMPONENT_BYTES)?;
        }
        Request::Delete {
            id,
            generation,
            root,
            path,
            name,
            expected,
            trash,
        } => {
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_DELETE);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_text(&mut bytes, name, MAX_COMPONENT_BYTES)?;
            push_expected(&mut bytes, *expected);
            bytes.push(u8::from(*trash));
        }
        Request::Watch {
            id,
            generation,
            root,
            path,
            since,
        } => {
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_WATCH);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_u64(&mut bytes, *since);
        }
        Request::Places { id, generation } => {
            // The common root and path fields stay, empty, so every request
            // shares one header.
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_PLACES);
            push_u64(&mut bytes, 0);
            push_text(&mut bytes, "", MAX_REL_BYTES)?;
        }
    }
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(Error::InvalidSetting("the filesystem request is too large"));
    }
    Ok(bytes)
}

pub(crate) fn decode_request(bytes: &[u8]) -> Result<Request, WireError> {
    let error = |message: &'static str| WireError(message);
    if bytes.len() > MAX_WIRE_BYTES || bytes.len() < 24 {
        return Err(error("the filesystem frame is malformed"));
    }
    if bytes[0] != FS_VERSION {
        return Err(error("the filesystem version is not supported"));
    }
    let id = u32::from_be_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| error("the filesystem frame is malformed"))?,
    );
    let generation = u64::from_be_bytes(
        bytes[5..13]
            .try_into()
            .map_err(|_| error("the filesystem frame is malformed"))?,
    );
    let tag = bytes[13];
    let mut cursor = &bytes[14..];
    let root = take_u64(&mut cursor).ok_or(error("the filesystem frame is malformed"))?;
    let path =
        take_text(&mut cursor, MAX_REL_BYTES).ok_or(error("the filesystem frame is malformed"))?;
    match tag {
        TAG_LIST => {
            let offset = take_u64(&mut cursor).ok_or(error("the filesystem frame is malformed"))?;
            let limit = take_u16(&mut cursor).ok_or(error("the filesystem frame is malformed"))?;
            if limit == 0 || limit > MAX_PAGE || !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::List {
                id,
                generation,
                root,
                path,
                offset,
                limit,
            })
        }
        TAG_MKDIR => {
            let name = take_text(&mut cursor, MAX_COMPONENT_BYTES)
                .ok_or(error("the filesystem frame is malformed"))?;
            if !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::Mkdir {
                id,
                generation,
                root,
                path,
                name,
            })
        }
        TAG_RENAME => {
            let name = take_text(&mut cursor, MAX_COMPONENT_BYTES)
                .ok_or(error("the filesystem frame is malformed"))?;
            let expected = take_expected(&mut cursor)?;
            let new_name = take_text(&mut cursor, MAX_COMPONENT_BYTES)
                .ok_or(error("the filesystem frame is malformed"))?;
            if !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::Rename {
                id,
                generation,
                root,
                path,
                name,
                expected,
                new_name,
            })
        }
        TAG_DELETE => {
            let name = take_text(&mut cursor, MAX_COMPONENT_BYTES)
                .ok_or(error("the filesystem frame is malformed"))?;
            let expected = take_expected(&mut cursor)?;
            let trash = match take_byte(&mut cursor) {
                Some(0) => false,
                Some(1) => true,
                _ => return Err(error("the filesystem frame is malformed")),
            };
            if !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::Delete {
                id,
                generation,
                root,
                path,
                name,
                expected,
                trash,
            })
        }
        TAG_WATCH => {
            let since = take_u64(&mut cursor).ok_or(error("the filesystem frame is malformed"))?;
            if !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::Watch {
                id,
                generation,
                root,
                path,
                since,
            })
        }
        TAG_PLACES => {
            if root != 0 || !path.is_empty() || !cursor.is_empty() {
                return Err(error("the filesystem frame is malformed"));
            }
            Ok(Request::Places { id, generation })
        }
        _ => Err(error("the filesystem frame is malformed")),
    }
}

pub(crate) fn encode_response(response: &Response) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.push(FS_VERSION);
    push_u32(&mut bytes, response.id);
    bytes.push(response.status);
    let message = response.message.as_bytes();
    let len = message.len().min(255);
    bytes.push(len as u8);
    bytes.extend_from_slice(&message[..len]);
    bytes
}

pub(crate) fn decode_response(bytes: &[u8]) -> Result<Response, WireError> {
    let error = |message: &'static str| WireError(message);
    if bytes.len() < 7 || bytes[0] != FS_VERSION {
        return Err(error("the filesystem frame is malformed"));
    }
    let id = u32::from_be_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| error("the filesystem frame is malformed"))?,
    );
    let status = bytes[5];
    let len = bytes[6] as usize;
    if bytes.len() != 7 + len {
        return Err(error("the filesystem frame is malformed"));
    }
    let message =
        std::str::from_utf8(&bytes[7..]).map_err(|_| error("the filesystem frame is malformed"))?;
    Ok(Response {
        id,
        status,
        message: message.to_owned(),
    })
}

impl WireError {
    pub fn to_error(&self) -> Error {
        Error::Malformed
    }
}

/// Encodes one listing page as the body frame following an OK list
/// response. Names are capped at one component each; the whole body stays
/// bounded by the page limit.
pub(crate) fn encode_list_body(page: &DirPage) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    push_u64(&mut bytes, page.total);
    push_u64(&mut bytes, page.next_offset.unwrap_or(u64::MAX));
    bytes.push(u8::from(page.truncated));
    push_u64(&mut bytes, page.non_utf8_skipped);
    if page.entries.len() > MAX_PAGE as usize {
        return Err(Error::InvalidSetting("the listing page is too large"));
    }
    push_u16(&mut bytes, page.entries.len() as u16);
    for entry in &page.entries {
        push_text(&mut bytes, &entry.name, MAX_COMPONENT_BYTES)?;
        bytes.push(match entry.kind {
            EntryKind::File => 0,
            EntryKind::Dir => 1,
            EntryKind::Symlink => 2,
            EntryKind::Other => 3,
        });
        push_u64(&mut bytes, entry.size);
        push_u64(&mut bytes, entry.mtime_ms);
        push_token(&mut bytes, entry.token);
    }
    Ok(bytes)
}

pub(crate) fn decode_list_body(bytes: &[u8]) -> Result<DirPage, WireError> {
    use super::entry::FsEntry;

    let error = |message: &'static str| WireError(message);
    let mut cursor = bytes;
    let total = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
    let next_raw = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
    let truncated = match take_byte(&mut cursor).ok_or(error("the listing body is malformed"))? {
        0 => false,
        1 => true,
        _ => return Err(error("the listing body is malformed")),
    };
    let non_utf8_skipped = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
    let count = take_u16(&mut cursor).ok_or(error("the listing body is malformed"))?;
    if count > MAX_PAGE {
        return Err(error("the listing body is malformed"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = take_text(&mut cursor, MAX_COMPONENT_BYTES)
            .ok_or(error("the listing body is malformed"))?;
        let kind = match take_byte(&mut cursor).ok_or(error("the listing body is malformed"))? {
            0 => EntryKind::File,
            1 => EntryKind::Dir,
            2 => EntryKind::Symlink,
            3 => EntryKind::Other,
            _ => return Err(error("the listing body is malformed")),
        };
        let size = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
        let mtime_ms = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
        let token = take_token(&mut cursor).ok_or(error("the listing body is malformed"))?;
        entries.push(FsEntry {
            name,
            kind,
            size,
            mtime_ms,
            token,
        });
    }
    if !cursor.is_empty() {
        return Err(error("the listing body is malformed"));
    }
    Ok(DirPage {
        entries,
        total,
        next_offset: (next_raw != u64::MAX).then_some(next_raw),
        truncated,
        non_utf8_skipped,
    })
}

/// Encodes the places reply body: roots, then folders.
pub(crate) fn encode_places_body(places: &Places) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    if places.roots.len() > usize::from(u16::MAX) || places.folders.len() > usize::from(u16::MAX) {
        return Err(Error::InvalidSetting("the places reply is too large"));
    }
    push_u16(&mut bytes, places.roots.len() as u16);
    for root in &places.roots {
        push_u64(&mut bytes, root.id);
        push_text(&mut bytes, &root.name, MAX_REL_BYTES)?;
    }
    push_u16(&mut bytes, places.folders.len() as u16);
    for folder in &places.folders {
        bytes.push(folder.kind.code());
        push_u64(&mut bytes, folder.root);
        push_text(&mut bytes, &folder.path, MAX_REL_BYTES)?;
    }
    Ok(bytes)
}

pub(crate) fn decode_places_body(bytes: &[u8]) -> Result<Places, WireError> {
    let error = || WireError("the places body is malformed");
    let mut cursor = bytes;
    let count = take_u16(&mut cursor).ok_or_else(error)?;
    if usize::from(count) > super::MAX_ROOTS {
        return Err(error());
    }
    let mut roots = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let id = take_u64(&mut cursor).ok_or_else(error)?;
        let name = take_text(&mut cursor, MAX_REL_BYTES).ok_or_else(error)?;
        roots.push(PlaceRoot { id, name });
    }
    let count = take_u16(&mut cursor).ok_or_else(error)?;
    // The usual folders, then up to 32 mounted drives.
    if count > 48 {
        return Err(error());
    }
    let mut folders = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let kind =
            FolderKind::from_code(take_byte(&mut cursor).ok_or_else(error)?).ok_or_else(error)?;
        let root = take_u64(&mut cursor).ok_or_else(error)?;
        let path = take_text(&mut cursor, MAX_REL_BYTES).ok_or_else(error)?;
        super::check_rel(&path).map_err(|_| error())?;
        folders.push(Place { kind, root, path });
    }
    if !cursor.is_empty() {
        return Err(error());
    }
    Ok(Places { roots, folders })
}

/// Caller-side navigation revisions. Each navigation mints a revision;
/// only a reply bearing the newest revision may paint the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionGate {
    current: u64,
}

impl RevisionGate {
    pub fn new() -> Self {
        Self { current: 0 }
    }

    /// Mints the revision for a fresh navigation.
    pub fn navigate(&mut self) -> u64 {
        self.current = self.current.saturating_add(1);
        self.current
    }

    /// True only for the newest outstanding revision.
    pub fn accept(&self, revision: u64) -> bool {
        revision != 0 && revision == self.current
    }
}

impl Default for RevisionGate {
    fn default() -> Self {
        Self::new()
    }
}

fn push_expected(bytes: &mut Vec<u8>, expected: Option<EntryToken>) {
    match expected {
        Some(token) => {
            bytes.push(1);
            push_token(bytes, token);
        }
        None => bytes.push(0),
    }
}

/// A token: its kind (0 weak, 1 identity, 2 fingerprint), then device,
/// inode, creation time and fingerprint, zero where the kind has none.
fn push_token(bytes: &mut Vec<u8>, token: EntryToken) {
    use super::entry::TokenKind;
    bytes.push(match (token.kind, token.is_strong()) {
        (_, false) => 0,
        (TokenKind::Identity, true) => 1,
        (TokenKind::Fingerprint, true) => 2,
    });
    let strong = token.is_strong();
    push_u64(bytes, token.dev.filter(|_| strong).unwrap_or(0));
    push_u64(
        bytes,
        token
            .ino
            .filter(|_| strong && token.kind == TokenKind::Identity)
            .unwrap_or(0),
    );
    push_born(
        bytes,
        token.born.filter(|_| token.kind == TokenKind::Identity),
    );
    push_u64(
        bytes,
        token
            .fingerprint
            .filter(|_| strong && token.kind == TokenKind::Fingerprint)
            .unwrap_or(0),
    );
}

/// A token as [`push_token`] writes it; `None` when malformed, including
/// a value written where its kind has none, so a decoded token encodes to
/// the same bytes.
fn take_token(cursor: &mut &[u8]) -> Option<EntryToken> {
    let kind = take_byte(cursor)?;
    let dev = take_u64(cursor)?;
    let ino = take_u64(cursor)?;
    let born = take_born(cursor)?;
    let fingerprint = take_u64(cursor)?;
    match kind {
        0 if dev == 0 && ino == 0 && fingerprint == 0 => Some(EntryToken {
            born,
            ..EntryToken::NONE
        }),
        1 if fingerprint == 0 => Some(EntryToken::identity(dev, ino, born)),
        2 if ino == 0 && born.is_none() => Some(EntryToken::fingerprinted(dev, fingerprint)),
        _ => None,
    }
}

/// A creation time: whether there is one, then its nanoseconds.
fn push_born(bytes: &mut Vec<u8>, born: Option<u64>) {
    bytes.push(u8::from(born.is_some()));
    push_u64(bytes, born.unwrap_or(0));
}

/// `None` inside when the frame says there is no creation time; `None`
/// outside when the frame is malformed.
fn take_born(cursor: &mut &[u8]) -> Option<Option<u64>> {
    let present = take_byte(cursor)?;
    let born = take_u64(cursor)?;
    match present {
        0 => Some(None),
        1 => Some(Some(born)),
        _ => None,
    }
}

fn take_expected(cursor: &mut &[u8]) -> Result<Option<EntryToken>, WireError> {
    let error = || WireError("the filesystem frame is malformed");
    match take_byte(cursor).ok_or_else(error)? {
        0 => Ok(None),
        1 => take_token(cursor).map(Some).ok_or_else(error),
        _ => Err(error()),
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_text(bytes: &mut Vec<u8>, text: &str, max: usize) -> Result<(), Error> {
    if text.len() > max || text.as_bytes().contains(&0) {
        return Err(Error::InvalidSetting("the filesystem text is too large"));
    }
    push_u16(bytes, text.len() as u16);
    bytes.extend_from_slice(text.as_bytes());
    Ok(())
}

fn take_byte(cursor: &mut &[u8]) -> Option<u8> {
    let (first, rest) = cursor.split_first()?;
    *cursor = rest;
    Some(*first)
}

fn take_u16(cursor: &mut &[u8]) -> Option<u16> {
    if cursor.len() < 2 {
        return None;
    }
    let value = u16::from_be_bytes([cursor[0], cursor[1]]);
    *cursor = &cursor[2..];
    Some(value)
}

fn take_u64(cursor: &mut &[u8]) -> Option<u64> {
    if cursor.len() < 8 {
        return None;
    }
    let value = u64::from_be_bytes(cursor[..8].try_into().ok()?);
    *cursor = &cursor[8..];
    Some(value)
}

fn take_text(cursor: &mut &[u8], max: usize) -> Option<String> {
    let len = take_u16(cursor)? as usize;
    // As `push_text` writes it: no NUL, which no path or name holds.
    if len > max || cursor.len() < len || cursor[..len].contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(&cursor[..len]).ok()?.to_owned();
    *cursor = &cursor[len..];
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listing page survives its wire body whole, and a body cut short or
    /// trailing a byte is refused.
    #[test]
    fn list_body_round_trips() {
        use crate::test_support::{loopback_core, FilesystemLocation, Scratch};
        let scratch = Scratch::tagged("fs");
        let root = scratch.join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"abc").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let core = loopback_core();
        core.enable_filesystem_access().unwrap();
        let files = FilesystemLocation::on(&core, &root);
        let page = core.list_dir(files.root, &files.path(""), 0, 50).unwrap();
        let bytes = encode_list_body(&page).unwrap();
        let back = decode_list_body(&bytes).unwrap();
        assert_eq!(back, page, "a page decodes to what was encoded");
        assert!(
            decode_list_body(&bytes[..bytes.len() - 1]).is_err(),
            "a body cut short is refused"
        );
        let mut bad = bytes.clone();
        bad.push(0);
        assert!(
            decode_list_body(&bad).is_err(),
            "a trailing byte is refused"
        );
    }

    /// Found by the `fs_wire` fuzz target: a List whose path is one NUL byte
    /// decoded, though no request with it can be encoded.
    #[test]
    fn wire_text_with_a_nul_byte_is_refused() {
        let mut list = vec![1, 0, 0x24, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 1];
        list.extend_from_slice(&2u64.to_be_bytes());
        list.extend_from_slice(&[0, 1, 0]);
        list.extend_from_slice(&0u64.to_be_bytes());
        list.extend_from_slice(&10u16.to_be_bytes());
        assert!(crate::fs::decode_request(&list).is_err());
        let path_ok = [&list[..22], &[0, 1, b'a'], &list[25..]].concat();
        assert!(crate::fs::decode_request(&path_ok).is_ok());
    }

    /// Breaks the `fs_wire` fuzz target's round trip: a strong listing token
    /// with a zero device decoded as half a token, which re-encodes as none.
    #[test]
    fn a_strong_listing_token_decodes_whole() {
        let mut body = Vec::new();
        body.extend_from_slice(&1u64.to_be_bytes());
        body.extend_from_slice(&u64::MAX.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&[0, 1, b'a', 0]);
        body.extend_from_slice(&[0; 16]);
        body.push(1);
        body.extend_from_slice(&0u64.to_be_bytes());
        body.extend_from_slice(&5u64.to_be_bytes());
        body.push(1);
        body.extend_from_slice(&7u64.to_be_bytes());
        body.extend_from_slice(&0u64.to_be_bytes());
        let page = crate::fs::decode_list_body(&body).unwrap();
        assert_eq!(page.entries[0].token, EntryToken::identity(0, 5, Some(7)));
        let encoded = crate::fs::encode_list_body(&page).unwrap();
        assert_eq!(encoded, body);
    }

    /// A listing carries each token's kind: a fingerprint comes back a
    /// fingerprint, and a token holding a value its kind has none of is
    /// malformed.
    #[test]
    fn a_listing_keeps_each_token_s_kind() {
        use crate::fs::{DirPage, EntryKind, FsEntry};
        let entry = |name: &str, token| FsEntry {
            name: name.to_owned(),
            kind: EntryKind::File,
            size: 1,
            mtime_ms: 2,
            token,
        };
        let page = DirPage {
            entries: vec![
                entry("a", EntryToken::identity(1, 2, Some(3))),
                entry("b", EntryToken::fingerprinted(1, 0xabc)),
                entry("c", EntryToken::NONE),
            ],
            total: 3,
            next_offset: None,
            truncated: false,
            non_utf8_skipped: 0,
        };
        let body = crate::fs::encode_list_body(&page).unwrap();
        assert_eq!(crate::fs::decode_list_body(&body).unwrap(), page);
        // The weak token is last: give it a device.
        let mut weak_with_dev = body.clone();
        let at = body.len() - 8 - 9 - 8 - 8;
        weak_with_dev[at + 7] = 1;
        assert!(crate::fs::decode_list_body(&weak_with_dev).is_err());
    }

    #[test]
    fn wire_codec_round_trips_and_rejects() {
        use crate::fs::{Request as WireRequest, Response as WireResponse};

        for request in [
            WireRequest::List {
                id: 7,
                generation: 3,
                root: 0,
                path: "docs/2026".to_string(),
                offset: 3,
                limit: 50,
            },
            WireRequest::Mkdir {
                id: 8,
                generation: 3,
                root: 1,
                path: "".to_string(),
                name: "new".to_string(),
            },
            WireRequest::Rename {
                id: 9,
                generation: 4,
                root: 0,
                path: "docs".to_string(),
                name: "a.txt".to_string(),
                expected: Some(EntryToken::identity(3, 9, Some(1_792_781_822_915_157_352))),
                new_name: "b.txt".to_string(),
            },
            WireRequest::Rename {
                id: 10,
                generation: 4,
                root: 0,
                path: "".to_string(),
                name: "a.txt".to_string(),
                expected: None,
                new_name: "b.txt".to_string(),
            },
            WireRequest::Delete {
                id: 11,
                generation: 4,
                root: 0,
                path: "docs".to_string(),
                name: "a.txt".to_string(),
                expected: Some(EntryToken::fingerprinted(3, 0xfeed_beef)),
                trash: false,
            },
            WireRequest::Delete {
                id: 12,
                generation: 4,
                root: 0,
                path: "".to_string(),
                name: "a.txt".to_string(),
                expected: None,
                trash: true,
            },
            WireRequest::Places {
                id: 13,
                generation: 4,
            },
        ] {
            let bytes = crate::fs::encode_request(&request).unwrap();
            assert_eq!(crate::fs::decode_request(&bytes).unwrap(), request);
        }

        let ok = WireResponse::ok(11);
        assert!(ok.is_ok());
        let bytes = crate::fs::encode_response(&ok);
        let back = crate::fs::decode_response(&bytes).unwrap();
        assert_eq!(back.id, 11);
        assert!(back.is_ok());

        // Wrong version, truncation, unknown tag and trailing bytes all fail.
        let mut bad = crate::fs::encode_request(&WireRequest::List {
            id: 1,
            generation: 1,
            root: 0,
            path: "".to_string(),
            offset: 0,
            limit: 10,
        })
        .unwrap();
        bad[0] = 99;
        assert!(crate::fs::decode_request(&bad).is_err());
        let truncated = &bad[..4];
        assert!(crate::fs::decode_request(truncated).is_err());
        let mut tagged = crate::fs::encode_request(&WireRequest::List {
            id: 1,
            generation: 1,
            root: 0,
            path: "".to_string(),
            offset: 0,
            limit: 10,
        })
        .unwrap();
        tagged[13] = 77;
        assert!(crate::fs::decode_request(&tagged).is_err());
        // Oversize text is refused at encode time, never sent.
        let huge = "a".repeat(crate::fs::MAX_REL_BYTES + 1);
        assert!(crate::fs::encode_request(&WireRequest::List {
            id: 1,
            generation: 1,
            root: 0,
            path: huge,
            offset: 0,
            limit: 10,
        })
        .is_err());
    }

    #[test]
    fn places_body_round_trips_and_refuses_unsafe_folders() {
        let places = crate::fs::Places {
            roots: vec![
                crate::fs::PlaceRoot {
                    id: 2,
                    name: "C:".to_owned(),
                },
                crate::fs::PlaceRoot {
                    id: 3,
                    name: "D:".to_owned(),
                },
            ],
            folders: vec![
                crate::fs::Place {
                    kind: crate::fs::FolderKind::Home,
                    root: 2,
                    path: "Users/someone".to_owned(),
                },
                crate::fs::Place {
                    kind: crate::fs::FolderKind::Downloads,
                    root: 2,
                    path: "Users/someone/Downloads".to_owned(),
                },
                crate::fs::Place {
                    kind: crate::fs::FolderKind::Volume,
                    root: 2,
                    path: "Volumes/Backup".to_owned(),
                },
            ],
        };
        let body = crate::fs::encode_places_body(&places).unwrap();
        assert_eq!(crate::fs::decode_places_body(&body).unwrap(), places);
        let mut escaping = places.clone();
        escaping.folders[0].path = "../outside".to_owned();
        let body = crate::fs::encode_places_body(&escaping).unwrap();
        assert!(crate::fs::decode_places_body(&body).is_err());
        let mut trailing = crate::fs::encode_places_body(&places).unwrap();
        trailing.push(0);
        assert!(crate::fs::decode_places_body(&trailing).is_err());
        // A places request carries no root or path of its own.
        let mut with_root = crate::fs::encode_request(&crate::fs::Request::Places {
            id: 1,
            generation: 1,
        })
        .unwrap();
        with_root[21] = 1;
        assert!(crate::fs::decode_request(&with_root).is_err());
    }

    #[test]
    fn revision_gate_fences_late_replies() {
        let mut gate = RevisionGate::new();
        assert!(!gate.accept(0));
        let first = gate.navigate();
        let second = gate.navigate();
        assert_ne!(first, second);
        // The delayed reply to the first navigation must not paint.
        assert!(!gate.accept(first));
        assert!(gate.accept(second));
    }
}
