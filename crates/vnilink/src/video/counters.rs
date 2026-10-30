// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Running counts for one desktop picture stream, for diagnostics: how
//! frames were lost, how often a keyframe was asked for and sent, and how
//! often the encoder was opened again. Counts only, never picture bytes.

use std::sync::atomic::{AtomicU64, Ordering};

use super::datagram::Losses;

#[derive(Default)]
pub(crate) struct VideoCounters {
    lost_expired: AtomicU64,
    lost_evicted: AtomicU64,
    lost_gap: AtomicU64,
    lost_stale: AtomicU64,
    keyframes_requested: AtomicU64,
    keyframes_sent: AtomicU64,
    keyframe_bytes: AtomicU64,
    encoder_reopens: AtomicU64,
    send_dropped: AtomicU64,
    stills_sent: AtomicU64,
    still_bytes: AtomicU64,
    stills_dropped: AtomicU64,
}

/// One reading of [`VideoCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VideoCounts {
    pub(crate) lost: Losses,
    pub(crate) keyframes_requested: u64,
    pub(crate) keyframes_sent: u64,
    pub(crate) keyframe_bytes: u64,
    pub(crate) encoder_reopens: u64,
    pub(crate) send_dropped: u64,
    pub(crate) stills_sent: u64,
    pub(crate) still_bytes: u64,
    pub(crate) stills_dropped: u64,
}

fn add(counter: &AtomicU64, value: u64) {
    counter.fetch_add(value, Ordering::Relaxed);
}

impl VideoCounters {
    /// Frames the viewer lost on the way.
    pub(crate) fn lost(&self, losses: Losses) {
        add(&self.lost_expired, losses.expired);
        add(&self.lost_evicted, losses.evicted);
        add(&self.lost_gap, losses.gap);
        add(&self.lost_stale, losses.stale);
    }

    /// The viewer asked the host for a keyframe.
    pub(crate) fn keyframe_requested(&self) {
        add(&self.keyframes_requested, 1);
    }

    /// The host sent a keyframe of `bytes` encoded bytes.
    pub(crate) fn keyframe_sent(&self, bytes: usize) {
        add(&self.keyframes_sent, 1);
        add(&self.keyframe_bytes, bytes as u64);
    }

    /// The host opened its encoder again for new settings.
    pub(crate) fn encoder_reopened(&self) {
        add(&self.encoder_reopens, 1);
    }

    /// The host dropped a frame rather than queue it behind a slow link.
    pub(crate) fn send_dropped(&self) {
        add(&self.send_dropped, 1);
    }

    /// The host sent a still picture of `bytes` encoded bytes.
    pub(crate) fn still_sent(&self, bytes: usize) {
        add(&self.stills_sent, 1);
        add(&self.still_bytes, bytes as u64);
    }

    /// The host left a still picture out: the link or the stream was busy.
    pub(crate) fn still_dropped(&self) {
        add(&self.stills_dropped, 1);
    }

    pub(crate) fn read(&self) -> VideoCounts {
        let read = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        VideoCounts {
            lost: Losses {
                expired: read(&self.lost_expired),
                evicted: read(&self.lost_evicted),
                gap: read(&self.lost_gap),
                stale: read(&self.lost_stale),
            },
            keyframes_requested: read(&self.keyframes_requested),
            keyframes_sent: read(&self.keyframes_sent),
            keyframe_bytes: read(&self.keyframe_bytes),
            encoder_reopens: read(&self.encoder_reopens),
            send_dropped: read(&self.send_dropped),
            stills_sent: read(&self.stills_sent),
            still_bytes: read(&self.still_bytes),
            stills_dropped: read(&self.stills_dropped),
        }
    }
}
