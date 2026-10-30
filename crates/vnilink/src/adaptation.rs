// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Measured per-Desktop adaptation. Only local monotonic intervals are used;
//! remote capture timestamps are not assumed synchronized with this computer.
use crate::bitrate::{self, RateLimit, FLOOR_KBPS, RELAY_KBPS};
use crate::protocol::{MAX_BITRATE_KBPS, MAX_EDGE};
use crate::{Quality, VideoCodec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdaptationReason {
    #[default]
    WarmingUp,
    Stable,
    ReceiverPressure,
    EncoderPressure,
    NetworkPressure,
    Recovering,
    Idle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopMediaStatus {
    pub reason: AdaptationReason,
    pub received_kbps: u32,
    pub received_fps: u16,
    pub receiver_queue_ms: u64,
    pub host_queue_ms: u64,
    pub send_loss_per_mille: u16,
    pub rtt_ms: u64,
    pub decode_us: u64,
    pub encode_us: u64,
    pub capture_us: u64,
    /// What most holds the bitrate where it is.
    pub limit: RateLimit,
    /// What is wrong with the picture now, from the most severe; see
    /// [`crate::Condition`].
    pub conditions: crate::ConditionSet,
}
/// Encoded size of a [`HostFeedback`].
const HOST_FEEDBACK_LEN: usize = 79;

/// [`HostFeedback::flags`]: the host knows whether its open encoder is
/// hardware, and the bit after says which.
pub(crate) const ENCODER_KNOWN: u8 = 1;
pub(crate) const ENCODER_HARDWARE: u8 = 2;
/// The host's screen is on a desktop it cannot capture.
pub(crate) const SECURE_DESKTOP: u8 = 4;
const HOST_FLAGS: u8 = ENCODER_KNOWN | ENCODER_HARDWARE | SECURE_DESKTOP;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HostFeedback {
    pub sent_bytes: u64,
    pub lost_bytes: u64,
    pub dropped: u64,
    /// Frames the host's send gate dropped rather than queue behind the link.
    pub send_dropped: u64,
    /// The least video waiting in the transport over the interval, in
    /// milliseconds at the agreed bitrate: a queue the round trip does not
    /// show, and that never drains while the link is slower than the video.
    pub backlog_ms: u64,
    pub queued: u16,
    pub oldest_ms: u64,
    pub encode_us: u64,
    pub capture_us: u64,
    /// What the host's link carries now, in kb/s, as its congestion window
    /// over the round trip it showed under load; 0 before a reading.
    pub link_kbps: u32,
    /// Congestion events on the host's selected path so far: its sending
    /// side of the link, which the viewer's own path does not see.
    pub congestion_events: u64,
    /// [`ENCODER_KNOWN`], [`ENCODER_HARDWARE`] and [`SECURE_DESKTOP`].
    pub flags: u8,
}
impl HostFeedback {
    /// The host's own state, from [`Self::flags`].
    pub fn host_state(&self) -> crate::video::HostState {
        crate::video::HostState {
            encoder_hardware: (self.flags & ENCODER_KNOWN != 0)
                .then_some(self.flags & ENCODER_HARDWARE != 0),
            secure_desktop: self.flags & SECURE_DESKTOP != 0,
        }
    }
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HOST_FEEDBACK_LEN);
        for value in [
            self.sent_bytes,
            self.lost_bytes,
            self.dropped,
            self.oldest_ms,
            self.encode_us,
            self.capture_us,
            self.send_dropped,
            self.backlog_ms,
            self.congestion_events,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&self.queued.to_be_bytes());
        bytes.extend_from_slice(&self.link_kbps.to_be_bytes());
        bytes.push(self.flags);
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, crate::Error> {
        if bytes.len() != HOST_FEEDBACK_LEN {
            return Err(crate::Error::Malformed);
        }
        let feedback = Self {
            sent_bytes: u64::from_be_bytes(bytes[0..8].try_into().unwrap()),
            lost_bytes: u64::from_be_bytes(bytes[8..16].try_into().unwrap()),
            dropped: u64::from_be_bytes(bytes[16..24].try_into().unwrap()),
            oldest_ms: u64::from_be_bytes(bytes[24..32].try_into().unwrap()),
            encode_us: u64::from_be_bytes(bytes[32..40].try_into().unwrap()),
            capture_us: u64::from_be_bytes(bytes[40..48].try_into().unwrap()),
            send_dropped: u64::from_be_bytes(bytes[48..56].try_into().unwrap()),
            backlog_ms: u64::from_be_bytes(bytes[56..64].try_into().unwrap()),
            congestion_events: u64::from_be_bytes(bytes[64..72].try_into().unwrap()),
            queued: u16::from_be_bytes(bytes[72..74].try_into().unwrap()),
            link_kbps: u32::from_be_bytes(bytes[74..78].try_into().unwrap()),
            flags: bytes[78],
        };
        if feedback.queued > 8
            || feedback.flags & !HOST_FLAGS != 0
            || (feedback.flags & ENCODER_HARDWARE != 0 && feedback.flags & ENCODER_KNOWN == 0)
            || feedback.encode_us > 5_000_000
            || feedback.capture_us > 5_000_000
            || feedback.backlog_ms > 600_000
        {
            return Err(crate::Error::Malformed);
        }
        Ok(feedback)
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Feedback {
    pub receiver: crate::video::QueueTelemetry,
    pub host: Option<HostFeedback>,
    /// Frames the viewer lost on the way, after any repair from parity.
    pub frames_lost: u64,
    pub rtt_ms: u64,
    pub path_changed: bool,
    /// The viewer decoder's time per picture while others wait for it: its
    /// throughput, not how long one picture takes to come out.
    pub decode_us: Option<u64>,
}
/// Share of frames missed, per mille, that strains the link.
pub(crate) const MISSED_PRESSURE: u64 = 50;

/// Share of frames missed, per mille, at which the bitrate halves.
const MISSED_SEVERE: u64 = 150;

/// How long frames must go missing at the viewer before it is the link.
const LOSS_LASTS_MS: u64 = 3000;

/// Healthy time before stepping back up towards the bitrate in use before
/// the strain, and before the frame rate and size grow back.
const RETURN_AFTER_MS: u64 = 3000;
const PROBE_AFTER_MS: u64 = 6000;

/// Healthy time before the bitrate probes above a rate it knows, and the
/// longest a probe that failed waits before the next.
const BITRATE_PROBE_MS: u64 = 3000;
const BITRATE_PROBE_MAX_MS: u64 = 24_000;

/// A probe is judged by the round trip this long after it, and what it
/// learnt is kept for as long again.
const PROBE_JUDGED_MS: u64 = 4000;

/// Round trips kept to judge the next by: ten seconds of them.
const RECENT_RTTS: usize = 10;

/// How far the round trip may rise above its recent floor with the stream
/// still at home on the link, and how far it may rise after a probe before the
/// probe is given back, in milliseconds at least.
const NEAR_FLOOR_MS: u64 = 15;
const PROBE_RTT_RISE_MS: u64 = 25;

/// Share of the picture budget at which the encoder or decoder stops the
/// bitrate rising: the pressure threshold is three quarters of the
/// picture's interval, and this is four fifths of that.
const HOT_US_A_SECOND: u64 = 600_000;

/// Time the stream must be strained before it is cut: two samples, or the
/// first when the strain is severe.
const STRAIN_PATIENCE_MS: u64 = 2000;

/// A rate recovery got back to and held this long is proven again.
const RESTORE_PROVEN_MS: u64 = 10_000;

/// What one step measured, before deciding: the controller's own inputs
/// and verdicts, so [`crate::video::Conditions`] explains the picture from
/// exactly what the controller saw. Reading them changes no decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Signals {
    /// The host's send loss over the interval, per mille of bytes.
    pub loss_per_mille: u16,
    /// How long the viewer has been missing frames without a break.
    pub lasting_loss_ms: u64,
    /// Frames lost on the way or dropped by the host's send gate, per mille.
    pub missed_per_mille: u64,
    /// The host's send gate dropped frames behind the link.
    pub gate_dropping: bool,
    pub backlog_ms: u64,
    /// The host's encode queue is old and deep.
    pub host_queue_backed_up: bool,
    pub host_queue_ms: u64,
    pub rtt_ms: u64,
    pub rtt_floor_ms: u64,
    /// The round trip at and above which it counts as inflated.
    pub inflated_rtt_ms: u64,
    /// Congestion events on the host's path over the interval.
    pub congestion_events: u64,
    pub encoder_pressure: bool,
    pub encode_us: u64,
    /// This computer's decoder takes longer than a picture's interval.
    pub decoder_pressure: bool,
    pub decode_us: u64,
    /// Pictures wait, or are dropped, before this computer's decoder.
    pub receiver_backed_up: bool,
    pub receiver_queue_ms: u64,
    /// The time a picture may take at the current frame rate, at the
    /// three-quarters the controller allows a coder.
    pub picture_budget_us: u64,
}
/// What the controller may do with a stream, from the viewer and the
/// session: the limits, and what the bitrate is derived from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// What the viewer chose. Its frame rate and size are ceilings, and its
    /// bitrate the most the stream may use, not the rate it asks for.
    pub ceiling: Quality,
    pub codec: VideoCodec,
    /// The host's monitor in pixels; zero when unknown.
    pub monitor: (u16, u16),
    /// The selected path goes through a relay.
    pub relayed: bool,
}
impl From<Quality> for Limits {
    fn from(ceiling: Quality) -> Self {
        Self {
            ceiling,
            codec: VideoCodec::H264,
            monitor: (0, 0),
            relayed: false,
        }
    }
}

/// A bound on the bitrate and what set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bound {
    kbps: u32,
    by: RateLimit,
}
impl Default for Bound {
    fn default() -> Self {
        Self {
            kbps: u32::MAX,
            by: RateLimit::None,
        }
    }
}

