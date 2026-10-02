//! ScreenCast portal on the session bus.
//!
//! This is compiled on Linux only. The phase record in [`PortalSession`] is
//! what tests exercise. Calling [`open_monitor`] asks the compositor for one
//! monitor and does not claim that grant works on every desktop.

use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use ashpd::enumflags2::BitFlags;
use vnilink_media::Error;

use crate::portal::PortalSession;

/// Create a portal session, select one monitor, and start it.
///
/// The returned file descriptor is the PipeWire remote. The node id is stored
/// on `session`. A failure leaves the local phase where the portal stopped.
pub async fn open_monitor(session: &mut PortalSession) -> Result<OwnedFd, Error> {
    let proxy = Screencast::new()
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal could not be opened"))?;
    let remote = proxy
        .create_session(Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal could not be opened"))?;
    session.create_session()?;
    // ashpd 0.13 stores the portal reply before these futures resolve.
    // `Request::response` is synchronous.
    let sources = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Hidden)
        .set_sources(BitFlags::from(SourceType::Monitor))
        .set_multiple(false)
        .set_persist_mode(PersistMode::DoNot);
    proxy
        .select_sources(&remote, sources)
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal rejected the monitor"))?
        .response()
        .map_err(|_| Error::CaptureRejected("the screen cast portal rejected the monitor"))?;
    session.select_monitor()?;
    let started = proxy
        .start(&remote, None, Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?
        .response()
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    let stream = started.streams().first().ok_or(Error::CaptureRejected(
        "the screen cast portal did not start",
    ))?;
    if matches!(stream.source_type(), Some(kind) if kind != SourceType::Monitor) {
        let _ = remote.close().await;
        return Err(Error::Unsupported("only one monitor is supported"));
    }
    let node = stream.pipe_wire_node_id();
    if let Some(token) = started.restore_token() {
        if !token.is_empty() {
            session.set_restore_token(token.to_string())?;
        }
    }
    session.start(node)?;
    proxy
        .open_pipe_wire_remote(&remote, Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))
}
