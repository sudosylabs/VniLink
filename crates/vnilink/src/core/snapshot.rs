// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Controller request and host admission for one screen snapshot.
//!
//! Kind byte 8 rides the existing service stream. The host checks view
//! permission again before it writes pixels. Closing the request finishes
//! only this stream.
// trace: records every ignored failure

use super::*;
use crate::snapshot::{
    validate_image, ScreenAccess, ScreenImage, ScreenSnapshot, SnapshotFailure, FRAME_LIMIT,
    MAX_SNAPSHOT_JOBS, SNAPSHOT_DEADLINE, SNAPSHOT_MIN_INTERVAL, WIRE_KIND,
};
use crate::ServiceKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const OUTCOME_IMAGE: u8 = 0;
const OUTCOME_PERMISSION: u8 = 1;
const OUTCOME_UNAVAILABLE: u8 = 2;
const OUTCOME_TIMEOUT: u8 = 3;
const OUTCOME_SOURCE: u8 = 4;
const FLAG_PROMPT: u8 = 1;

impl Core {
    pub fn set_snapshot_factory(
        &self,
        factory: Option<Arc<dyn crate::snapshot::HostSnapshotFactory>>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        drop(self.alive_state()?);
        *lock(&self.shared.snapshot_factory) = factory;
        Ok(())
    }

