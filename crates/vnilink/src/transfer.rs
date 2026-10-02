//! One file for one control session.
//!
//! The bytes ride their own stream so a transfer does not wait behind keyboard
//! input or clipboard text. The host writes a file only when that controller's
//! transfer grant is on. The name has to stay inside the directory chosen for
//! this installation. This module does not log the name or the bytes.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Error;

/// Largest file, in bytes. An empty file is allowed.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) const MAX_NAME_BYTES: usize = 1024;
pub(crate) const MAX_COMPONENT_BYTES: usize = 255;
pub(crate) const MAX_COMPONENTS: usize = 16;
pub(crate) const MAX_CHUNK: usize = 64 * 1024;

/// Tag plus one chunk. The outer frame length is four more bytes.
pub(crate) const MAX_FRAME: usize = 1 + MAX_CHUNK;

const READY: u8 = 1;
const BEGIN: u8 = 2;
const CHUNK: u8 = 3;
const FINISH: u8 = 4;
const STATUS: u8 = 5;

const STATUS_APPLIED: u8 = 1;
const STATUS_DENIED: u8 = 2;
const STATUS_UNAVAILABLE: u8 = 3;
const STATUS_EXISTS: u8 = 4;
const STATUS_REJECTED: u8 = 5;

const UNSAFE_NAME: &str = "the file name is not a safe relative path";
const TOO_BIG: &str = "the file is larger than 8 MiB";
const SAVE_FAILED: &str = "the file could not be saved";
const REJECTED: &str = "the file was rejected";
const DIRECTORY_FAILED: &str = "the transfer directory could not be created";

pub(crate) fn check_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > MAX_NAME_BYTES
        || name.as_bytes().contains(&0)
        || name.contains('\\')
        || name.contains(':')
        || name.starts_with('/')
        || name.ends_with('/')
    {
        return Err(Error::InvalidSetting(UNSAFE_NAME));
    }
    let mut count = 0usize;
    for component in name.split('/') {
        count += 1;
        if count > MAX_COMPONENTS
            || component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > MAX_COMPONENT_BYTES
        {
            return Err(Error::InvalidSetting(UNSAFE_NAME));
        }
    }
    Ok(())
}

pub(crate) fn checked_size(size: u64) -> Result<usize, Error> {
    let size = usize::try_from(size).map_err(|_| Error::InvalidSetting(TOO_BIG))?;
    if size > MAX_FILE_BYTES {
        Err(Error::InvalidSetting(TOO_BIG))
    } else {
        Ok(size)
    }
}

pub(crate) fn check_submission(name: &str, bytes: &[u8]) -> Result<(), Error> {
    check_name(name)?;
    if bytes.len() > MAX_FILE_BYTES {
        Err(Error::InvalidSetting(TOO_BIG))
    } else {
        Ok(())
    }
}

/// Creates the directory when it is missing and stores its canonical path.
///
/// Canonicalize follows a symlink only for this chosen root. Later writes do
/// not canonicalize names inside it.
pub(crate) fn prepare_directory(path: &Path) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(path).map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    let canonical =
        std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    let meta = std::fs::symlink_metadata(&canonical)
        .map_err(|_| Error::InvalidSetting(DIRECTORY_FAILED))?;
    if !meta.is_dir() {
        return Err(Error::InvalidSetting(DIRECTORY_FAILED));
    }
    Ok(canonical)
}