/// A rate tried above the last known good one.
#[derive(Debug, Clone, Copy)]
struct Probe {
    /// The rate before it.
    from: u32,
    age_ms: u64,
    /// The round trip before it.
    rtt_ms: u64,
}

pub(crate) struct Adaptive {
    previous: Option<Feedback>,
    healthy_ms: u64,
    strained_ms: u64,
    /// How long the viewer has been missing frames without a break.
    lossy_ms: u64,
    cooldown_ms: u64,
    rtt_floor: u64,
    /// The last few round trips: a link whose round trip stays up under
    /// the stream's own load, as a shared radio's does, is judged against
    /// that, not against the round trip it showed idle.
    recent_rtts: std::collections::VecDeque<u64>,
    /// The bitrate in use when the link last strained: recovery returns to
    /// it sooner than it probes above it.
    before_strain: Option<u32>,
    /// How long since recovery got back to `before_strain`; strain within
    /// [`RESTORE_PROVEN_MS`] of it says that rate was too much after all.
    since_restore: Option<u64>,
    /// Whether the link has strained since the stream started: until it
    /// has, the bitrate climbs in large steps.
    strain_seen: bool,
    probe: Option<Probe>,
    /// Healthy time before the next probe: longer after each that failed.
    probe_wait_ms: u64,
    /// The lowest bound on the bitrate, and the lowest including what the
    /// link was measured to carry, which only stops it rising.
    bound: Bound,
    raise_to: Bound,
    /// The encoder or the decoder is near its budget.
    hot: RateLimit,
    pub status: DesktopMediaStatus,
    /// The last step's measurements; `None` while warming up.
    pub signals: Option<Signals>,
}
impl Default for Adaptive {
    fn default() -> Self {
        Self {
            previous: None,
            healthy_ms: 0,
            strained_ms: 0,
            lossy_ms: 0,
            cooldown_ms: 0,
            rtt_floor: 0,
            recent_rtts: std::collections::VecDeque::new(),
            before_strain: None,
            since_restore: None,
            strain_seen: false,
            probe: None,
            probe_wait_ms: BITRATE_PROBE_MS,
            bound: Bound::default(),
            raise_to: Bound::default(),
            hot: RateLimit::None,
            status: DesktopMediaStatus::default(),
            signals: None,
        }
    }
}
impl Adaptive {
    /// One sampling interval. `limits` are what the viewer chose, as the
    /// host agreed them, and what the session is on: recovery climbs back
    /// toward the frame rate and size and never past them, and the bitrate
    /// follows the picture and the link up to the viewer's maximum.
    pub fn step(
        &mut self,
        current: Quality,
        limits: impl Into<Limits>,
        elapsed_ms: u64,
        next: Feedback,
    ) -> Option<Quality> {
        let decided = self.decide(current, &limits.into(), elapsed_ms, next);
        self.status.limit = if self.signals.is_some() {
            self.attribute(decided.unwrap_or(current).max_bitrate_kbps)
        } else {
            RateLimit::None
        };
        decided
    }

    /// What holds a bitrate where it is: the bound it is at, the coder that
    /// is near its budget, or a strain the link showed lately.
    fn attribute(&self, rate: u32) -> RateLimit {
        if rate.saturating_add(rate / 50) >= self.raise_to.kbps {
            return self.raise_to.by;
        }
        if self.hot != RateLimit::None {
            return self.hot;
        }
        if self.before_strain.is_some() || self.probe_wait_ms > BITRATE_PROBE_MS {
            return RateLimit::Network;
        }
        RateLimit::None
    }

