// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Sending pictures, the pointer and the host's feedback to a viewer.

use super::*;

/// Bytes of video a desktop may have waiting in the transport: three
/// frames at the agreed bitrate, and never under 64 KiB. Beyond it a new
/// frame would only add latency.
pub(super) fn backlog_budget(bitrate_kbps: u32, frame_rate: u8) -> usize {
    let per_frame = u64::from(bitrate_kbps) * 1000 / 8 / u64::from(frame_rate.max(1));
    usize::try_from(per_frame * 3)
        .unwrap_or(usize::MAX)
        .max(64 * 1024)
}

/// Whether a frame of `wire` bytes goes out now. It must fit the space the
/// transport has without dropping older datagrams, and, unless nothing is
/// waiting, keep the backlog within `budget`: a keyframe larger than the
/// budget still goes once the link has caught up.
pub(super) fn admits_frame(space: usize, wire: usize, budget: usize) -> bool {
    let queued = super::host::DATAGRAM_SEND_BUFFER.saturating_sub(space);
    wire <= space && (queued == 0 || queued + wire <= budget)
}

/// Which frames go out, and the keyframe owed after video is dropped.
///
/// The predicted frames after a dropped frame reference it, so they are
/// dropped too until a keyframe goes out. Every drop therefore costs a
/// keyframe, the largest frame. The owed keyframe is asked for once, and
/// only when the link could take it. Asked for while the backlog that
/// caused the drop remains, it would be dropped in turn and ask for
/// another: the encoder would make keyframe after keyframe, and the link
/// would carry few of them.
#[derive(Debug, Default)]
pub(super) struct SendGate {
    /// Frames are dropped until a keyframe goes out.
    awaiting: bool,
    /// The owed keyframe was asked for, and has not been sent or dropped.
    asked: bool,
    /// Wire size of the keyframe the next ask would bring, to judge when it
    /// would fit: the latest that answered an ask. A larger keyframe nobody
    /// asked for, a full-quality refresh or a periodic one, does not raise
    /// it: the one asked for is a recovery keyframe.
    keyframe_wire: usize,
    /// Backlog a keyframe just sent may hold on top of the budget, until
    /// the link drains back within it.
    draining: usize,
}

impl SendGate {
    /// Whether a frame is dropped without reaching the gate: a predicted
    /// frame while a keyframe is owed.
    pub(super) fn skips(&self, keyframe: bool) -> bool {
        self.awaiting && !keyframe
    }

    /// Whether a frame of `wire` bytes goes out now. The frames behind a
    /// keyframe larger than the budget wait for it whatever the gate does,
    /// and dropping one would only cost another keyframe: so they may add
    /// to its backlog until it drains back within the budget.
    pub(super) fn admits(&mut self, space: usize, wire: usize, budget: usize) -> bool {
        if super::host::DATAGRAM_SEND_BUFFER.saturating_sub(space) <= budget {
            self.draining = 0;
        }
        admits_frame(space, wire, budget.saturating_add(self.draining))
    }

    /// The gate dropped a frame. A dropped keyframe leaves the one owed
    /// still to ask for.
    pub(super) fn dropped(&mut self, keyframe: bool, wire: usize) {
        if keyframe {
            self.learn_keyframe(wire);
        }
        self.awaiting = true;
        self.asked = false;
    }

    /// Learns the size of a keyframe sent or dropped: as it is when it
    /// answered an ask or nothing is known yet, otherwise only if smaller.
    pub(super) fn learn_keyframe(&mut self, wire: usize) {
        self.keyframe_wire = if self.asked || self.keyframe_wire == 0 {
            wire
        } else {
            self.keyframe_wire.min(wire)
        };
    }

    /// A still picture went out. The frames behind it may wait for it as for
    /// a large keyframe; nothing owed changes.
    pub(super) fn still_sent(&mut self, wire: usize) {
        self.draining = self.draining.max(wire);
    }

    /// A frame went out; a keyframe pays what was owed.
    pub(super) fn sent(&mut self, keyframe: bool, wire: usize) {
        if keyframe {
            self.learn_keyframe(wire);
            self.awaiting = false;
            self.asked = false;
            self.draining = wire;
        }
    }