    /// One screenshot on an existing controller connection. The local service
    /// record is retired before this returns, so a later request is not Busy
    /// merely because the previous one finished. `cancel` stops delivery and
    /// returns [`ScreenSnapshot::Cancelled`], which is not a missing session.
    /// `prompt` asks the host to show its own consent or source dialog.
    pub fn request_screen_snapshot(
        &self,
        connection_id: u64,
        generation: u64,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenSnapshot, Error> {
        let _engine = self.engine();
        if cancel.load(Ordering::Acquire) {
            return Ok(ScreenSnapshot::Cancelled);
        }
        let owner =
            self.attach_controller_service(connection_id, generation, ServiceKind::ScreenSnapshot)?;
        let outcome = self.exchange_snapshot(connection_id, generation, owner.id, cancel, prompt);
        crate::ignored!(
            crate::trace::SNAPSHOT,
            "an owner already gone needs no close",
            self.close_controller_service(owner.id, owner.generation)
        );
        crate::ignored!(
            crate::trace::SNAPSHOT,
            "an owner already gone needs no close",
            self.forget_controller_service(owner.id)
        );
        if cancel.load(Ordering::Acquire) {
            return Ok(ScreenSnapshot::Cancelled);
        }
        outcome
    }

    /// Whether this host would admit a screenshot. Does not capture or rate-limit.
    pub fn screen_capture_access(
        &self,
        connection_id: u64,
        generation: u64,
    ) -> Result<ScreenAccess, Error> {
        let _engine = self.engine();
        let (link, stamp) = self.service_link(connection_id, generation)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.runtime.spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
                super::service_stream::write(
                    &mut send,
                    super::service_stream::preamble(stamp, 0, 0, WIRE_KIND),
                )
                .await?;
                let bytes = super::service_stream::read(&mut recv).await?;
                access_from_status(&bytes)
            })
            .await
            .map_err(|_| Error::Timeout)
            .and_then(|result| result);
            crate::ignored!(
                crate::trace::SNAPSHOT,
                "the caller stopped waiting for its answer",
                tx.send(result)
            );
        });
        rx.recv_timeout(Duration::from_secs(6))
            .map_err(|_| Error::Timeout)?
    }

    /// Whether [`Self::offer_attested_picture`] would take a picture
    /// attested `at`. A capture whose pixels cost a copy to read asks first.
    pub fn wants_attested_picture(&self, at: std::time::Instant) -> bool {
        let _engine = self.engine();
        crate::snapshot::tap_is_fresh(at, std::time::Instant::now())
            && !lock(&self.shared.snapshot_tap)
                .as_ref()
                .is_some_and(|existing| existing.at.elapsed() < Duration::from_secs(1))
    }

    /// Remember one freshly attested desktop frame for a later thumbnail.
    /// Idle retimestamps must not call this. At most one picture is kept.
    pub fn offer_attested_picture(
        &self,
        at: std::time::Instant,
        width: u16,
        height: u16,
        stride: usize,
        pixels: &[u8],
        display_id: &str,
    ) {
        let _engine = self.engine();
        if !crate::snapshot::tap_is_fresh(at, std::time::Instant::now()) {
            return;
        }
        let mut slot = lock(&self.shared.snapshot_tap);
        if slot
            .as_ref()
            .is_some_and(|existing| existing.at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let Ok(image) = crate::snapshot::encode_bgra(
            width,
            height,
            stride,
            pixels,
            crate::snapshot::unix_ms(),
            display_id,
        ) else {
            return;
        };
        *slot = Some(std::sync::Arc::new(AttestedShot { at, image }));
    }

    fn exchange_snapshot(
        &self,
        connection_id: u64,
        generation: u64,
        remote_id: u64,
        cancel: &Arc<AtomicBool>,
        prompt: bool,
    ) -> Result<ScreenSnapshot, Error> {
        let (link, stamp) = self.service_link(connection_id, generation)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let task_cancel = Arc::clone(cancel);
        self.runtime.spawn(async move {
            let exchange = async {
                tokio::time::timeout(SNAPSHOT_DEADLINE, async {
                    let (mut send, mut recv) =
                        link.open_bi().await.map_err(Error::network_debug)?;
                    super::service_stream::write(
                        &mut send,
                        super::service_stream::preamble(stamp, remote_id, 1, WIRE_KIND),
                    )
                    .await?;
                    super::service_stream::status(&super::service_stream::read(&mut recv).await?)?;
                    super::service_stream::write(&mut send, request_flags(prompt)).await?;
                    decode_payload(&read_payload(&mut recv).await?)
                })
                .await
                .map_err(|_| Error::Timeout)?
            };
            let cancel_wait = wait_cancel(task_cancel);
            let result = tokio::select! {
                biased;
                _ = cancel_wait => Ok(ScreenSnapshot::Cancelled),
                result = exchange => result,
            };
            crate::ignored!(
                crate::trace::SNAPSHOT,
                "the caller stopped waiting for its answer",
                tx.send(result)
            );
        });
        let deadline = Instant::now() + SNAPSHOT_DEADLINE + Duration::from_secs(1);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Ok(ScreenSnapshot::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(result) => {
                    return if cancel.load(Ordering::Acquire) {
                        Ok(ScreenSnapshot::Cancelled)
                    } else {
                        result
                    };
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return if cancel.load(Ordering::Acquire) {
                        Ok(ScreenSnapshot::Cancelled)
                    } else {
                        Err(Error::NoSession)
                    };
                }
            }
        }
    }
}

async fn wait_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(crate) fn snapshot_admitted(active_jobs: usize, since_last: Option<Duration>) -> bool {
    active_jobs < MAX_SNAPSHOT_JOBS
        && since_last.is_none_or(|elapsed| elapsed >= SNAPSHOT_MIN_INTERVAL)
}

pub(super) fn snapshot_room(state: &State, peer: &[u8; 32], now: Instant) -> bool {
    let jobs = state
        .hosted_services
        .values()
        .filter(|record| {
            record.info.kind == ServiceKind::ScreenSnapshot && !record.info.phase.finished()
        })
        .count();
    let since_last = state
        .snapshot_recent
        .get(peer)
        .map(|last| now.saturating_duration_since(*last));
    snapshot_admitted(jobs, since_last)
}

pub(super) fn note_snapshot(state: &mut State, peer: &[u8; 32], now: Instant) {
    state
        .snapshot_recent
        .retain(|_, last| now.saturating_duration_since(*last) < Duration::from_secs(60));
    if state.snapshot_recent.len() >= 64 && !state.snapshot_recent.contains_key(peer) {
        if let Some(oldest) = state
            .snapshot_recent
            .iter()
            .min_by_key(|(_, seen)| *seen)
            .map(|(key, _)| *key)
        {
            state.snapshot_recent.remove(&oldest);
        }
    }
    state.snapshot_recent.insert(*peer, now);
}