    fn decide(
        &mut self,
        current: Quality,
        limits: &Limits,
        elapsed_ms: u64,
        next: Feedback,
    ) -> Option<Quality> {
        let ceiling = limits.ceiling;
        let previous = self.previous.replace(next);
        self.signals = None;
        let Some(previous) = previous else {
            self.rtt_floor = next.rtt_ms.max(1);
            return None;
        };
        // Counter reset, route change or a suspended sampler must warm up afresh.
        if next.path_changed
            || elapsed_ms == 0
            || elapsed_ms > 10_000
            || next.receiver.received_bytes < previous.receiver.received_bytes
            || next.receiver.received_frames < previous.receiver.received_frames
        {
            self.healthy_ms = 0;
            self.strained_ms = 0;
            self.lossy_ms = 0;
            self.cooldown_ms = 0;
            self.before_strain = None;
            self.since_restore = None;
            self.probe = None;
            self.probe_wait_ms = BITRATE_PROBE_MS;
            self.rtt_floor = next.rtt_ms.max(1);
            self.recent_rtts.clear();
            self.status.reason = AdaptationReason::WarmingUp;
            return None;
        }
        let frames = next
            .receiver
            .received_frames
            .saturating_sub(previous.receiver.received_frames);
        let bytes = next
            .receiver
            .received_bytes
            .saturating_sub(previous.receiver.received_bytes);
        let drops = next
            .receiver
            .dropped
            .saturating_sub(previous.receiver.dropped);
        let loss = match (next.host, previous.host) {
            (Some(now), Some(before))
                if now.sent_bytes >= before.sent_bytes && now.lost_bytes >= before.lost_bytes =>
            {
                let sent = now.sent_bytes - before.sent_bytes;
                let lost = now.lost_bytes - before.lost_bytes;
                if sent >= 4096 {
                    lost.saturating_mul(1000)
                        .checked_div(sent)
                        .unwrap_or(0)
                        .min(1000) as u16
                } else {
                    0
                }
            }
            _ => 0,
        };
        // Frames that never reached the viewer: lost on the way after any
        // repair, or dropped by the host's send gate behind the link.
        let viewer_lost = next.frames_lost.saturating_sub(previous.frames_lost);
        let gate_dropped = match (next.host, previous.host) {
            (Some(now), Some(before)) => now.send_dropped.saturating_sub(before.send_dropped),
            _ => 0,
        };
        let per_mille = |missed: u64| {
            missed
                .saturating_mul(1000)
                .checked_div(frames.saturating_add(missed))
                .unwrap_or(0)
        };
        let missing = |missed: u64| missed >= 2 && per_mille(missed) >= MISSED_PRESSURE;
        // A blackout of a fraction of a second loses many frames at once
        // but says nothing of the link's rate: loss at the viewer counts
        // only once it lasts. The host dropping frames behind the link
        // is the link falling short, and counts as soon as any strain.
        self.lossy_ms = if missing(viewer_lost) {
            self.lossy_ms.saturating_add(elapsed_ms)
        } else {
            0
        };
        let missed_per_mille = per_mille(viewer_lost.saturating_add(gate_dropped));
        self.rtt_floor = self.rtt_floor.min(next.rtt_ms.max(1));
        if self.recent_rtts.len() == RECENT_RTTS {
            self.recent_rtts.pop_front();
        }
        self.recent_rtts.push_back(next.rtt_ms.max(1));
        let recent_floor = self.recent_rtts.iter().copied().min().unwrap_or(1);
        self.cooldown_ms = self.cooldown_ms.saturating_sub(elapsed_ms);
        if let Some(since) = self.since_restore.as_mut() {
            *since = since.saturating_add(elapsed_ms);
            if *since >= RESTORE_PROVEN_MS {
                self.since_restore = None;
                self.before_strain = None;
            }
        }
        // A probe that was not answered by a longer round trip within
        // [`PROBE_JUDGED_MS`] found a rate the link carries.
        if let Some(probe) = self.probe.as_mut() {
            probe.age_ms = probe.age_ms.saturating_add(elapsed_ms);
            if probe.age_ms >= PROBE_JUDGED_MS {
                self.probe = None;
                self.probe_wait_ms = BITRATE_PROBE_MS;
            }
        }
        self.status = DesktopMediaStatus {
            reason: AdaptationReason::Stable,
            received_kbps: bytes
                .saturating_mul(8)
                .checked_div(elapsed_ms)
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            received_fps: frames
                .saturating_mul(1000)
                .checked_div(elapsed_ms)
                .unwrap_or(0)
                .min(u16::MAX as u64) as u16,
            receiver_queue_ms: next.receiver.oldest_ms,
            host_queue_ms: next.host.map_or(0, |host| host.oldest_ms),
            send_loss_per_mille: loss,
            rtt_ms: next.rtt_ms,
            decode_us: next.decode_us.unwrap_or(0),
            encode_us: next.host.map_or(0, |host| host.encode_us),
            capture_us: next.host.map_or(0, |host| host.capture_us),
            limit: RateLimit::None,
            conditions: crate::ConditionSet::default(),
        };
        let rate = u64::from(current.max_frame_rate);
        let decode_pressure = frames >= 2
            && next
                .decode_us
                .is_some_and(|us| us.saturating_mul(rate) > 750_000);
        let encoder_pressure = frames >= 2
            && next
                .host
                .is_some_and(|host| host.encode_us.saturating_mul(rate) > 750_000);
        // Near the budget is not pressure, but is no time to ask for more.
        self.hot = if frames >= 2
            && next
                .host
                .is_some_and(|host| host.encode_us.saturating_mul(rate) > HOT_US_A_SECOND)
        {
            RateLimit::Encoder
        } else if frames >= 2
            && next
                .decode_us
                .is_some_and(|us| us.saturating_mul(rate) > HOT_US_A_SECOND)
        {
            RateLimit::Receiver
        } else {
            RateLimit::None
        };
        let receiver_backed_up = next.receiver.oldest_ms >= 150
            || next.receiver.queued >= 6
            || (drops >= 3 && next.receiver.oldest_ms >= 60);
        let receiver_pressure = decode_pressure || encoder_pressure || receiver_backed_up;
        // Sender loss concerns the actual sending connection, not the controller's
        // unrelated outbound input. RTT alone or use of a relay is not congestion.
        let host_queue_backed_up = next
            .host
            .is_some_and(|host| host.oldest_ms >= 150 && host.queued >= 3);
        let host_pressure =
            host_queue_backed_up || next.host.is_some_and(|host| host.backlog_ms >= 150);
        let inflated_rtt_ms = self
            .rtt_floor
            .saturating_mul(3)
            .max(self.rtt_floor.saturating_add(80));
        let inflated_rtt = next.rtt_ms >= inflated_rtt_ms;
        // The round trip answering a rate just tried: the link's queue
        // filling, before it loses anything.
        let probe_queueing = self.probe.is_some_and(|probe| {
            next.rtt_ms >= probe.rtt_ms + (probe.rtt_ms / 2).max(PROBE_RTT_RISE_MS)
        });
        let losing_frames = missing(gate_dropped) || self.lossy_ms >= LOSS_LASTS_MS;
        self.signals = Some(Signals {
            loss_per_mille: loss,
            lasting_loss_ms: self.lossy_ms,
            missed_per_mille,
            gate_dropping: missing(gate_dropped),
            backlog_ms: next.host.map_or(0, |host| host.backlog_ms),
            host_queue_backed_up,
            host_queue_ms: self.status.host_queue_ms,
            rtt_ms: next.rtt_ms,
            rtt_floor_ms: self.rtt_floor,
            inflated_rtt_ms,
            congestion_events: match (next.host, previous.host) {
                (Some(now), Some(before)) => now
                    .congestion_events
                    .saturating_sub(before.congestion_events),
                _ => 0,
            },
            encoder_pressure,
            encode_us: self.status.encode_us,
            decoder_pressure: decode_pressure,
            decode_us: self.status.decode_us,
            receiver_backed_up,
            receiver_queue_ms: next.receiver.oldest_ms,
            picture_budget_us: 750_000 / u64::from(current.max_frame_rate.max(1)),
        });
        // What the bitrate may be: the viewer's maximum, what the picture
        // can use and what a relayed path is held to. What the link was
        // measured to carry only stops it rising: the window it is read
        // from stays small while the stream does not fill it.
        let floor = FLOOR_KBPS.min(ceiling.max_bitrate_kbps);
        let size = bitrate::encode_size((current.max_width, current.max_height), limits.monitor);
        let mut bound = Bound {
            kbps: ceiling.max_bitrate_kbps,
            // The largest a quality holds is no choice of the viewer's.
            by: if ceiling.max_bitrate_kbps >= MAX_BITRATE_KBPS {
                RateLimit::None
            } else {
                RateLimit::Setting
            },
        };
        for (kbps, by) in [
            (
                bitrate::picture_kbps(limits.codec, size, current.max_frame_rate),
                RateLimit::None,
            ),
            (
                if limits.relayed { RELAY_KBPS } else { u32::MAX },
                RateLimit::Relay,
            ),
        ] {
            if kbps < bound.kbps {
                bound = Bound { kbps, by };
            }
        }
        bound.kbps = bound.kbps.max(floor);
        self.bound = bound;
        self.raise_to = match next
            .host
            .map(|host| host.link_kbps)
            .filter(|kbps| *kbps != 0)
        {
            Some(kbps) if kbps.saturating_add(kbps / 4) < bound.kbps => Bound {
                kbps: kbps.saturating_add(kbps / 4).max(floor),
                by: RateLimit::Network,
            },
            _ => bound,
        };
        let network_pressure = loss >= 20
            || host_pressure
            || (loss >= 5 && inflated_rtt)
            || losing_frames
            || probe_queueing;
        if receiver_pressure || network_pressure {
            self.healthy_ms = 0;
            self.strained_ms = self.strained_ms.saturating_add(elapsed_ms);
            self.status.reason = if decode_pressure || receiver_backed_up {
                AdaptationReason::ReceiverPressure
            } else if encoder_pressure {
                AdaptationReason::EncoderPressure
            } else {
                AdaptationReason::NetworkPressure
            };
            // A strain that costs frames now, or a rate just tried that
            // fills the link's queue, is cut at the first sample.
            let backlog = next.host.map_or(0, |host| host.backlog_ms);
            let severe = !receiver_pressure
                && (missed_per_mille >= 100 || backlog >= 300 || loss >= 50 || probe_queueing);
            let patience = if severe { 1 } else { STRAIN_PATIENCE_MS };
            // The cooldown that follows a rise does not hold back the answer
            // to it.
            let answering_probe = severe && self.probe.is_some();
            if self.strained_ms < patience || (self.cooldown_ms != 0 && !answering_probe) {
                return None;
            }
            self.strained_ms = 0;
            self.cooldown_ms = 2000;
            let mut quality = current;
            if receiver_pressure {
                if quality.max_frame_rate > 15 {
                    quality.max_frame_rate =
                        (u16::from(quality.max_frame_rate) * 3 / 4).max(15) as u8;
                } else {
                    quality.max_width = ((u32::from(quality.max_width) * 3 / 4)
                        .max(320)
                        .min(u32::from(quality.max_width)))
                        as u16;
                    quality.max_height = ((u32::from(quality.max_height) * 3 / 4)
                        .max(180)
                        .min(u32::from(quality.max_height)))
                        as u16;
                }
            } else {
                let rate = quality.max_bitrate_kbps;
                self.strain_seen = true;
                // A rate just tried that did not hold is tried again later,
                // and later still each time.
                let probe = self.probe.take();
                if probe.is_some() {
                    self.probe_wait_ms = self
                        .probe_wait_ms
                        .saturating_mul(2)
                        .min(BITRATE_PROBE_MAX_MS);
                }
                let kept = match probe {
                    // The round trip answered it: given back, to the rate
                    // before it, which the link was known to carry.
                    Some(probe) if probe_queueing => probe.from,
                    _ => {
                        // A restore that strains again was too much: the rate to
                        // return to comes down by a quarter. Otherwise the rate in
                        // use now is the one this strain started from.
                        self.before_strain = match (self.before_strain, self.since_restore.take()) {
                            (Some(before), Some(_)) => Some(before * 3 / 4),
                            (Some(before), None) => Some(before),
                            (None, _) => Some(rate),
                        };
                        // Graded by how bad: a mild strain takes off less.
                        if missed_per_mille >= MISSED_SEVERE || backlog >= 600 || loss >= 100 {
                            rate / 2
                        } else if missed_per_mille >= 100 || backlog >= 300 || loss >= 50 {
                            rate * 3 / 4
                        } else {
                            rate * 85 / 100
                        }
                    }
                };
                quality.max_bitrate_kbps = kept.max(floor).min(rate);
                if quality.max_bitrate_kbps <= 1500 {
                    quality.max_frame_rate = (quality.max_frame_rate / 2)
                        .max(10)
                        .min(quality.max_frame_rate);
                }
            }
            return (quality != current).then_some(quality);
        }
        self.strained_ms = 0;
        // A bound that came down under the rate in use (a relayed path, a
        // smaller picture) is not waited for.
        if current.max_bitrate_kbps > bound.kbps {
            self.probe = None;
            return Some(Quality {
                max_bitrate_kbps: bound.kbps,
                ..current
            });
        }
        if frames == 0 {
            self.status.reason = AdaptationReason::Idle;
            self.healthy_ms = 0;
            return None;
        }
        // Recovery requires sustained observed frame delivery; no upshift from
        // an unused owner's empty queue or from a clock leap. Back to the
        // rate in use before a strain is sooner than probing above it.
        self.healthy_ms = self.healthy_ms.saturating_add(elapsed_ms);
        let returning = self
            .before_strain
            .is_some_and(|before| current.max_bitrate_kbps < before);
        if self.cooldown_ms != 0 {
            return None;
        }
        let mut quality = current;
        // Only a stream using its bitrate proves the link carries more: a
        // still screen sending a fraction of it proves nothing. Nor does
        // one whose round trip has left its floor, that lost data in the
        // interval, or whose coders have no time to spare.
        let used = u64::from(self.status.received_kbps) * 2 >= u64::from(current.max_bitrate_kbps);
        let near_floor = next.rtt_ms <= recent_floor + (recent_floor / 2).max(NEAR_FLOOR_MS);
        let probing = used
            && near_floor
            && loss == 0
            && self.hot == RateLimit::None
            && current.max_bitrate_kbps < self.raise_to.kbps;
        let bitrate_wait = if returning {
            RETURN_AFTER_MS
        } else {
            self.probe_wait_ms
        };
        if probing && self.healthy_ms >= bitrate_wait {
            let step = if self.strain_seen {
                (current.max_bitrate_kbps / 4).max(250)
            } else {
                (current.max_bitrate_kbps / 2).max(500)
            };
            let mut up = current.max_bitrate_kbps.saturating_add(step);
            // Back up to the rate before the strain, not past it in one step.
            if let Some(before) = self.before_strain.filter(|_| returning) {
                up = up.min(before);
            }
            quality.max_bitrate_kbps = up.min(self.raise_to.kbps).max(current.max_bitrate_kbps);
            if returning
                && self
                    .before_strain
                    .is_some_and(|before| quality.max_bitrate_kbps >= before)
            {
                self.since_restore = Some(0);
            } else if !returning && quality.max_bitrate_kbps > current.max_bitrate_kbps {
                self.probe = Some(Probe {
                    from: current.max_bitrate_kbps,
                    age_ms: 0,
                    rtt_ms: next.rtt_ms,
                });
            }
        }
        let shape_wait = if returning {
            RETURN_AFTER_MS
        } else {
            PROBE_AFTER_MS
        };
        if self.healthy_ms >= shape_wait {
            // Back toward what the viewer chose, but no further than a quality
            // holds.
            let width = ceiling.max_width.min(MAX_EDGE);
            let height = ceiling.max_height.min(MAX_EDGE);
            let processing_us = next
                .decode_us
                .unwrap_or(0)
                .max(next.host.map_or(0, |host| host.encode_us));
            let decoder_cap = Some(processing_us)
                .filter(|us| *us != 0)
                .map_or(60, |us| (750_000 / us).clamp(1, 60) as u8);
            quality.max_frame_rate = quality
                .max_frame_rate
                .saturating_add(10)
                .min(decoder_cap)
                .min(ceiling.max_frame_rate)
                .max(current.max_frame_rate);
            quality.max_width = quality
                .max_width
                .saturating_add((quality.max_width / 4).max(1))
                .min(width);
            quality.max_height = quality
                .max_height
                .saturating_add((quality.max_height / 4).max(1))
                .min(height);
        }
        if quality != current {
            self.healthy_ms = 0;
            self.cooldown_ms = 2000;
            self.status.reason = AdaptationReason::Recovering;
            Some(quality)
        } else {
            None
        }
    }
}

