// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! ScreenCast portal on the session bus.
//!
//! This is compiled on Linux only. The phase record in [`PortalSession`] is
//! what tests exercise. Calling [`open_monitor`] asks the compositor for one
//! monitor and does not claim that grant works on every desktop.
// trace: records every ignored failure

use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use ashpd::enumflags2::BitFlags;
use vnilink_media::Error;

use crate::grant::ScreenGrant;
use crate::portal::PortalSession;

/// A session-bus connection that lives on the caller's runtime.
///
/// ashpd otherwise shares one connection per process, and its socket reader
/// runs on the runtime that opened it first. Every portal open here runs on a
/// runtime of its own that ends with it, so a shared connection would die with
/// the first open and every later portal call would wait forever.
pub(crate) async fn session_bus() -> zbus::Result<zbus::Connection> {
    zbus::Connection::session().await
}

/// Create a portal session, select one monitor, and start it.
///
/// The returned file descriptor is the PipeWire remote. The node id is stored
/// on `session`. A failure leaves the local phase where the portal stopped.
/// With a `grant`, the portal keeps the consent until it is revoked: its
/// token is offered, so only the first start asks, and the token the start
/// answers with is kept for the next. Without one, every start asks.
pub async fn open_monitor(
    session: &mut PortalSession,
    grant: Option<&ScreenGrant>,
) -> Result<OwnedFd, Error> {
    let bus = session_bus()
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal could not be opened"))?;
    let proxy = Screencast::with_connection(bus)
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal could not be opened"))?;
    let remote = proxy
        .create_session(Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal could not be opened"))?;
    session.create_session()?;
    // ashpd 0.13 stores the portal reply before these futures resolve.
    // `Request::response` is synchronous.
    let token = grant.and_then(ScreenGrant::token);
    let sources = SelectSourcesOptions::default()
        .set_cursor_mode(CursorMode::Hidden)
        .set_sources(BitFlags::from(SourceType::Monitor))
        .set_multiple(false)
        .set_persist_mode(if grant.is_some() {
            PersistMode::ExplicitlyRevoked
        } else {
            PersistMode::DoNot
        })
        .set_restore_token(token.as_deref());
    let selected = proxy
        .select_sources(&remote, sources)
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal rejected the monitor"))?
        .response();
    refused(session, &selected)?;
    selected.map_err(|_| Error::CaptureRejected("the screen cast portal rejected the monitor"))?;
    session.select_monitor()?;
    let started = proxy
        .start(&remote, None, Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?
        .response();
    refused(session, &started)?;
    let started =
        started.map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))?;
    let stream = started.streams().first().ok_or(Error::CaptureRejected(
        "the screen cast portal did not start",
    ))?;
    if matches!(stream.source_type(), Some(kind) if kind != SourceType::Monitor) {
        vnilink::ignored!(
            vnilink::trace::CAPTURE,
            "a refused screen cast is closed at best",
            remote.close().await
        );
        return Err(Error::Unsupported("only one monitor is supported"));
    }
    let node = stream.pipe_wire_node_id();
    if let Some(token) = started.restore_token() {
        if !token.is_empty() {
            session.set_restore_token(token.to_string())?;
            if let Some(grant) = grant {
                // A grant that cannot be kept asks again next time.
                if let Err(error) = grant.keep(token) {
                    tracing::warn!(
                        target: vnilink::trace::CAPTURE,
                        failure = vnilink::trace::Failure::failure_name(&error),
                        reason = vnilink::trace::Failure::failure_reason(&error),
                        "the screenshot grant was not kept: the next screenshot asks again"
                    );
                }
            }
        }
    }
    session.start(node)?;
    proxy
        .open_pipe_wire_remote(&remote, Default::default())
        .await
        .map_err(|_| Error::CaptureRejected("the screen cast portal did not start"))
}

/// A request the portal answered with a refusal (`Response` code 1 or 2)
/// is a denial, not a failure: the session records it and capture says so.
fn refused<T>(
    session: &mut PortalSession,
    response: &Result<T, ashpd::Error>,
) -> Result<(), Error> {
    if let Err(ashpd::Error::Response(_)) = response {
        tracing::info!(target: vnilink::trace::CAPTURE, "the screen cast was declined on this computer");
        vnilink::ignored!(
            vnilink::trace::CAPTURE,
            "a declined session that cannot record it is declined anyway",
            session.refuse()
        );
        return Err(Error::CaptureDenied("the screen cast was not allowed"));
    }
    Ok(())
}