    /// Whether a keyframe is owed and not yet asked for.
    pub(super) fn owed(&self) -> bool {
        self.awaiting && !self.asked
    }

    /// Whether to ask the encoder for the owed keyframe now: the link has
    /// room for one as large as the latest.
    pub(super) fn should_ask(&mut self, space: usize, budget: usize) -> bool {
        if !self.owed() || !admits_frame(space, self.keyframe_wire, budget) {
            return false;
        }
        self.asked = true;
        true
    }
}

/// What the host says of itself in its feedback: its encoder, and whether
/// its screen is on a desktop it cannot capture.
pub(super) fn host_flags(shared: &Shared, desktop: &DesktopRuntime) -> u8 {
    use crate::adaptation::{ENCODER_HARDWARE, ENCODER_KNOWN, SECURE_DESKTOP};
    let encoder = match lock(&desktop.encoder).and_then(|encoder| encoder.hardware) {
        Some(true) => ENCODER_KNOWN | ENCODER_HARDWARE,
        Some(false) => ENCODER_KNOWN,
        None => 0,
    };
    let secure = if shared
        .secure_desktop
        .load(std::sync::atomic::Ordering::Acquire)
    {
        SECURE_DESKTOP
    } else {
        0
    };
    encoder | secure
}

/// The video send backlog allowed at the agreed quality.
pub(super) fn send_budget(desktop: &DesktopRuntime) -> usize {
    let quality = *lock(&desktop.quality);
    backlog_budget(quality.max_bitrate_kbps, quality.max_frame_rate)
}

/// The interval between frames at the agreed frame rate.
pub(super) fn frame_period(desktop: &DesktopRuntime) -> Duration {
    Duration::from_secs(1) / u32::from(lock(&desktop.quality).max_frame_rate.max(1))
}

/// What the host's sender keeps from one picture to the next.
struct PictureSender<'a> {
    shared: &'a Shared,
    desktop: &'a DesktopRuntime,
    link: iroh::endpoint::Connection,
    /// Groups each datagram picture's fragments, stills among them.
    seq: u32,
    /// The number the next stream picture takes, on either transport; a
    /// still repeats the one before, and a picture dropped here takes none.
    picture: u32,
    gate: SendGate,
    /// Whether the last picture went on the stream rather than as datagrams.
    streaming: bool,
    congestion: crate::video::Congestion,
    path: Option<iroh::endpoint::PathId>,
    /// The least backlog the gate saw since the last feedback.
    least_backlog: Option<usize>,
}

impl<'a> PictureSender<'a> {
    /// Sends what the encoder queued; whether any picture went out.
    async fn send_frames(
        &mut self,
        send: &mut SendStream,
        frames: Vec<crate::video::EncodedFrame>,
    ) -> Result<bool, Error> {
        let mut pictured = false;
        let last_change = frames
            .iter()
            .rposition(|frame| frame.role == crate::FrameRole::Changed);
        for (index, frame) in frames.into_iter().enumerate() {
            // A still picture is stale once a changed picture follows it,
            // and never goes while the stream owes a keyframe.
            let still = frame.role == crate::FrameRole::Still;
            let changed_after = last_change.is_some_and(|at| at > index);
            if still && (changed_after || self.gate.awaiting) {
                self.desktop.counters.still_dropped();
                continue;
            }
            // After a frame was dropped, the predicted frames that follow
            // reference it: they go too, until the keyframe owed for it.
            if self.gate.skips(frame.keyframe) {
                self.desktop.counters.send_dropped();
                continue;
            }
            pictured |= self.send_frame(send, &frame, still).await?;
        }
        Ok(pictured)
    }

