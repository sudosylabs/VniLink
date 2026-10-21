//! Measured per-Desktop adaptation. Only local monotonic intervals are used;
//! remote capture timestamps are not assumed synchronized with this computer.
use crate::Quality;

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
}
/// Encoded size of a [`HostFeedback`].
const HOST_FEEDBACK_LEN: usize = 66;

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
}
impl HostFeedback {
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
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&self.queued.to_be_bytes());
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
            queued: u16::from_be_bytes(bytes[64..66].try_into().unwrap()),
        };
        if feedback.queued > 8
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
const MISSED_PRESSURE: u64 = 50;

/// Share of frames missed, per mille, at which the bitrate halves.
const MISSED_SEVERE: u64 = 150;

/// How long frames must go missing at the viewer before it is the link.
const LOSS_LASTS_MS: u64 = 3000;

#[derive(Default)]
pub(crate) struct Adaptive {
    previous: Option<Feedback>,
    healthy_ms: u64,
    strained_ms: u64,
    /// How long the viewer has been missing frames without a break.
    lossy_ms: u64,
    cooldown_ms: u64,
    rtt_floor: u64,
    pub status: DesktopMediaStatus,
}
impl Adaptive {
    pub fn step(
        &mut self,
        current: Quality,
        source: (u16, u16),
        elapsed_ms: u64,
        next: Feedback,
    ) -> Option<Quality> {
        let previous = self.previous.replace(next);
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
            self.rtt_floor = next.rtt_ms.max(1);
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
        self.cooldown_ms = self.cooldown_ms.saturating_sub(elapsed_ms);
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
        };
        let decode_pressure = frames >= 2
            && next
                .decode_us
                .is_some_and(|us| us.saturating_mul(u64::from(current.max_frame_rate)) > 750_000);
        let encoder_pressure = frames >= 2
            && next.host.is_some_and(|host| {
                host.encode_us
                    .saturating_mul(u64::from(current.max_frame_rate))
                    > 750_000
            });
        let receiver_pressure = decode_pressure
            || encoder_pressure
            || next.receiver.oldest_ms >= 150
            || next.receiver.queued >= 6
            || (drops >= 3 && next.receiver.oldest_ms >= 60);
        // Sender loss concerns the actual sending connection, not the controller's
        // unrelated outbound input. RTT alone or use of a relay is not congestion.
        let host_pressure = next.host.is_some_and(|host| {
            (host.oldest_ms >= 150 && host.queued >= 3) || host.backlog_ms >= 150
        });
        let inflated_rtt = next.rtt_ms
            >= self
                .rtt_floor
                .saturating_mul(3)
                .max(self.rtt_floor.saturating_add(80));
        let losing_frames = missing(gate_dropped) || self.lossy_ms >= LOSS_LASTS_MS;
        let network_pressure =
            loss >= 20 || host_pressure || (loss >= 5 && inflated_rtt) || losing_frames;
        if receiver_pressure || network_pressure {
            self.healthy_ms = 0;
            self.strained_ms = self.strained_ms.saturating_add(elapsed_ms);
            self.status.reason = if decode_pressure
                || next.receiver.oldest_ms >= 150
                || next.receiver.queued >= 6
                || (drops >= 3 && next.receiver.oldest_ms >= 60)
            {
                AdaptationReason::ReceiverPressure
            } else if encoder_pressure {
                AdaptationReason::EncoderPressure
            } else {
                AdaptationReason::NetworkPressure
            };
            if self.strained_ms < 2000 || self.cooldown_ms != 0 {
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
                let kept = if missed_per_mille >= MISSED_SEVERE {
                    quality.max_bitrate_kbps / 2
                } else {
                    quality.max_bitrate_kbps * 3 / 4
                };
                quality.max_bitrate_kbps = kept.max(500).min(quality.max_bitrate_kbps);
                if quality.max_bitrate_kbps <= 1500 {
                    quality.max_frame_rate = (quality.max_frame_rate / 2)
                        .max(10)
                        .min(quality.max_frame_rate);
                }
            }
            return (quality != current).then_some(quality);
        }
        self.strained_ms = 0;
        if frames == 0 {
            self.status.reason = AdaptationReason::Idle;
            self.healthy_ms = 0;
            return None;
        }
        // Recovery requires sustained observed frame delivery; no upshift from
        // an unused owner's empty queue or from a clock leap.
        self.healthy_ms = self.healthy_ms.saturating_add(elapsed_ms);
        if self.healthy_ms < 6000 || self.cooldown_ms != 0 {
            return None;
        }
        self.healthy_ms = 0;
        self.cooldown_ms = 2000;
        let mut quality = current;
        let width = source.0.max(current.max_width);
        let height = source.1.max(current.max_height);
        // The host announces its monitor; any u16 by u16 counts in u64.
        let cap = ((u64::from(width) * u64::from(height) * 3) / 1000).clamp(6000, 50_000) as u32;
        quality.max_bitrate_kbps = quality
            .max_bitrate_kbps
            .saturating_add((quality.max_bitrate_kbps / 4).max(250))
            .min(cap)
            .max(current.max_bitrate_kbps);
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
            .max(current.max_frame_rate);
        quality.max_width = quality
            .max_width
            .saturating_add((quality.max_width / 4).max(1))
            .min(width);
        quality.max_height = quality
            .max_height
            .saturating_add((quality.max_height / 4).max(1))
            .min(height);
        if quality != current {
            self.status.reason = AdaptationReason::Recovering;
            Some(quality)
        } else {
            None
        }
    }
}

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
        assert!(state
            .step(quality, (1920, 1080), 1000, frame_sample(0))
            .is_none());
        for second in 1..6 {
            assert!(state
                .step(quality, (1920, 1080), 1000, frame_sample(second))
                .is_none());
        }
        quality = state
            .step(quality, (1920, 1080), 1000, frame_sample(6))
            .unwrap();
        assert_eq!(quality, Quality::new(1250, 25, 1600, 900).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::Recovering);
        assert_eq!(state.status.received_fps, 30);
        assert_eq!(state.status.received_kbps, 2000);
    }
    /// Found by the `host_feedback` fuzz target: the host's monitor size
    /// comes from its Accept, and the largest one overflowed the bitrate
    /// cap's pixel count when the link recovered.
    #[test]
    fn the_largest_monitor_a_host_announces_caps_recovery_without_overflow() {
        let mut state = Adaptive::default();
        let quality = Quality::new(1000, 15, 1280, 720).unwrap();
        let monitor = (u16::MAX, u16::MAX);
        state.step(quality, monitor, 1000, frame_sample(0));
        for second in 1..6 {
            assert!(state
                .step(quality, monitor, 1000, frame_sample(second))
                .is_none());
        }
        let next = state.step(quality, monitor, 1000, frame_sample(6)).unwrap();
        assert_eq!(next.max_bitrate_kbps, 1250);
        assert_eq!(state.status.reason, AdaptationReason::Recovering);
    }
    #[test]
    fn decoder_pressure_reduces_frame_rate_before_bitrate_or_text_resolution() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, (1920, 1080), 1000, frame_sample(0));
        let mut first = frame_sample(1);
        first.receiver.queued = 7;
        first.receiver.oldest_ms = 200;
        assert!(state.step(quality, (1920, 1080), 1000, first).is_none());
        let mut second = frame_sample(2);
        second.receiver.queued = 6;
        second.receiver.oldest_ms = 190;
        let next = state.step(quality, (1920, 1080), 1000, second).unwrap();
        assert_eq!(next, Quality::new(8000, 45, 1920, 1080).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::ReceiverPressure);
        assert_eq!(state.status.send_loss_per_mille, 0);
    }
    #[test]
    fn sender_loss_reduces_bitrate_and_cooldown_prevents_per_tick_oscillation() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, (1920, 1080), 1000, frame_sample(0));
        let mut sample = frame_sample(1);
        sample.host.as_mut().unwrap().lost_bytes = 10_000;
        assert!(state.step(quality, (1920, 1080), 1000, sample).is_none());
        sample = frame_sample(2);
        sample.host.as_mut().unwrap().lost_bytes = 20_000;
        let next = state.step(quality, (1920, 1080), 1000, sample).unwrap();
        assert_eq!(next, Quality::new(6000, 60, 1920, 1080).unwrap());
        assert_eq!(state.status.send_loss_per_mille, 40);
        assert_eq!(state.status.reason, AdaptationReason::NetworkPressure);
        sample = frame_sample(3);
        sample.host.as_mut().unwrap().lost_bytes = 30_000;
        assert!(state.step(next, (1920, 1080), 1000, sample).is_none());
    }
    #[test]
    fn frames_that_keep_going_missing_lower_the_bitrate_and_many_halve_it() {
        let quality = initial();
        let run = |samples: &[(u64, u64)]| {
            let mut state = Adaptive::default();
            state.step(quality, (1920, 1080), 1000, frame_sample(0));
            let mut stepped = None;
            for (second, &(lost, dropped)) in (1..).zip(samples) {
                let mut sample = frame_sample(second);
                sample.frames_lost = lost;
                sample.host.as_mut().unwrap().send_dropped = dropped;
                stepped = stepped.or(state.step(quality, (1920, 1080), 1000, sample));
            }
            stepped.map(|quality| quality.max_bitrate_kbps)
        };
        // The host dropping frames behind the link: two seconds of it.
        assert_eq!(run(&[(0, 2), (0, 4)]), Some(6000));
        assert_eq!(run(&[(0, 10), (0, 20)]), Some(4000));
        // Frames lost at the viewer: three seconds of it.
        assert_eq!(run(&[(2, 0), (4, 0)]), None);
        assert_eq!(run(&[(2, 0), (4, 0), (6, 0), (8, 0)]), Some(6000));
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
        state.step(quality, (1920, 1080), 1000, frame_sample(0));
        let mut sample = frame_sample(1);
        sample.host.as_mut().unwrap().backlog_ms = 300;
        assert!(state.step(quality, (1920, 1080), 1000, sample).is_none());
        let mut sample = frame_sample(2);
        sample.host.as_mut().unwrap().backlog_ms = 280;
        let next = state.step(quality, (1920, 1080), 1000, sample).unwrap();
        assert_eq!(next.max_bitrate_kbps, 6000);
        assert_eq!(state.status.reason, AdaptationReason::NetworkPressure);
    }

    #[test]
    fn idle_counter_reset_and_path_change_never_pretend_to_prove_capacity() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, (3840, 2160), 1000, frame_sample(0));
        for _ in 0..20 {
            assert!(state
                .step(quality, (3840, 2160), 1000, frame_sample(0))
                .is_none());
        }
        assert_eq!(state.status.reason, AdaptationReason::Idle);
        let mut sample = frame_sample(1);
        sample.path_changed = true;
        assert!(state.step(quality, (3840, 2160), 1000, sample).is_none());
        assert_eq!(state.status.reason, AdaptationReason::WarmingUp);
        assert!(state
            .step(quality, (3840, 2160), 1000, frame_sample(0))
            .is_none());
        assert!(state
            .step(quality, (3840, 2160), 20_000, frame_sample(1))
            .is_none());
    }
    #[test]
    fn keyframe_replacement_drops_without_backlog_do_not_lower_quality() {
        let mut state = Adaptive::default();
        let quality = initial();
        state.step(quality, (1920, 1080), 1000, frame_sample(0));
        for second in 1..5 {
            let mut sample = frame_sample(second);
            sample.receiver.dropped = second * 2;
            assert!(state.step(quality, (1920, 1080), 1000, sample).is_none());
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
        };
        let bytes = feedback.encode();
        let decoded = HostFeedback::decode(&bytes).unwrap();
        assert_eq!(decoded.sent_bytes, u64::MAX);
        assert_eq!((decoded.encode_us, decoded.capture_us), (1234, 5678));
        assert_eq!((decoded.send_dropped, decoded.backlog_ms), (42, 150));
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
        let mut malformed = bytes;
        malformed[65] = 9;
        assert!(HostFeedback::decode(&malformed).is_err());
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
        adaptive.step(quality, (1920, 1080), 1000, sample(0));
        for second in 1..=8 {
            if let Some(next) = adaptive.step(quality, (1920, 1080), 1000, sample(second)) {
                quality = next;
            }
        }
        assert_eq!(quality.max_frame_rate, 24);
        assert_eq!(quality.max_bitrate_kbps, 8000);
        assert_eq!((quality.max_width, quality.max_height), (1920, 1080));
        for second in 9..=14 {
            if let Some(next) = adaptive.step(quality, (1920, 1080), 1000, sample(second)) {
                quality = next;
            }
        }
        assert_eq!(quality.max_frame_rate, 30);
        for second in 15..=40 {
            if let Some(next) = adaptive.step(quality, (1920, 1080), 1000, sample(second)) {
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
        state.step(quality, (1920, 1080), 1000, sample(0, 1000));
        assert!(state
            .step(quality, (1920, 1080), 1000, sample(1, 1000))
            .is_none());
        assert_eq!(state.status.reason, AdaptationReason::Stable);
        assert!(state
            .step(quality, (1920, 1080), 1000, sample(2, 30_000))
            .is_none());
        let next = state
            .step(quality, (1920, 1080), 1000, sample(3, 30_000))
            .unwrap();
        assert_eq!(next, Quality::new(8000, 45, 1920, 1080).unwrap());
        assert_eq!(state.status.reason, AdaptationReason::EncoderPressure);
        assert_eq!(state.status.send_loss_per_mille, 0);
        assert_eq!(
            (state.status.encode_us, state.status.capture_us),
            (30_000, 100_000)
        );
    }
}