/// Writes `bytes` at `name` under `root`.
///
/// `name` is a relative path. A symlink is not a destination, and `..` never
/// leaves `root`. `replace` is required to change an existing regular file.
/// The bytes land in one rename, so a failed call leaves no partial file.
pub(crate) fn write_file(
    root: &Path,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    check_submission(name, bytes)?;
    let parts: Vec<&str> = name.split('/').collect();
    let file_name = parts[parts.len() - 1];
    let mut parent = root.to_path_buf();
    for component in &parts[..parts.len() - 1] {
        parent.push(component);
        if !under_root(root, &parent) {
            return Err(Error::InvalidSetting(UNSAFE_NAME));
        }
        ensure_real_dir(&parent)?;
    }
    let dest = parent.join(file_name);
    if !under_root(root, &dest) {
        return Err(Error::InvalidSetting(UNSAFE_NAME));
    }
    match classify(&dest)? {
        Dest::Reject => return Err(Error::InvalidSetting(UNSAFE_NAME)),
        Dest::File if !replace => return Err(Error::FileExists),
        Dest::File | Dest::Missing => {}
    }
    let temp = write_unique_temp(root, &parent, bytes)?;
    let result = commit(&temp, &dest, replace);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn under_root(root: &Path, path: &Path) -> bool {
    path.starts_with(root) && path != root
}

fn ensure_real_dir(path: &Path) -> Result<(), Error> {
    for _ in 0..2 {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::InvalidSetting(UNSAFE_NAME))
            }
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => return Err(Error::InvalidSetting(UNSAFE_NAME)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(path) {
                    Ok(()) => return Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(Error::InvalidSetting(SAVE_FAILED)),
                }
            }
            Err(_) => return Err(Error::InvalidSetting(SAVE_FAILED)),
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Err(_) => Err(Error::InvalidSetting(SAVE_FAILED)),
    }
}

enum Dest {
    Missing,
    File,
    Reject,
}

fn classify(path: &Path) -> Result<Dest, Error> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Ok(Dest::Reject),
        Ok(meta) if meta.is_dir() => Ok(Dest::Reject),
        Ok(meta) if meta.is_file() => Ok(Dest::File),
        Ok(_) => Ok(Dest::Reject),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Dest::Missing),
        Err(_) => Err(Error::InvalidSetting(SAVE_FAILED)),
    }
}

fn write_unique_temp(root: &Path, parent: &Path, bytes: &[u8]) -> Result<PathBuf, Error> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    for _ in 0..8 {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{id}.vnlpart"));
        if !under_root(root, &path) {
            continue;
        }
        if write_temp(&path, bytes).is_ok() {
            return Ok(path);
        }
    }
    Err(Error::InvalidSetting(SAVE_FAILED))
}

fn write_temp(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(Error::InvalidSetting(SAVE_FAILED));
    }
    Ok(())
}

fn commit(temp: &Path, dest: &Path, replace: bool) -> Result<(), Error> {
    match classify(dest)? {
        Dest::Reject => Err(Error::InvalidSetting(UNSAFE_NAME)),
        Dest::File if !replace => Err(Error::FileExists),
        Dest::Missing => {
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::File => rename_over(temp, dest),
    }
}

/// Unix rename replaces a directory entry and does not follow a symlink.
/// Windows rename fails when the destination exists, so remove a regular
/// file and rename again. A symlink is never removed.
fn rename_over(temp: &Path, dest: &Path) -> Result<(), Error> {
    if std::fs::rename(temp, dest).is_ok() {
        return Ok(());
    }
    match classify(dest)? {
        Dest::File => {
            std::fs::remove_file(dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))?;
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::Missing => {
            std::fs::rename(temp, dest).map_err(|_| Error::InvalidSetting(SAVE_FAILED))
        }
        Dest::Reject => Err(Error::InvalidSetting(UNSAFE_NAME)),
    }
}

/// One transfer frame. Debug prints lengths, not the name or the bytes.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Ready,
    Begin {
        replace: bool,
        name: String,
        size: u64,
    },
    Chunk(Vec<u8>),
    Finish,
    Status(u8),
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => formatter.write_str("Ready"),
            Self::Begin {
                replace,
                name,
                size,
            } => formatter
                .debug_struct("Begin")
                .field("replace", replace)
                .field("name_len", &name.len())
                .field("size", size)
                .finish(),
            Self::Chunk(bytes) => formatter
                .debug_struct("Chunk")
                .field("len", &bytes.len())
                .finish(),
            Self::Finish => formatter.write_str("Finish"),
            Self::Status(code) => formatter.debug_tuple("Status").field(code).finish(),
        }
    }
}