    /// Reads the selected path and returns its round trip time. A new path
    /// starts its round trip floor afresh.
    fn track_path(&mut self) -> Duration {
        let selected = self
            .link
            .paths()
            .iter()
            .find(|candidate| candidate.is_selected())
            .map(|selected| (selected.id(), selected.stats()));
        let rtt = selected
            .as_ref()
            .map_or(Duration::ZERO, |(_, stats)| stats.rtt);
        if selected.as_ref().map(|(id, _)| *id) != self.path {
            self.path = selected.as_ref().map(|(id, _)| *id);
            self.congestion = crate::video::Congestion::default();
            lock(&self.desktop.link_rate).reset();
        }
        if let Some((_, stats)) = &selected {
            lock(&self.desktop.link_rate).observe(stats.cwnd, stats.rtt, std::time::Instant::now());
        }
        self.congestion.rtt(rtt, std::time::Instant::now());
        rtt
    }

    /// Sends one picture by the transport the link offers; whether it went.
    async fn send_frame(
        &mut self,
        send: &mut SendStream,
        frame: &crate::video::EncodedFrame,
        still: bool,
    ) -> Result<bool, Error> {
        let desktop = self.desktop;
        let rtt = self.track_path();
        let number = if still {
            self.picture.wrapping_sub(1)
        } else {
            self.picture
        };
        let body = crate::video::encode_video_body(frame, number);
        let max = self.link.max_datagram_size();
        #[cfg(any(test, feature = "test-probes"))]
        let max = if desktop
            .force_stream
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            None
        } else {
            max
        };
        if let Some(max) = max.filter(|max| *max > PREFIX + crate::video::FRAG_HEADER + 64) {
            self.streaming = false;
            desktop
                .datagram
                .store(max - PREFIX, std::sync::atomic::Ordering::Relaxed);
            self.send_datagrams(frame, still, &body, max, rtt)
        } else if still {
            if body.len() > desktop.still_drain() {
                desktop.counters.still_dropped();
                return Ok(false);
            }
            let mut bytes = vec![VIDEO];
            bytes.extend_from_slice(&body);
            write(send, &bytes).await?;
            desktop.counters.still_sent(frame.payload.len());
            Ok(true)
        } else {
            self.streaming = true;
            // The stream holds whatever is written to it: while the
            // viewer has not acknowledged the pictures already written,
            // this one is dropped whole and a keyframe follows once it
            // has.
            if !lock(&desktop.stream_flight).may_send(std::time::Instant::now()) {
                desktop.counters.send_dropped();
                self.gate.dropped(frame.keyframe, body.len());
                return Ok(false);
            }
            let mut bytes = vec![VIDEO];
            bytes.extend_from_slice(&body);
            write(send, &bytes).await?;
            lock(&desktop.stream_flight).sent(self.picture, std::time::Instant::now());
            self.picture = self.picture.wrapping_add(1);
            self.gate.sent(frame.keyframe, bytes.len());
            if frame.keyframe {
                desktop.counters.keyframe_sent(frame.payload.len());
            }
            Ok(true)
        }
    }

    /// Sends one picture as datagrams of at most `max` bytes.
    fn send_datagrams(
        &mut self,
        frame: &crate::video::EncodedFrame,
        still: bool,
        body: &[u8],
        max: usize,
        rtt: Duration,
    ) -> Result<bool, Error> {
        let desktop = self.desktop;
        let now = std::time::Instant::now();
        let protection = lock(&desktop.viewer_loss).protection(
            frame.keyframe,
            self.congestion.congested(rtt),
            now,
        );
        let datagrams = crate::video::fragment(body, self.seq, max - PREFIX, protection)
            .into_iter()
            .map(|fragment| {
                let mut bytes = Vec::with_capacity(PREFIX + fragment.len());
                bytes.extend_from_slice(&epoch(desktop.stamp));
                bytes.extend_from_slice(&fragment);
                bytes::Bytes::from(bytes)
            })
            .collect::<Vec<_>>();
        if datagrams.is_empty() {
            // Too large to reassemble: dropped whole, as a frame the link
            // cannot take.
            desktop.counters.send_dropped();
            self.gate.dropped(frame.keyframe, body.len());
            return Ok(false);
        }
        let wire: usize = datagrams.iter().map(bytes::Bytes::len).sum();
        let backlog = super::super::host::DATAGRAM_SEND_BUFFER
            .saturating_sub(self.link.datagram_send_buffer_space());
        self.least_backlog = Some(
            self.least_backlog
                .map_or(backlog, |least| least.min(backlog)),
        );
        if still {
            // A change behind it waits for what is queued and for
            // it: both must drain within the moment a still may
            // hold the link.
            let space = self.link.datagram_send_buffer_space();
            if wire > space || backlog + wire > desktop.still_drain() {
                desktop.counters.still_dropped();
                return Ok(false);
            }
            self.link
                .send_many_datagrams(&datagrams)
                .map_err(Error::network_debug)?;
            self.seq = self.seq.wrapping_add(1);
            self.gate.still_sent(wire);
            desktop.counters.still_sent(frame.payload.len());
            return Ok(true);
        }
        if dropped(self.shared, DropPoint::Gate)
            || !self.gate.admits(
                self.link.datagram_send_buffer_space(),
                wire,
                send_budget(desktop),
            )
        {
            // The link is behind: sending would queue stale video, or
            // make the transport drop older fragments and so split
            // frames. Drop this one whole and start again from a
            // keyframe once the backlog drains.
            desktop.counters.send_dropped();
            self.gate.dropped(frame.keyframe, wire);
            return Ok(false);
        }
        self.link
            .send_many_datagrams(&datagrams)
            .map_err(Error::network_debug)?;
        self.seq = self.seq.wrapping_add(1);
        self.picture = self.picture.wrapping_add(1);
        self.gate.sent(frame.keyframe, wire);
        if frame.keyframe {
            desktop.counters.keyframe_sent(frame.payload.len());
        }
        Ok(true)
    }

    /// Asks the encoder for the owed keyframe once the stream's
    /// unacknowledged pictures let it go, or the datagram backlog has room
    /// for it.
    fn ask_for_keyframe(&mut self) {
        let stream_blocked = self.streaming
            && !lock(&self.desktop.stream_flight).may_send(std::time::Instant::now());
        if !stream_blocked
            && self.gate.should_ask(
                self.link.datagram_send_buffer_space(),
                send_budget(self.desktop),
            )
        {
            self.desktop.keyframe.request();
        }
    }

    /// The host's feedback message for the viewer.
    fn feedback(&mut self) -> Vec<u8> {
        let desktop = self.desktop;
        let stats = self.link.stats();
        let queue = desktop.video.telemetry();
        let timing = lock(&desktop.producer_duration)
            .as_ref()
            .filter(|(_, _, at)| still_current(*at, std::time::Instant::now()))
            .map(|(capture, encode, _)| (*capture, *encode))
            .unwrap_or_default();
        // Read before the feedback is built: a guard held through its
        // fields would meet the one taken below.
        let link_kbps = lock(&desktop.link_rate)
            .bits_per_second()
            .map_or(0, |bits| (bits / 1000).min(u64::from(u32::MAX)) as u32);
        let feedback = crate::adaptation::HostFeedback {
            sent_bytes: stats.udp_tx.bytes,
            lost_bytes: stats.lost_bytes,
            dropped: queue.dropped,
            queued: queue.queued,
            oldest_ms: queue.oldest_ms,
            send_dropped: desktop.counters.read().send_dropped,
            backlog_ms: (self.least_backlog.take().unwrap_or(0) as u64 * 8)
                .checked_div(u64::from(lock(&desktop.quality).max_bitrate_kbps))
                .unwrap_or(0)
                .min(600_000),
            capture_us: timing.0,
            encode_us: timing.1,
            link_kbps,
            congestion_events: self
                .link
                .paths()
                .iter()
                .find(|path| path.is_selected())
                .map_or(0, |path| {
                    let stats = path.stats();
                    // A still screen sends little: read the link here too.
                    lock(&desktop.link_rate).observe(
                        stats.cwnd,
                        stats.rtt,
                        std::time::Instant::now(),
                    );
                    stats.congestion_events
                }),
            flags: host_flags(self.shared, desktop),
        };
        let mut bytes = vec![MEDIA_FEEDBACK];
        bytes.extend(feedback.encode());
        bytes
    }
}