/// A ceiling no test reaches: the largest bitrate a quality holds, at the
/// size and rate the tests start from.
#[cfg(test)]
const WIDE: Quality = Quality {
    max_bitrate_kbps: 50_000,
    max_frame_rate: 60,
    max_width: 1920,
    max_height: 1080,
};

#[cfg(test)]
mod tests {
    use super::*;
    fn frame_sample(second: u64) -> Feedback {
        Feedback {
            receiver: crate::video::QueueTelemetry {
                received_frames: second * 30,
                received_bytes: second * 250_000,
                ..Default::default()
            },
            host: Some(HostFeedback {
                sent_bytes: second * 250_000,
                ..Default::default()
            }),
            frames_lost: 0,
            rtt_ms: 300,
            path_changed: false,
            decode_us: None,
        }
    }
    fn initial() -> Quality {
        Quality::new(8000, 60, 1920, 1080).unwrap()
    }
    #[test]
    fn stable_high_latency_path_recovers_without_a_relay_penalty() {
        let mut state = Adaptive::default();
        let mut quality = Quality::new(1000, 15, 1280, 720).unwrap();
        assert!(state.step(quality, WIDE, 1000, frame_sample(0)).is_none());
        for second in 1..3 {
            assert!(state
                .step(quality, WIDE, 1000, frame_sample(second))
                .is_none());
        }
        // A round trip of 300 ms that stays 300 ms is no strain: the rate
        // climbs after three healthy seconds, in half steps until it has
        // met the link.
        quality = state.step(quality, WIDE, 1000, frame_sample(3)).unwrap();
        assert_eq!(quality, Quality::new(1500, 15, 1280, 720).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::Recovering);
        assert_eq!(state.status.received_fps, 30);
        assert_eq!(state.status.received_kbps, 2000);
        for second in 4..40 {
            if let Some(next) = state.step(quality, WIDE, 1000, frame_sample(second)) {
                assert!(next.max_bitrate_kbps >= quality.max_bitrate_kbps);
                quality = next;
            }
        }
        assert!(quality.max_frame_rate > 15 && quality.max_width > 1280);
    }
    /// Found by the `host_feedback` fuzz target, when recovery still grew
    /// toward the host's monitor: the largest one overflowed the bitrate
    /// cap's pixel count, and the size grew past what a quality may hold,
    /// which the host refused as malformed. The largest ceiling a quality
    /// holds stays a quality.
    #[test]
    fn recovery_toward_the_largest_ceiling_stays_a_quality() {
        let mut state = Adaptive::default();
        let mut quality = Quality::new(1000, 15, 7000, 7000).unwrap();
        let ceiling = Quality::new(50_000, 60, MAX_EDGE, MAX_EDGE).unwrap();
        state.step(quality, ceiling, 1000, frame_sample(0));
        let mut sizes = Vec::new();
        for second in 1..30 {
            if let Some(next) = state.step(quality, ceiling, 1000, frame_sample(second)) {
                assert_eq!(state.status.reason, AdaptationReason::Recovering);
                let encoded = crate::protocol::ControlMessage::Quality {
                    stamp: crate::protocol::SessionStamp::new(1, 1),
                    quality: next,
                }
                .encode();
                assert!(crate::protocol::ControlMessage::decode(&encoded).is_ok());
                sizes.push((next.max_width, next.max_height));
                quality = next;
            }
        }
        assert_eq!(sizes.last(), Some(&(MAX_EDGE, MAX_EDGE)));
        assert!(quality.max_bitrate_kbps > 1000 && quality.max_bitrate_kbps <= 50_000);
    }
    /// The frame rate, size and bitrate the viewer chose are a ceiling:
    /// after a strain took them down, recovery climbs back to them and
    /// stops there, however healthy the link and however large the host's
    /// screen. It once climbed to 60 fps from a choice of 30.
    #[test]
    fn recovery_stops_at_the_quality_the_viewer_chose() {
        let mut state = Adaptive::default();
        let chosen = Quality::new(12_000, 30, 1920, 1080).unwrap();
        let mut quality = Quality::new(4000, 15, 1280, 720).unwrap();
        // Using its bitrate: every second carries what the quality allows.
        let sample = |second: u64, kbps: u64| Feedback {
            receiver: crate::video::QueueTelemetry {
                received_frames: second * 30,
                received_bytes: second * kbps * 125,
                ..Default::default()
            },
            host: Some(HostFeedback {
                sent_bytes: second * kbps * 125,
                encode_us: 5000,
                ..Default::default()
            }),
            frames_lost: 0,
            rtt_ms: 10,
            path_changed: false,
            decode_us: Some(2000),
        };
        state.step(quality, chosen, 1000, sample(0, 0));
        for second in 1..=120 {
            let used = u64::from(quality.max_bitrate_kbps);
            if let Some(next) = state.step(quality, chosen, 1000, sample(second, used)) {
                assert!(next.max_frame_rate <= 30, "{next:?}");
                assert!(next.max_bitrate_kbps <= 12_000, "{next:?}");
                assert!(
                    next.max_width <= 1920 && next.max_height <= 1080,
                    "{next:?}"
                );
                quality = next;
            }
        }
        // The frame rate and size reach the choice; the bitrate, the most a
        // 1080p picture at 30 fps can use, below the viewer's maximum.
        assert_eq!(
            quality,
            Quality::new(9953, 30, 1920, 1080).unwrap(),
            "recovery reaches the choice"
        );
    }
    /// A choice below what the link and coders could take stays where it
    /// is: a healthy stream at the chosen quality is not raised.
    #[test]
    fn a_healthy_stream_at_its_ceiling_stays_there() {
        let mut state = Adaptive::default();
        let chosen = Quality::new(1000, 15, 1280, 720).unwrap();
        state.step(chosen, chosen, 1000, frame_sample(0));
        for second in 1..=30 {
            assert!(state
                .step(chosen, chosen, 1000, frame_sample(second))
                .is_none());
        }
    }
    #[test]
    fn decoder_pressure_reduces_frame_rate_before_bitrate_or_text_resolution() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, WIDE, 1000, frame_sample(0));
        let mut first = frame_sample(1);
        first.receiver.queued = 7;
        first.receiver.oldest_ms = 200;
        assert!(state.step(quality, WIDE, 1000, first).is_none());
        let mut second = frame_sample(2);
        second.receiver.queued = 6;
        second.receiver.oldest_ms = 190;
        let next = state.step(quality, WIDE, 1000, second).unwrap();
        assert_eq!(next, Quality::new(8000, 45, 1920, 1080).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::ReceiverPressure);
        assert_eq!(state.status.send_loss_per_mille, 0);
    }
    #[test]
    fn sender_loss_reduces_bitrate_and_cooldown_prevents_per_tick_oscillation() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, WIDE, 1000, frame_sample(0));
        let mut sample = frame_sample(1);
        sample.host.as_mut().unwrap().lost_bytes = 10_000;
        assert!(state.step(quality, WIDE, 1000, sample).is_none());
        sample = frame_sample(2);
        sample.host.as_mut().unwrap().lost_bytes = 20_000;
        // 4% of bytes lost is a mild strain: 15% off.
        let next = state.step(quality, WIDE, 1000, sample).unwrap();
        assert_eq!(next, Quality::new(6800, 60, 1920, 1080).unwrap());
        assert_eq!(state.status.send_loss_per_mille, 40);
        assert_eq!(state.status.reason, AdaptationReason::NetworkPressure);
        sample = frame_sample(3);
        sample.host.as_mut().unwrap().lost_bytes = 30_000;
        assert!(state.step(next, WIDE, 1000, sample).is_none());
    }
    #[test]
    fn frames_that_keep_going_missing_lower_the_bitrate_and_many_halve_it() {
        let quality = initial();
        let run = |samples: &[(u64, u64)]| {
            let mut state = Adaptive::default();
            state.step(quality, WIDE, 1000, frame_sample(0));
            let mut stepped = None;
            for (second, &(lost, dropped)) in (1..).zip(samples) {
                let mut sample = frame_sample(second);
                sample.frames_lost = lost;
                sample.host.as_mut().unwrap().send_dropped = dropped;
                stepped = stepped.or(state.step(quality, WIDE, 1000, sample));
            }
            stepped.map(|quality| quality.max_bitrate_kbps)
        };
        // The host dropping frames behind the link: two seconds of it.
        assert_eq!(run(&[(0, 2), (0, 4)]), Some(6800));
        assert_eq!(run(&[(0, 10), (0, 20)]), Some(4000));
        // Frames lost at the viewer: three seconds of it.
        assert_eq!(run(&[(2, 0), (4, 0)]), None);
        assert_eq!(run(&[(2, 0), (4, 0), (6, 0), (8, 0)]), Some(6800));
        // A blackout loses many frames once, or across two samples.
        assert_eq!(run(&[(8, 0), (8, 0), (8, 0), (8, 0)]), None);
        assert_eq!(run(&[(0, 0), (4, 0), (8, 0), (8, 0), (8, 0)]), None);
        // One stray loss is not pressure.
        assert_eq!(run(&[(1, 0), (2, 0), (3, 0), (4, 0)]), None);
    }

    #[test]
    fn a_standing_backlog_in_the_transport_lowers_the_bitrate() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, WIDE, 1000, frame_sample(0));
        // A queue of 300 ms is a severe strain: cut at the first sample.
        let mut sample = frame_sample(1);
        sample.host.as_mut().unwrap().backlog_ms = 300;
        let next = state.step(quality, WIDE, 1000, sample).unwrap();
        assert_eq!(next.max_bitrate_kbps, 6000);
        assert_eq!(state.status.reason, AdaptationReason::NetworkPressure);
        // One of 150 ms is a mild one, and waits for a second sample.
        let mut state = Adaptive::default();
        state.step(quality, WIDE, 1000, frame_sample(0));
        let mut sample = frame_sample(1);
        sample.host.as_mut().unwrap().backlog_ms = 160;
        assert!(state.step(quality, WIDE, 1000, sample).is_none());
        let mut sample = frame_sample(2);
        sample.host.as_mut().unwrap().backlog_ms = 170;
        let next = state.step(quality, WIDE, 1000, sample).unwrap();
        assert_eq!(next.max_bitrate_kbps, 6800);
    }

    /// The cut grows with the strain; recovery returns to the rate in use
    /// before it sooner than it probes above, and a return that strains
    /// again lowers that rate by a quarter.
    #[test]
    fn recovery_returns_to_the_last_good_rate_and_learns_from_a_failed_return() {
        // Samples carry 4,000 kb/s, so a 4,000 kb/s quality uses its rate.
        let sample = |second: u64, lost_bytes: u64| {
            let mut feedback = frame_sample(second);
            feedback.receiver.received_bytes = second * 500_000;
            let host = feedback.host.as_mut().unwrap();
            host.sent_bytes = second * 500_000;
            host.lost_bytes = lost_bytes;
            feedback
        };
        let mut state = Adaptive::default();
        let mut quality = Quality::new(4000, 30, 1920, 1080).unwrap();
        let mut second = 0;
        let mut lost = 0;
        let mut run = |state: &mut Adaptive, quality: &mut Quality, seconds: u64, loss: u64| {
            let mut changes = Vec::new();
            for _ in 0..seconds {
                second += 1;
                lost += loss;
                if let Some(next) = state.step(*quality, WIDE, 1000, sample(second, lost)) {
                    *quality = next;
                    changes.push((second, next.max_bitrate_kbps));
                }
            }
            changes
        };
        run(&mut state, &mut quality, 1, 0);
        // 12% of bytes lost: a severe strain, halved.
        let down = run(&mut state, &mut quality, 2, 60_000);
        assert_eq!(down.last().unwrap().1, 2000);
        // Healthy: back towards 4,000 after three seconds, not six, and not
        // past it.
        let back = run(&mut state, &mut quality, 12, 0);
        assert_eq!(back.first().unwrap(), &(6, 2500));
        assert_eq!(quality.max_bitrate_kbps, 4000);
        // Strain right after getting back: the rate to return to is now
        // 3,000.
        run(&mut state, &mut quality, 2, 60_000);
        let again = run(&mut state, &mut quality, 20, 0);
        let rates: Vec<u32> = again.iter().map(|(_, rate)| *rate).collect();
        assert_eq!(
            &rates[..2],
            [2500, 3000],
            "returned only to 3,000: {rates:?}"
        );
        assert_eq!(state.before_strain, None, "proven again after holding");
    }

    /// A stream sending far less than its bitrate does not earn more:
    /// frame rate may still recover, the bitrate stays.
    #[test]
    fn an_unused_bitrate_is_not_raised() {
        let mut state = Adaptive::default();
        // Samples carry 2,000 kb/s against a 6,000 kb/s quality.
        let quality = Quality::new(6000, 30, 1920, 1080).unwrap();
        state.step(quality, WIDE, 1000, frame_sample(0));
        let mut raised = None;
        for second in 1..=7 {
            raised = raised.or(state.step(quality, WIDE, 1000, frame_sample(second)));
        }
        let raised = raised.unwrap();
        assert_eq!(raised.max_bitrate_kbps, 6000);
        assert!(raised.max_frame_rate > 30);
    }

    #[test]
    fn idle_counter_reset_and_path_change_never_pretend_to_prove_capacity() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, WIDE, 1000, frame_sample(0));
        for _ in 0..20 {
            assert!(state.step(quality, WIDE, 1000, frame_sample(0)).is_none());
        }
        assert_eq!(state.status.reason, AdaptationReason::Idle);
        let mut sample = frame_sample(1);
        sample.path_changed = true;
        assert!(state.step(quality, WIDE, 1000, sample).is_none());
        assert_eq!(state.status.reason, AdaptationReason::WarmingUp);
        assert!(state.step(quality, WIDE, 1000, frame_sample(0)).is_none());
        assert!(state.step(quality, WIDE, 20_000, frame_sample(1)).is_none());
    }
    #[test]
    fn keyframe_replacement_drops_without_backlog_do_not_lower_quality() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, WIDE, 1000, frame_sample(0));
        for second in 1..5 {
            let mut sample = frame_sample(second);
            sample.receiver.dropped = second * 2;
            assert!(state.step(quality, WIDE, 1000, sample).is_none());
            assert_eq!(state.status.reason, AdaptationReason::Stable);
        }
    }
    #[test]
    fn host_feedback_is_exact_bounded_and_truncation_is_rejected() {
        let feedback = HostFeedback {
            sent_bytes: u64::MAX,
            lost_bytes: 123,
            dropped: 456,
            send_dropped: 42,
            backlog_ms: 150,
            queued: 8,
            oldest_ms: 789,
            encode_us: 1234,
            capture_us: 5678,
            link_kbps: 940_000,
            congestion_events: 17,
            flags: ENCODER_KNOWN | SECURE_DESKTOP,
        };
        let bytes = feedback.encode();
        let decoded = HostFeedback::decode(&bytes).unwrap();
        assert_eq!(decoded.sent_bytes, u64::MAX);
        assert_eq!((decoded.encode_us, decoded.capture_us), (1234, 5678));
        assert_eq!((decoded.send_dropped, decoded.backlog_ms), (42, 150));
        assert_eq!(decoded.congestion_events, 17);
        assert_eq!(decoded.link_kbps, 940_000);
        assert_eq!(
            decoded.host_state(),
            crate::video::HostState {
                encoder_hardware: Some(false),
                secure_desktop: true,
            }
        );
        assert_eq!(
            (
                decoded.lost_bytes,
                decoded.dropped,
                decoded.queued,
                decoded.oldest_ms
            ),
            (123, 456, 8, 789)
        );
        for end in 0..bytes.len() {
            assert!(HostFeedback::decode(&bytes[..end]).is_err());
        }
        let mut malformed = bytes.clone();
        malformed[73] = 9;
        assert!(HostFeedback::decode(&malformed).is_err());
        // An unknown flag, or hardware without knowing the encoder.
        for flags in [8, ENCODER_HARDWARE] {
            let mut malformed = bytes.clone();
            malformed[78] = flags;
            assert!(HostFeedback::decode(&malformed).is_err(), "{flags}");
        }
    }
}

