//! Length-prefixed frames on the streams beside the control channel: a
//! service's pipe, a Desktop's clipboard pipe, a file request. Each stream
//! keeps its own length limit and codec; the 4-byte big-endian prefix is
//! shared.
// trace: records every ignored failure

use crate::error::Error;

/// Length-prefixed frame read shared by the side streams. The caller supplies
/// its own length limit and decoder; the 4-byte big-endian prefix is the same
/// on both streams.
pub(crate) async fn read_frame<F>(
    recv: &mut iroh::endpoint::RecvStream,
    check_len: impl FnOnce(usize) -> Result<(), Error>,
    decode: impl FnOnce(&[u8]) -> Result<F, Error>,
) -> Result<F, Error> {
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

/// Length-prefixed frame write shared by the side streams. `did_not_write`
/// names the stream that stayed shut.
pub(crate) async fn write_frame<F>(
    send: &mut iroh::endpoint::SendStream,
    frame: &F,
    encode: impl FnOnce(&F) -> Result<Vec<u8>, Error>,
    did_not_write: &'static str,
) -> Result<(), Error> {
    let bytes = encode(frame)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Error::network(did_not_write))?;
    Ok(())
}