/// Writes the pointer when it changed since `previous`.
async fn send_cursor(
    send: &mut SendStream,
    current: Option<Arc<CursorState>>,
    previous: &mut Option<Arc<CursorState>>,
) -> Result<(), Error> {
    if current != *previous {
        if let Some(current) = &current {
            let mut bytes = vec![CURSOR];
            bytes.extend_from_slice(&crate::cursor::encode(
                current,
                previous.as_ref().and_then(|c| c.image.as_ref()) != current.image.as_ref(),
            ));
            write(send, &bytes).await?;
        }
        *previous = current;
    }
    Ok(())
}

pub(super) async fn host_send(
    shared: &Shared,
    desktop: &DesktopRuntime,
    send: &mut SendStream,
) -> Result<(), Error> {
    let link = lock(&shared.state)
        .incoming
        .get(&desktop.info.service.connection_id)
        .and_then(|s| s.link.clone())
        .ok_or(Error::NoSession)?;
    let mut sender = PictureSender {
        shared,
        desktop,
        link,
        seq: 0,
        picture: 0,
        gate: SendGate::default(),
        streaming: false,
        congestion: crate::video::Congestion::default(),
        path: None,
        least_backlog: None,
    };
    let mut metrics = tokio::time::interval(Duration::from_secs(1));
    metrics.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cursor = desktop.cursor.subscribe();
    let mut previous = None;
    let mut topology_rx = shared.topology_watch.subscribe();
    let mut control_rx = desktop.control_state.subscribe();
    let mut recording_rx = desktop.recording_allowed.subscribe();
    let status = *control_rx.borrow_and_update();
    write_control_state(send, status).await?;
    if let Some(bytes) = topology_bytes(shared, desktop) {
        write(send, &bytes).await?;
    }
    loop {
        let notified = desktop.video.notified();
        tokio::pin!(notified);
        let frames = desktop.video.drain();
        let pictured = sender.send_frames(send, frames).await?;
        if pictured && control_rx.borrow().0 == DesktopControlState::Controlling {
            super::super::power::note_control_used(shared);
        }
        sender.ask_for_keyframe();
        // While a keyframe is owed, look at the backlog again each frame
        // period: a still screen may bring no frame to wake on.
        let owed_check = sender.gate.owed().then(|| frame_period(desktop));
        let current = cursor.borrow_and_update().clone();
        send_cursor(send, current, &mut previous).await?;
        tokio::select! {
            _ = metrics.tick() => {
                let bytes = sender.feedback();
                write(send, &bytes).await?;
            }
            _ = &mut notified => {},
            () = tokio::time::sleep(owed_check.unwrap_or_default()), if owed_check.is_some() => {},
            _ = cursor.changed() => {},
            changed = recording_rx.changed() => {
                changed.map_err(|_| Error::NoSession)?;
                let allowed = *recording_rx.borrow_and_update();
                let mut bytes = vec![RECORDING_STATE, u8::from(allowed.0)];
                bytes.extend_from_slice(&allowed.1.to_be_bytes());
                write(send, &bytes).await?;
            }
            changed = control_rx.changed() => {
                if changed.is_err() { return Err(Error::NoSession); }
                let status = *control_rx.borrow_and_update();
                write_control_state(send, status).await?;
            }
            changed = topology_rx.changed() => {
                if changed.is_err() {
                    return Err(Error::ShutDown);
                }
                if let Some(bytes) = topology_bytes(shared, desktop) {
                    write(send, &bytes).await?;
                }
            }
        }
    }
}

pub(super) fn topology_bytes(shared: &Shared, desktop: &DesktopRuntime) -> Option<Vec<u8>> {
    let snapshot = lock(&shared.topology).clone();
    if snapshot.revision == 0 {
        return None;
    }
    let mut bytes = vec![TOPOLOGY];
    bytes.extend_from_slice(
        &ControlMessage::Topology {
            stamp: desktop.stamp,
            revision: snapshot.revision,
            displays: snapshot.displays,
        }
        .encode(),
    );
    Some(bytes)
}