#[cfg(test)]
mod decoder_tests {
    use super::*;
    #[test]
    fn measured_decoder_cost_limits_fps_and_recovery_does_not_probe_above_capacity() {
        let mut adaptive = Adaptive::default();
        let mut quality = Quality::new(8000, 60, 1920, 1080).unwrap();
        let sample = |second: u64| Feedback {
            receiver: crate::video::QueueTelemetry {
                received_frames: second * 30,
                received_bytes: second * 250_000,
                ..Default::default()
            },
            host: None,
            frames_lost: 0,
            rtt_ms: 5,
            path_changed: false,
            decode_us: Some(25_000),
        };
        adaptive.step(quality, WIDE, 1000, sample(0));
        for second in 1..=8 {
            if let Some(next) = adaptive.step(quality, WIDE, 1000, sample(second)) {
                quality = next;
            }
        }
        assert_eq!(quality.max_frame_rate, 24);
        // The picture at 24 fps can use 7,962 kb/s, and no more.
        assert_eq!(quality.max_bitrate_kbps, 7962);
        assert_eq!((quality.max_width, quality.max_height), (1920, 1080));
        for second in 9..=14 {
            if let Some(next) = adaptive.step(quality, WIDE, 1000, sample(second)) {
                quality = next;
            }
        }
        assert_eq!(quality.max_frame_rate, 30);
        for second in 15..=40 {
            if let Some(next) = adaptive.step(quality, WIDE, 1000, sample(second)) {
                quality = next;
            }
        }
        assert_eq!(quality.max_frame_rate, 30);
    }
}

