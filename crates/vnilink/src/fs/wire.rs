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

const STATUS_OK: u8 = 0;
/// Machine-readable response codes. The message line stays human-only.
pub(crate) const STATUS_EXISTS: u8 = 1;
pub(crate) const STATUS_DENIED: u8 = 2;
pub(crate) const STATUS_STALE: u8 = 3;
pub(crate) const STATUS_ERROR: u8 = 4;

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
    Delete {
        id: u32,
        generation: u64,
        root: u64,
        path: String,
        name: String,
        expected: Option<EntryToken>,
    },
    /// The host's roots, home folder and usual folders.
    Places { id: u32, generation: u64 },
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
        } => {
            push_u32(&mut bytes, *id);
            push_u64(&mut bytes, *generation);
            bytes.push(TAG_DELETE);
            push_u64(&mut bytes, *root);
            push_text(&mut bytes, path, MAX_REL_BYTES)?;
            push_text(&mut bytes, name, MAX_COMPONENT_BYTES)?;
            push_expected(&mut bytes, *expected);
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
        let token = entry.token;
        bytes.push(u8::from(token.is_strong()));
        push_u64(&mut bytes, token.dev.unwrap_or(0));
        push_u64(&mut bytes, token.ino.unwrap_or(0));
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
        let strong = take_byte(&mut cursor).ok_or(error("the listing body is malformed"))?;
        let dev = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
        let ino = take_u64(&mut cursor).ok_or(error("the listing body is malformed"))?;
        if strong != 0 && strong != 1 {
            return Err(error("the listing body is malformed"));
        }
        entries.push(FsEntry {
            name,
            kind,
            size,
            mtime_ms,
            token: EntryToken {
                dev: (strong == 1 && dev != 0).then_some(dev),
                ino: (strong == 1 && ino != 0).then_some(ino),
            },
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
    if count > 16 {
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
            push_u64(bytes, token.dev.unwrap_or(0));
            push_u64(bytes, token.ino.unwrap_or(0));
            bytes.push(u8::from(token.is_strong()));
        }
        None => bytes.push(0),
    }
}

fn take_expected(cursor: &mut &[u8]) -> Result<Option<EntryToken>, WireError> {
    let error = || WireError("the filesystem frame is malformed");
    match take_byte(cursor).ok_or_else(error)? {
        0 => Ok(None),
        1 => {
            let dev = take_u64(cursor).ok_or_else(error)?;
            let ino = take_u64(cursor).ok_or_else(error)?;
            let strong = take_byte(cursor).ok_or_else(error)?;
            if strong != 0 && strong != 1 {
                return Err(error());
            }
            Ok(Some(EntryToken {
                dev: (dev != 0).then_some(dev),
                ino: (ino != 0).then_some(ino),
            }))
        }
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
    if len > max || cursor.len() < len {
        return None;
    }
    let text = std::str::from_utf8(&cursor[..len]).ok()?.to_owned();
    *cursor = &cursor[len..];
    Some(text)
}