pub(crate) struct TransferRequest {
    pub name: String,
    pub bytes: Vec<u8>,
    pub replace: bool,
    pub reply: std::sync::mpsc::SyncSender<Result<(), Error>>,
}

pub(crate) type TransferSender = tokio::sync::mpsc::Sender<TransferRequest>;

pub(crate) fn encode(frame: &Frame) -> Result<Vec<u8>, Error> {
    let mut payload = Vec::new();
    match frame {
        Frame::Ready => payload.push(READY),
        Frame::Finish => payload.push(FINISH),
        Frame::Status(code) => {
            if !matches!(
                *code,
                STATUS_APPLIED
                    | STATUS_DENIED
                    | STATUS_UNAVAILABLE
                    | STATUS_EXISTS
                    | STATUS_REJECTED
            ) {
                return Err(Error::Malformed);
            }
            payload.push(STATUS);
            payload.push(*code);
        }
        Frame::Begin {
            replace,
            name,
            size,
        } => {
            check_name(name)?;
            checked_size(*size)?;
            payload.push(BEGIN);
            payload.push(u8::from(*replace));
            let name_len = u16::try_from(name.len()).map_err(|_| Error::Malformed)?;
            payload.extend_from_slice(&name_len.to_be_bytes());
            payload.extend_from_slice(name.as_bytes());
            payload.extend_from_slice(&size.to_be_bytes());
        }
        Frame::Chunk(bytes) => {
            if bytes.len() > MAX_CHUNK {
                return Err(Error::Malformed);
            }
            payload.push(CHUNK);
            payload.extend_from_slice(bytes);
        }
    }
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(Error::Malformed);
    }
    let mut bytes = Vec::with_capacity(4 + payload.len());
    let len = u32::try_from(payload.len()).map_err(|_| Error::Malformed)?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend(payload);
    Ok(bytes)
}

pub(crate) fn decode(payload: &[u8]) -> Result<Frame, Error> {
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(Error::Malformed);
    }
    match payload[0] {
        READY if payload.len() == 1 => Ok(Frame::Ready),
        FINISH if payload.len() == 1 => Ok(Frame::Finish),
        STATUS if payload.len() == 2 && matches!(payload[1], 1 | 2 | 3 | 4 | 5) => {
            Ok(Frame::Status(payload[1]))
        }
        BEGIN => decode_begin(payload),
        CHUNK => Ok(Frame::Chunk(payload[1..].to_vec())),
        _ => Err(Error::Malformed),
    }
}

fn decode_begin(payload: &[u8]) -> Result<Frame, Error> {
    if payload.len() < 12 {
        return Err(Error::Malformed);
    }
    let replace = match payload[1] {
        0 => false,
        1 => true,
        _ => return Err(Error::Malformed),
    };
    let name_len = usize::from(u16::from_be_bytes([payload[2], payload[3]]));
    if payload.len() != 4 + name_len + 8 {
        return Err(Error::Malformed);
    }
    let name = std::str::from_utf8(&payload[4..4 + name_len]).map_err(|_| Error::Malformed)?;
    let size = u64::from_be_bytes(
        payload[4 + name_len..]
            .try_into()
            .map_err(|_| Error::Malformed)?,
    );
    Ok(Frame::Begin {
        replace,
        name: name.to_owned(),
        size,
    })
}

pub(crate) fn check_len(len: usize) -> Result<(), Error> {
    if len == 0 || len > MAX_FRAME {
        Err(Error::Malformed)
    } else {
        Ok(())
    }
}

fn status_result(code: u8) -> Result<(), Error> {
    match code {
        STATUS_APPLIED => Ok(()),
        STATUS_DENIED => Err(Error::TransferDenied),
        STATUS_UNAVAILABLE => Err(Error::Unsupported("file transfer is not available")),
        STATUS_EXISTS => Err(Error::FileExists),
        STATUS_REJECTED => Err(Error::InvalidSetting(REJECTED)),
        _ => Err(Error::Malformed),
    }
}

fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::TransferDenied) => STATUS_DENIED,
        Err(Error::Unsupported(_)) => STATUS_UNAVAILABLE,
        Err(Error::FileExists) => STATUS_EXISTS,
        Err(_) => STATUS_REJECTED,
    }
}