#[cfg(test)]
mod producer_tests {
    use super::*;
    #[test]
    fn slow_encoding_is_not_misreported_as_network_loss_and_capture_wait_is_diagnostic() {
        let mut state = Adaptive::default();
        let quality = Quality::new(8000, 60, 1920, 1080).unwrap();
        let sample = |second: u64, encode_us| Feedback {
            receiver: crate::video::QueueTelemetry {
                received_frames: second * 30,
                received_bytes: second * 250_000,
                ..Default::default()
            },
            host: Some(HostFeedback {
                encode_us,
                capture_us: 100_000,
                sent_bytes: second * 250_000,
                ..Default::default()
            }),
            frames_lost: 0,
            rtt_ms: 5,
            path_changed: false,
            decode_us: None,
        };
        state.step(quality, WIDE, 1000, sample(0, 1000));
        assert!(state.step(quality, WIDE, 1000, sample(1, 1000)).is_none());
        assert_eq!(state.status.reason, AdaptationReason::Stable);
        assert!(state.step(quality, WIDE, 1000, sample(2, 30_000)).is_none());
        let next = state.step(quality, WIDE, 1000, sample(3, 30_000)).unwrap();
        assert_eq!(next, Quality::new(8000, 45, 1920, 1080).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::EncoderPressure);
        assert_eq!(state.status.send_loss_per_mille, 0);
        assert_eq!(
            (state.status.encode_us, state.status.capture_us),
            (30_000, 100_000)
        );
    }
}

