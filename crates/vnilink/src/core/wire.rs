//! Control-frame read and write.

use std::time::Duration;

use crate::error::Error;
use crate::protocol::{ControlMessage, MAX_ADDRESS_FRAME, MAX_CONTROL_FRAME};

pub(super) async fn write_frame(
    send: &mut iroh::endpoint::SendStream,
    message: &ControlMessage,
) -> Result<(), Error> {
    let limit = match message {
        ControlMessage::Addresses { .. } => MAX_ADDRESS_FRAME,
        ControlMessage::Topology { .. } => crate::protocol::MAX_TOPOLOGY_FRAME,
        _ => MAX_CONTROL_FRAME,
    };
    let payload = message.encode();
    if payload.is_empty() || payload.len() > limit {
        return Err(Error::Malformed);
    }
    let len = u16::try_from(payload.len()).map_err(|_| Error::Malformed)?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(Error::network_debug)?;
    send.write_all(&payload)
        .await
        .map_err(Error::network_debug)?;
    Ok(())
}

pub(super) async fn read_frame(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<ControlMessage, Error> {
    read_frame_within(recv, Some(Duration::from_secs(10))).await
}

/// Reads a frame on a session that is already up. Silence is not a timeout.
pub(super) async fn read_session_frame(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<ControlMessage, Error> {
    read_frame_within(recv, None).await
}

pub(super) async fn read_frame_within(
    recv: &mut iroh::endpoint::RecvStream,
    timeout: Option<Duration>,
) -> Result<ControlMessage, Error> {
    let mut len_buf = [0u8; 2];
    let length = recv.read_exact(&mut len_buf);
    let read = match timeout {
        Some(timeout) => tokio::time::timeout(timeout, length)
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::network_debug)?,
        None => length.await.map_err(Error::network_debug)?,
    };
    let _ = read;
    let len = usize::from(u16::from_be_bytes(len_buf));
    if len == 0 || len > MAX_ADDRESS_FRAME {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    tokio::time::timeout(Duration::from_secs(10), recv.read_exact(&mut payload))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    ControlMessage::decode(&payload)
}