/// A finished transfer writes only when the byte count matches and no earlier
/// check failed. A mismatch drops the buffer.
pub(crate) fn finish_decision(
    got: u64,
    size: u64,
    failed: bool,
    preset: Option<Error>,
) -> Result<(), Error> {
    if failed || got != size {
        Err(Error::InvalidSetting(REJECTED))
    } else if let Some(error) = preset {
        Err(error)
    } else {
        Ok(())
    }
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<Frame, Error> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(Error::network_debug)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    check_len(len)?;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .map_err(Error::network_debug)?;
    decode(&payload)
}

async fn write_frame(send: &mut iroh::endpoint::SendStream, frame: &Frame) -> Result<(), Error> {
    let bytes = encode(frame)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Error::network("the transfer frame was not written"))?;
    Ok(())
}

async fn write_transfer(
    send: &mut iroh::endpoint::SendStream,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    check_submission(name, bytes)?;
    write_frame(
        send,
        &Frame::Begin {
            replace,
            name: name.to_owned(),
            size: bytes.len() as u64,
        },
    )
    .await?;
    for chunk in bytes.chunks(MAX_CHUNK) {
        write_frame(send, &Frame::Chunk(chunk.to_vec())).await?;
    }
    write_frame(send, &Frame::Finish).await
}

pub(crate) async fn accept_bi(
    connection: &iroh::endpoint::Connection,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), Error> {
    if *stop_rx.borrow() {
        return Err(Error::NoSession);
    }
    tokio::select! {
        result = connection.accept_bi() => result.map_err(Error::network_debug),
        _ = stop_rx.changed() => Err(Error::NoSession),
    }
}

struct Incoming {
    replace: bool,
    name: String,
    size: u64,
    got: u64,
    buf: Option<Vec<u8>>,
    failed: bool,
    preset: Option<Error>,
}

/// Owns one transfer stream.
///
/// `dial` is the side that opens the stream. Both sides wait for a status
/// after sending a file, and both write a status after receiving one.
/// `inspect` runs before the body is buffered. `on_file` runs only when the
/// received length matches the declared size.
pub(crate) async fn attach<Inspect, OnFile, Allow>(
    connection: iroh::endpoint::Connection,
    dial: bool,
    requests: tokio::sync::mpsc::Receiver<TransferRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    opened: Option<tokio::sync::oneshot::Sender<Result<(), Error>>>,
    inspect: Inspect,
    on_file: OnFile,
    allow_outgoing: Allow,
) where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error> + Send,
    OnFile: FnMut(&str, &[u8], bool) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    if *stop_rx.borrow() {
        if let Some(opened) = opened {
            let _ = opened.send(Err(Error::NoSession));
        }
        return;
    }
    let streams = tokio::select! {
        result = open_stream(&connection, dial) => result,
        _ = stop_rx.changed() => Err(Error::NoSession),
    };
    match streams {
        Ok((mut send, recv)) => {
            // The peer's accept_bi stays pending until this side sends a byte.
            if dial {
                if let Err(error) = write_frame(&mut send, &Frame::Ready).await {
                    if let Some(opened) = opened {
                        let _ = opened.send(Err(error));
                    }
                    return;
                }
            }
            if let Some(opened) = opened {
                let _ = opened.send(Ok(()));
            }
            exchange(
                send,
                recv,
                requests,
                stop_rx,
                inspect,
                on_file,
                allow_outgoing,
            )
            .await;
        }
        Err(error) => {
            if let Some(opened) = opened {
                let _ = opened.send(Err(error));
            }
        }
    }
}

async fn open_stream(
    connection: &iroh::endpoint::Connection,
    dial: bool,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), Error> {
    if dial {
        connection.open_bi().await.map_err(Error::network_debug)
    } else {
        connection.accept_bi().await.map_err(Error::network_debug)
    }
}

