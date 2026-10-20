//! Asking for one file and receiving it into its partial.

use iroh::endpoint::{Connection, ReadError, RecvStream, VarInt};

use crate::error::Error;

use super::wire::{FileRequest, RESET_CHANGED, STATUS_CHANGED, STATUS_SENDING, VALUE_LEN};
use super::{Partial, CHUNK};

/// The sender's answer to a request.
pub(crate) enum Answer {
    /// The bytes follow on this stream.
    Sending(RecvStream),
    /// The source is no longer the file the manifest describes.
    Changed,
    /// The sender does not serve this file now.
    Denied,
}

/// How receiving one file ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Received {
    /// Every segment of the file checked; carries the bytes received,
    /// below the size when it resumed.
    Done { downloaded: u64 },
    /// Stopped by the cancel watch; the partial stays for resume.
    Cancelled,
    /// The sender does not serve this file now.
    Denied,
    /// The source is no longer the file the manifest describes.
    Changed,
    /// A segment did not hash to the sender's value; its bytes are
    /// dropped and the partial keeps the segments before it.
    Mismatch,
}

/// Opens a stream, sends the request and reads the sender's answer.
pub(crate) async fn ask(connection: &Connection, request: FileRequest) -> Result<Answer, Error> {
    let (mut send, mut recv) = connection.open_bi().await.map_err(Error::network)?;
    send.write_all(&request.encode())
        .await
        .map_err(Error::network)?;
    send.finish().map_err(Error::network)?;
    let mut status = [0u8; 1];
    recv.read_exact(&mut status).await.map_err(Error::network)?;
    Ok(match status[0] {
        STATUS_SENDING => Answer::Sending(recv),
        STATUS_CHANGED => Answer::Changed,
        _ => Answer::Denied,
    })
}

/// Appends what arrives on `recv` to `partial` until the file is whole,
/// checking each segment against the value that follows it. Reports the
/// bytes written to `progress` as they arrive; a cancel stops at once and
/// keeps the checked segments.
pub(crate) async fn receive(
    mut recv: RecvStream,
    partial: &mut Partial,
    size: u64,
    progress: &std::sync::mpsc::Sender<u64>,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<Received, Error> {
    let from = partial.checked();
    let stopped = async {
        if cancel.wait_for(|stop| *stop).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopped);
    // The value of the segment just written, while it arrives.
    let mut value: Option<Vec<u8>> = None;
    loop {
        let chunk = tokio::select! {
            biased;
            _ = &mut stopped => return Ok(Received::Cancelled),
            chunk = recv.read_chunk(CHUNK) => chunk,
        };
        let bytes = match chunk {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(ReadError::Reset(code)) if code == VarInt::from_u32(RESET_CHANGED) => {
                return Ok(Received::Changed);
            }
            Err(error) => return Err(Error::network(error)),
        };
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            if let Some(pending) = value.as_mut() {
                let take = rest.len().min(VALUE_LEN - pending.len());
                pending.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                if pending.len() == VALUE_LEN {
                    if !partial.seal(pending)? {
                        return Ok(Received::Mismatch);
                    }
                    value = None;
                }
                continue;
            }
            let left = partial.segment_left();
            if left == 0 {
                // Bytes past the file's end.
                return Err(Error::Malformed);
            }
            let take = rest.len().min(left as usize);
            partial.append(&rest[..take])?;
            rest = &rest[take..];
            let _ = progress.send(partial.written());
            if partial.segment_left() == 0 {
                value = Some(Vec::with_capacity(VALUE_LEN));
            }
        }
    }
    if value.is_some() || partial.checked() != size {
        return Err(Error::network("the file stream ended early"));
    }
    Ok(Received::Done {
        downloaded: size - from,
    })
}