/// The bitrate follows the picture and the link: where it starts, how it
/// climbs, what stops it, how fast it falls, and what the status says holds
/// it. The link is simulated by what each second's feedback says.
#[cfg(test)]
mod automatic_tests {
    use super::*;

    /// One simulated second on a link: counters that only grow, and what
    /// this second's samples show.
    struct Link {
        second: u64,
        frames: u64,
        bytes: u64,
        sent: u64,
        lost: u64,
        rtt_ms: u64,
        /// Share of the stream's bytes the link loses, per mille.
        loss_per_mille: u64,
        /// The round trip rises by this much for each kb/s above the
        /// capacity, in microseconds, as a queue filling does.
        capacity_kbps: u64,
        link_kbps: u32,
        encode_us: u64,
        backlog_ms: u64,
        /// Whether the screen is changing, so the stream uses its rate.
        moving: bool,
    }
    impl Link {
        fn new(capacity_kbps: u64) -> Self {
            Self {
                second: 0,
                frames: 0,
                bytes: 0,
                sent: 0,
                lost: 0,
                rtt_ms: 10,
                loss_per_mille: 0,
                capacity_kbps,
                link_kbps: 0,
                encode_us: 2000,
                backlog_ms: 0,
                moving: true,
            }
        }
        /// The feedback after a second at `quality`: a moving screen sends
        /// what the quality allows; a link beyond its capacity queues, then
        /// loses.
        fn feedback(&mut self, quality: Quality) -> Feedback {
            self.second += 1;
            let wanted = if self.moving {
                u64::from(quality.max_bitrate_kbps)
            } else {
                u64::from(quality.max_bitrate_kbps) / 40
            };
            let carried = wanted.min(self.capacity_kbps);
            let over = wanted.saturating_sub(self.capacity_kbps);
            let sent_bytes = wanted * 125;
            self.sent += sent_bytes;
            self.lost += if over > 0 {
                over * 125
            } else {
                sent_bytes * self.loss_per_mille / 1000
            };
            self.bytes += carried * 125;
            self.frames += u64::from(quality.max_frame_rate);
            self.rtt_ms = if over > 0 {
                10 + over * 40 / self.capacity_kbps.max(1)
            } else {
                10
            };
            Feedback {
                receiver: crate::video::QueueTelemetry {
                    received_frames: self.frames,
                    received_bytes: self.bytes,
                    ..Default::default()
                },
                host: Some(HostFeedback {
                    sent_bytes: self.sent,
                    lost_bytes: self.lost,
                    encode_us: self.encode_us,
                    backlog_ms: self.backlog_ms,
                    link_kbps: self.link_kbps,
                    ..Default::default()
                }),
                frames_lost: 0,
                rtt_ms: self.rtt_ms,
                path_changed: false,
                decode_us: Some(1000),
            }
        }
    }

    /// Runs `seconds` of the link against the controller; returns each
    /// change, with the second it came at.
    fn drive(
        state: &mut Adaptive,
        link: &mut Link,
        quality: &mut Quality,
        limits: Limits,
        seconds: u64,
    ) -> Vec<(u64, Quality)> {
        let mut changes = Vec::new();
        for _ in 0..seconds {
            let feedback = link.feedback(*quality);
            if let Some(next) = state.step(*quality, limits, 1000, feedback) {
                *quality = next;
                changes.push((link.second, next));
            }
        }
        changes
    }

    fn limits(maximum: u32, fps: u8) -> Limits {
        Limits::from(Quality::new(maximum, fps, 1920, 1080).unwrap())
    }