pub(crate) async fn exchange<Inspect, OnFile, Allow>(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut requests: tokio::sync::mpsc::Receiver<TransferRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    mut inspect: Inspect,
    mut on_file: OnFile,
    mut allow_outgoing: Allow,
) where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error> + Send,
    OnFile: FnMut(&str, &[u8], bool) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            match read_frame(&mut recv).await {
                Ok(frame) => {
                    if frames_tx.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = frames_tx.send(Err(error)).await;
                    break;
                }
            }
        }
    });
    let mut pending: Option<std::sync::mpsc::SyncSender<Result<(), Error>>> = None;
    let mut incoming: Option<Incoming> = None;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
            request = requests.recv(), if pending.is_none() && incoming.is_none() => {
                let Some(TransferRequest { name, bytes, replace, reply }) = request else {
                    break;
                };
                if let Err(error) = allow_outgoing() {
                    let _ = reply.send(Err(error));
                    continue;
                }
                if let Err(error) = write_transfer(&mut send, &name, &bytes, replace).await {
                    let _ = reply.send(Err(error));
                    break;
                }
                pending = Some(reply);
            }
            frame = frames_rx.recv() => {
                match frame {
                    Some(Ok(Frame::Ready)) => {}
                    Some(Ok(Frame::Begin { replace, name, size })) => {
                        if pending.is_some() || incoming.is_some() {
                            if let Some(reply) = pending.take() {
                                let _ = reply.send(Err(Error::Malformed));
                            }
                            break;
                        }
                        incoming = Some(begin_incoming(&mut inspect, replace, name, size));
                    }
                    Some(Ok(Frame::Chunk(chunk))) => {
                        let Some(current) = incoming.as_mut() else {
                            break;
                        };
                        push_chunk(current, &chunk);
                    }
                    Some(Ok(Frame::Finish)) => {
                        let Some(current) = incoming.take() else {
                            break;
                        };
                        let result = match finish_decision(
                            current.got,
                            current.size,
                            current.failed,
                            current.preset,
                        ) {
                            Ok(()) => on_file(
                                &current.name,
                                current.buf.as_deref().unwrap_or(&[]),
                                current.replace,
                            ),
                            Err(error) => Err(error),
                        };
                        if write_frame(&mut send, &Frame::Status(status_code(&result)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Frame::Status(code))) => {
                        if incoming.is_some() {
                            break;
                        }
                        if let Some(reply) = pending.take() {
                            let _ = reply.send(status_result(code));
                        }
                    }
                    Some(Err(error)) => {
                        if let Some(reply) = pending.take() {
                            let _ = reply.send(Err(error));
                        }
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    if let Some(reply) = pending.take() {
        let _ = reply.send(Err(Error::NoSession));
    }
    while let Ok(request) = requests.try_recv() {
        let _ = request.reply.send(Err(Error::NoSession));
    }
}

fn begin_incoming<Inspect>(
    inspect: &mut Inspect,
    replace: bool,
    name: String,
    size: u64,
) -> Incoming
where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error>,
{
    match inspect(&name, size, replace) {
        Err(error) => Incoming {
            replace,
            name,
            size,
            got: 0,
            buf: None,
            failed: false,
            preset: Some(error),
        },
        Ok(()) => {
            let mut buf = Vec::new();
            let reserved = checked_size(size)
                .ok()
                .and_then(|cap| buf.try_reserve_exact(cap).ok());
            if reserved.is_none() {
                return Incoming {
                    replace,
                    name,
                    size,
                    got: 0,
                    buf: None,
                    failed: false,
                    preset: Some(Error::InvalidSetting(REJECTED)),
                };
            }
            Incoming {
                replace,
                name,
                size,
                got: 0,
                buf: Some(buf),
                failed: false,
                preset: None,
            }
        }
    }
}

fn push_chunk(incoming: &mut Incoming, chunk: &[u8]) {
    let added = chunk.len() as u64;
    if chunk.len() > MAX_CHUNK || incoming.got.saturating_add(added) > incoming.size {
        incoming.failed = true;
        incoming.buf = None;
        return;
    }
    incoming.got += added;
    if let Some(buf) = incoming.buf.as_mut() {
        buf.extend_from_slice(chunk);
    }
}