pub(super) struct AttestedShot {
    at: Instant,
    image: ScreenImage,
}

fn request_flags(prompt: bool) -> Vec<u8> {
    vec![1, if prompt { FLAG_PROMPT } else { 0 }]
}

fn read_prompt(bytes: &[u8]) -> bool {
    bytes.len() == 2 && bytes[0] == 1 && bytes[1] & FLAG_PROMPT != 0
}

fn access_from_status(bytes: &[u8]) -> Result<ScreenAccess, Error> {
    if bytes.len() != 3 || bytes[0] != super::service_stream::VERSION {
        return Err(Error::IncompatibleVersion);
    }
    match bytes[1] {
        0 => Ok(ScreenAccess::Allowed),
        6 => Ok(ScreenAccess::Denied),
        1 => Ok(ScreenAccess::Unavailable),
        3 => Err(Error::StaleSession),
        7 => Err(Error::NoSession),
        _ => Err(Error::IncompatibleVersion),
    }
}

pub(super) async fn probe(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    mut send: iroh::endpoint::SendStream,
    stamp: crate::protocol::SessionStamp,
) -> Result<(), Error> {
    let status = {
        let state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if session.wire_stamp != stamp || session.peer_key != peer {
            3
        } else if !super::service_permissions::admitted(&state, &peer, ServiceKind::ScreenSnapshot)
        {
            6
        } else if lock(&shared.snapshot_factory).is_none() {
            1
        } else {
            0
        }
    };
    super::service_stream::write(&mut send, vec![super::service_stream::VERSION, status, 0])
        .await?;
    crate::ignored!(
        crate::trace::SNAPSHOT,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(())
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    peer: [u8; 32],
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
    mut service_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    super::service_stream::write(&mut send, vec![super::service_stream::VERSION, 0, 0]).await?;
    let request = super::service_stream::read(&mut recv).await;
    let Ok(request) = request else {
        crate::ignored!(
            crate::trace::SNAPSHOT,
            "a stream that cannot finish is closed with its link",
            send.finish()
        );
        return Ok(());
    };
    let prompt = read_prompt(&request);
    if !prompt {
        let shot = lock(&shared.snapshot_tap).clone();
        if let Some(shot) = shot {
            if crate::snapshot::tap_is_fresh(shot.at, Instant::now())
                && crate::snapshot::validate_image(&shot.image).is_ok()
                && still_allowed(&shared, &peer)
            {
                write_payload(&mut send, &encode_image(&shot.image)).await?;
                crate::ignored!(
                    crate::trace::SNAPSHOT,
                    "a stream that cannot finish is closed with its link",
                    send.finish()
                );
                return Ok(());
            }
        }
    }
    let factory = lock(&shared.snapshot_factory).clone();
    let Some(factory) = factory else {
        write_outcome(&mut send, OUTCOME_UNAVAILABLE).await?;
        crate::ignored!(
            crate::trace::SNAPSHOT,
            "a stream that cannot finish is closed with its link",
            send.finish()
        );
        return Ok(());
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let deadline = Instant::now() + SNAPSHOT_DEADLINE;
    let mut task = tokio::task::spawn_blocking(move || factory.capture(deadline, &flag, prompt));
    let mut ignored = [0u8; 1];
    // Caller stops drop the picture. The deadline still reports TimedOut
    // when the capture itself does not return an image in time.
    let mut stop = Stop::None;
    if *parent_stop.borrow() || *service_stop.borrow() {
        cancel.store(true, Ordering::Release);
        stop = Stop::Caller;
    }
    let captured = loop {
        tokio::select! {
            biased;
            _ = parent_stop.changed(), if stop == Stop::None => {
                cancel.store(true, Ordering::Release);
                stop = Stop::Caller;
            }
            _ = service_stop.changed(), if stop == Stop::None => {
                cancel.store(true, Ordering::Release);
                stop = Stop::Caller;
            }
            _ = recv.read(&mut ignored), if stop == Stop::None => {
                cancel.store(true, Ordering::Release);
                stop = Stop::Caller;
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)), if stop == Stop::None => {
                cancel.store(true, Ordering::Release);
                stop = Stop::Deadline;
            }
            _ = tokio::time::sleep(Duration::from_secs(2)), if stop != Stop::None => {
                break Err(SnapshotFailure::Cancelled);
            }
            result = &mut task => break result.unwrap_or(Err(SnapshotFailure::Unavailable)),
        }
    };
    if stop == Stop::Caller
        || *parent_stop.borrow()
        || *service_stop.borrow()
        || !still_allowed(&shared, &peer)
    {
        crate::ignored!(
            crate::trace::SNAPSHOT,
            "a stream that cannot finish is closed with its link",
            send.finish()
        );
        return Ok(());
    }
    let outcome = match captured {
        Ok(image) if stop == Stop::None && validate_image(&image).is_ok() => encode_image(&image),
        Ok(_) if stop == Stop::Deadline => vec![1, OUTCOME_TIMEOUT],
        Ok(_) => vec![1, OUTCOME_UNAVAILABLE],
        Err(SnapshotFailure::NeedsPermission) => vec![1, OUTCOME_PERMISSION],
        Err(SnapshotFailure::NeedsSourceSelection) => vec![1, OUTCOME_SOURCE],
        Err(SnapshotFailure::TimedOut) => vec![1, OUTCOME_TIMEOUT],
        Err(SnapshotFailure::Cancelled) if stop == Stop::Deadline => vec![1, OUTCOME_TIMEOUT],
        Err(SnapshotFailure::Cancelled) => {
            crate::ignored!(
                crate::trace::SNAPSHOT,
                "a stream that cannot finish is closed with its link",
                send.finish()
            );
            return Ok(());
        }
        Err(SnapshotFailure::Unavailable) => vec![1, OUTCOME_UNAVAILABLE],
    };
    write_payload(&mut send, &outcome).await?;
    crate::ignored!(
        crate::trace::SNAPSHOT,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    None,
    Caller,
    Deadline,
}

fn still_allowed(shared: &Shared, peer: &[u8; 32]) -> bool {
    let state = lock(&shared.state);
    super::service_permissions::admitted(&state, peer, ServiceKind::ScreenSnapshot)
}

async fn write_outcome(send: &mut iroh::endpoint::SendStream, outcome: u8) -> Result<(), Error> {
    write_payload(send, &[1, outcome]).await
}

async fn write_payload(send: &mut iroh::endpoint::SendStream, payload: &[u8]) -> Result<(), Error> {
    let payload = payload.to_vec();
    crate::sidestream::write_frame(
        send,
        &payload,
        |bytes: &Vec<u8>| {
            if bytes.len() > FRAME_LIMIT {
                return Err(Error::Malformed);
            }
            let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
            framed.extend_from_slice(bytes);
            Ok(framed)
        },
        "the snapshot was not written",
    )
    .await
}

async fn read_payload(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>, Error> {
    crate::sidestream::read_frame(
        recv,
        |len| {
            if len < 2 || len > FRAME_LIMIT {
                Err(Error::Malformed)
            } else {
                Ok(())
            }
        },
        |bytes| Ok(bytes.to_vec()),
    )
    .await
}

fn encode_image(image: &ScreenImage) -> Vec<u8> {
    let id = image.display_id.as_bytes();
    let mut payload = Vec::with_capacity(14 + id.len() + image.jpeg.len());
    payload.push(1);
    payload.push(OUTCOME_IMAGE);
    payload.extend_from_slice(&image.width.to_be_bytes());
    payload.extend_from_slice(&image.height.to_be_bytes());
    payload.extend_from_slice(&image.captured_unix_ms.to_be_bytes());
    payload.extend_from_slice(&(id.len() as u16).to_be_bytes());
    payload.extend_from_slice(id);
    payload.extend_from_slice(&image.jpeg);
    payload
}

pub(crate) fn decode_payload(bytes: &[u8]) -> Result<ScreenSnapshot, Error> {
    if bytes.len() < 2 || bytes[0] != 1 {
        return Err(Error::Malformed);
    }
    // Only an image follows its outcome byte.
    if bytes[1] != OUTCOME_IMAGE && bytes.len() != 2 {
        return Err(Error::Malformed);
    }
    match bytes[1] {
        OUTCOME_PERMISSION => Ok(ScreenSnapshot::NeedsPermission),
        OUTCOME_SOURCE => Ok(ScreenSnapshot::NeedsSourceSelection),
        OUTCOME_UNAVAILABLE => Ok(ScreenSnapshot::Unavailable),
        OUTCOME_TIMEOUT => Ok(ScreenSnapshot::TimedOut),
        OUTCOME_IMAGE => decode_image(&bytes[2..]).map(ScreenSnapshot::Ready),
        _ => Err(Error::Malformed),
    }
}

/// The payload the host writes for `snapshot`: what [`decode_payload`]
/// reads. A cancelled snapshot never crosses the wire.
#[cfg(feature = "fuzzing")]
pub(crate) fn encode_payload(snapshot: &ScreenSnapshot) -> Vec<u8> {
    let outcome = match snapshot {
        ScreenSnapshot::Ready(image) => return encode_image(image),
        ScreenSnapshot::NeedsPermission => OUTCOME_PERMISSION,
        ScreenSnapshot::NeedsSourceSelection => OUTCOME_SOURCE,
        ScreenSnapshot::Unavailable => OUTCOME_UNAVAILABLE,
        ScreenSnapshot::TimedOut => OUTCOME_TIMEOUT,
        ScreenSnapshot::Cancelled => unreachable!("a cancelled snapshot is never written"),
    };
    vec![1, outcome]
}

fn decode_image(bytes: &[u8]) -> Result<ScreenImage, Error> {
    if bytes.len() < 14 {
        return Err(Error::Malformed);
    }
    let width = u16::from_be_bytes(bytes[0..2].try_into().unwrap());
    let height = u16::from_be_bytes(bytes[2..4].try_into().unwrap());
    let captured_unix_ms = u64::from_be_bytes(bytes[4..12].try_into().unwrap());
    let id_len = usize::from(u16::from_be_bytes(bytes[12..14].try_into().unwrap()));
    let rest = bytes.get(14..).ok_or(Error::Malformed)?;
    if id_len > 64 || rest.len() < id_len {
        return Err(Error::Malformed);
    }
    let display_id = std::str::from_utf8(&rest[..id_len])
        .map_err(|_| Error::Malformed)?
        .to_owned();
    let jpeg = rest[id_len..].to_vec();
    let image = ScreenImage {
        jpeg,
        width,
        height,
        captured_unix_ms,
        display_id,
    };
    validate_image(&image).map_err(|_| Error::Malformed)?;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by the `snapshot_payload` fuzz target: an outcome with bytes after
    /// it decoded, though the host writes none.
    #[test]
    fn an_outcome_with_trailing_bytes_is_malformed() {
        assert!(matches!(
            decode_payload(&[1, 1]),
            Ok(ScreenSnapshot::NeedsPermission)
        ));
        assert!(matches!(
            decode_payload(&[1, 1, 0x37, 0x37]),
            Err(Error::Malformed)
        ));
    }

    #[test]
    fn admission_bounds_jobs_and_spacing() {
        assert!(snapshot_admitted(0, None));
        assert!(snapshot_admitted(
            MAX_SNAPSHOT_JOBS - 1,
            Some(SNAPSHOT_MIN_INTERVAL)
        ));
        assert!(!snapshot_admitted(MAX_SNAPSHOT_JOBS, None));
        assert!(!snapshot_admitted(
            0,
            Some(SNAPSHOT_MIN_INTERVAL - Duration::from_millis(1))
        ));
    }
}