    #[test]
    fn the_bitrate_climbs_in_half_steps_to_what_the_picture_can_use_and_stops() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        let changes = drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 40);
        let rates: Vec<u32> = changes.iter().map(|(_, q)| q.max_bitrate_kbps).collect();
        // 1080p60 H.264 can use 19,906 kb/s: 0.16 bits a pixel.
        assert_eq!(rates, [6000, 9000, 13500, 19906], "{changes:?}");
        // The first climb is three healthy seconds after the first reading.
        assert_eq!(changes[0].0, 4);
        assert_eq!(
            state.status.limit,
            RateLimit::None,
            "the picture needs no more"
        );
        // The same picture in HEVC needs a third less.
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        let hevc = Limits {
            codec: VideoCodec::H265,
            ..limits(50_000, 60)
        };
        state.step(quality, hevc, 1000, link.feedback(quality));
        drive(&mut state, &mut link, &mut quality, hevc, 40);
        assert_eq!(quality.max_bitrate_kbps, 13685);
    }

    #[test]
    fn the_viewers_maximum_stops_it_and_is_named_unless_it_is_the_largest_a_stream_holds() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(2000, 30, 1920, 1080).unwrap();
        state.step(quality, limits(2000, 30), 1000, link.feedback(quality));
        // Data saver: a stream that starts at its limit stays there.
        assert!(drive(&mut state, &mut link, &mut quality, limits(2000, 30), 30).is_empty());
        assert_eq!(state.status.limit, RateLimit::Setting);
        // A maximum of 6 Mb/s is reached and named.
        let mut state = Adaptive::default();
        let mut quality = Quality::new(4000, 30, 1920, 1080).unwrap();
        state.step(quality, limits(6000, 30), 1000, link.feedback(quality));
        drive(&mut state, &mut link, &mut quality, limits(6000, 30), 30);
        assert_eq!(quality.max_bitrate_kbps, 6000);
        assert_eq!(state.status.limit, RateLimit::Setting);
        // The most a stream holds is no one's choice.
        let mut state = Adaptive::default();
        let mut quality = Quality::new(4000, 60, 3840, 2160).unwrap();
        let wide = Limits::from(Quality::new(50_000, 60, 3840, 2160).unwrap());
        state.step(quality, wide, 1000, link.feedback(quality));
        drive(&mut state, &mut link, &mut quality, wide, 60);
        assert_eq!(quality.max_bitrate_kbps, 50_000);
        assert_eq!(state.status.limit, RateLimit::None);
    }

    #[test]
    fn a_still_screen_a_risen_round_trip_or_loss_do_not_earn_a_higher_rate() {
        let climbs = |change: &dyn Fn(&mut Link)| {
            let mut state = Adaptive::default();
            let mut link = Link::new(1_000_000);
            let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
            state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
            change(&mut link);
            !drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 12).is_empty()
        };
        assert!(climbs(&|_| {}));
        // A still screen sends a fraction of its rate: nothing proven.
        assert!(!climbs(&|link| link.moving = false));
        // A little loss: no probe, though not enough to cut either.
        assert!(!climbs(&|link| link.loss_per_mille = 10));
    }

    #[test]
    fn the_round_trip_leaving_its_floor_holds_the_rate_until_it_is_the_new_floor() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        for _ in 0..2 {
            assert!(state
                .step(quality, limits(50_000, 60), 1000, link.feedback(quality))
                .is_none());
        }
        // 30 ms above a floor of 10: queued by someone else's traffic, or by
        // this stream's own on a shared radio, and not so much as to cut. No
        // probe while it is new.
        for _ in 0..8 {
            let mut feedback = link.feedback(quality);
            feedback.rtt_ms = 40;
            assert!(state
                .step(quality, limits(50_000, 60), 1000, feedback)
                .is_none());
        }
        // Ten seconds on, 40 ms is where the link is, and a probe may judge
        // it from there.
        let mut probed = None;
        for _ in 0..6 {
            let mut feedback = link.feedback(quality);
            feedback.rtt_ms = 40;
            probed = probed.or(state.step(quality, limits(50_000, 60), 1000, feedback));
        }
        assert_eq!(probed.unwrap().max_bitrate_kbps, 6000);
    }

    #[test]
    fn a_probe_the_link_answers_with_a_longer_round_trip_is_given_back_at_once_and_retried_later() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        // Healthy to the probe: 4,000 -> 6,000.
        let up = drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 3);
        assert_eq!(up.last().unwrap().1.max_bitrate_kbps, 6000);
        // Its first second: the round trip climbs from 10 to 80 ms with no
        // loss yet. The rate before the probe is back in one sample.
        let mut feedback = link.feedback(quality);
        feedback.rtt_ms = 80;
        let back = state
            .step(quality, limits(50_000, 60), 1000, feedback)
            .unwrap();
        assert_eq!(back.max_bitrate_kbps, 4000);
        assert_eq!(state.status.reason, AdaptationReason::NetworkPressure);
        quality = back;
        // The next probe waits twice as long: six healthy seconds, not three,
        // and is a quarter step, not a half.
        let again = drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 12);
        assert_eq!(again.first().unwrap().1.max_bitrate_kbps, 5000);
        assert!(again.first().unwrap().0 >= 6 + 2, "{again:?}");
    }

    #[test]
    fn a_link_that_cannot_carry_the_rate_is_found_and_left_alone() {
        let mut state = Adaptive::default();
        // A 10 Mb/s link: the stream climbs past it, loses data, and falls.
        let mut link = Link::new(10_000);
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        let changes = drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 60);
        let peak = changes
            .iter()
            .map(|(_, q)| q.max_bitrate_kbps)
            .max()
            .unwrap();
        assert!((6000..=14_000).contains(&peak), "{changes:?}");
        // It does not stay over the link: the last quarter is at or under it.
        let late = changes.iter().filter(|(second, _)| *second > 45);
        for (second, q) in late {
            assert!(q.max_bitrate_kbps <= 13_500, "{second}: {q:?}");
        }
        assert!(quality.max_bitrate_kbps <= 13_500, "{quality:?}");
    }

    #[test]
    fn heavy_loss_at_a_high_rate_cuts_at_the_first_sample_and_light_loss_waits_for_a_second() {
        let run = |loss_per_mille: u64| {
            let mut state = Adaptive::default();
            let mut link = Link::new(1_000_000);
            let quality = Quality::new(20_000, 60, 1920, 1080).unwrap();
            state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
            link.loss_per_mille = loss_per_mille;
            let first = state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
            let second = state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
            (
                first.map(|q| q.max_bitrate_kbps),
                second.map(|q| q.max_bitrate_kbps),
            )
        };
        // 12% lost: half, at once.
        assert_eq!(run(120), (Some(10_000), None));
        // 3% lost: a mild strain, cut a little at the second sample.
        assert_eq!(run(30), (None, Some(17_000)));
    }

    #[test]
    fn a_relayed_path_is_held_to_what_a_relay_carries_and_says_so() {
        let relayed = Limits {
            relayed: true,
            ..limits(50_000, 60)
        };
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(12_000, 60, 1920, 1080).unwrap();
        state.step(quality, relayed, 1000, link.feedback(quality));
        // A rate above the relay's is brought down at once, not waited for.
        let first = state
            .step(quality, relayed, 1000, link.feedback(quality))
            .unwrap();
        assert_eq!(first.max_bitrate_kbps, RELAY_KBPS);
        quality = first;
        assert!(drive(&mut state, &mut link, &mut quality, relayed, 30).is_empty());
        assert_eq!(state.status.limit, RateLimit::Relay);
        // A direct path is not held.
        let mut state = Adaptive::default();
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 12);
        assert!(quality.max_bitrate_kbps > RELAY_KBPS);
    }

    #[test]
    fn what_the_link_is_measured_to_carry_stops_the_climb_but_never_cuts_by_itself() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        link.link_kbps = 8000;
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 40);
        // A quarter above the measured rate: room to find it was low.
        assert_eq!(quality.max_bitrate_kbps, 10_000);
        assert_eq!(state.status.limit, RateLimit::Network);
        // A reading that falls below the rate in use cuts nothing.
        link.link_kbps = 2000;
        assert!(drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 10).is_empty());
        // A reading that rises lets it climb again.
        link.link_kbps = 40_000;
        drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 30);
        assert_eq!(quality.max_bitrate_kbps, 19_906);
    }

    #[test]
    fn a_coder_near_its_budget_gets_no_more_bitrate_whatever_the_link_offers_and_is_named() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        link.link_kbps = 400_000;
        // 11 ms an encode at 60 fps is 66% of the interval: not yet
        // pressure (75%), but no time to spare.
        link.encode_us = 11_000;
        let mut quality = Quality::new(4000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        assert!(drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 20).is_empty());
        assert_eq!(state.status.limit, RateLimit::Encoder);
        assert_eq!(state.status.reason, AdaptationReason::Stable);
        // Once it has time to spare, the climb goes on.
        link.encode_us = 4000;
        drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 12);
        assert!(quality.max_bitrate_kbps > 4000);
        // Past its budget the frame rate comes down and the bitrate does not
        // go up.
        link.encode_us = 15_000;
        let before = quality.max_bitrate_kbps;
        let changes = drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 2);
        assert!(changes.iter().all(|(_, q)| q.max_bitrate_kbps <= before));
        assert_eq!(quality.max_frame_rate, 45, "{quality:?}");
        assert_eq!(state.status.reason, AdaptationReason::EncoderPressure);
    }

    #[test]
    fn a_strain_the_link_showed_lately_is_named_while_the_rate_is_below_what_it_was() {
        let mut state = Adaptive::default();
        let mut link = Link::new(1_000_000);
        let mut quality = Quality::new(8000, 60, 1920, 1080).unwrap();
        state.step(quality, limits(50_000, 60), 1000, link.feedback(quality));
        link.backlog_ms = 400;
        let cut = state
            .step(quality, limits(50_000, 60), 1000, link.feedback(quality))
            .unwrap();
        assert_eq!(cut.max_bitrate_kbps, 6000);
        assert_eq!(state.status.limit, RateLimit::Network);
        quality = cut;
        link.backlog_ms = 0;
        // Back to 8,000 within a few healthy seconds, and then nothing is
        // said: the stream is where it was.
        drive(&mut state, &mut link, &mut quality, limits(50_000, 60), 30);
        assert!(quality.max_bitrate_kbps >= 8000);
    }
}
