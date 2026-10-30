// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What is wrong with a live desktop picture, while it lives.
//!
//! [`crate::adaptation`] decides the quality from one reason per interval;
//! this explains the picture from the same inputs, as a set: a relayed path
//! can be congested and lossy at once, and a slow decoder does not hide a
//! software encoder. The rate controller does not read it. Each condition
//! enters after it held for a few reports and leaves after it was clear for
//! a few, so one bad second neither shows nor hides it, and each carries
//! the measurement that set it and since when.
//!
//! The viewer owns the set: it sees the link, its decoder and what the host
//! reports in [`crate::adaptation::HostFeedback`]. Why a picture *stopped*
//! is not a condition; see [`crate::DesktopEnd`].

use crate::adaptation::Signals;

/// One thing wrong with a live picture. Declared from the most to the least
/// severe: [`ConditionSet::worst`] is the first one held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Condition {
    /// No picture arrived for seconds while the stream is open.
    LinkBlackout,
    /// The relay path holds a queue: its round trip grew several times
    /// over without loss, so the transport never slowed down. A relayed
    /// path that is also losing shows [`Self::LinkCongestion`] instead.
    RelayBacklog,
    /// Video waits behind the link, or the link drops it.
    LinkCongestion,
    /// Data is lost on the way, or pictures keep going missing.
    LinkLoss,
    /// The host's screen is on a desktop it cannot capture, such as a
    /// sign-in or elevation prompt; pictures resume when it leaves it.
    HostSecureDesktop,
    /// The host's encoder takes longer than a picture's interval.
    EncoderSlow,
    /// This computer's decoder, or the pictures waiting for it, fall behind.
    DecoderSlow,
    /// This computer's decoder restarted from a keyframe after an error.
    DecoderRecovering,
    /// The host encodes in software.
    EncoderSoftware,
    /// This computer decodes in software.
    DecoderSoftware,
    /// The connection goes through a relay. Informational: a relay is not
    /// pressure by itself.
    RelayPath,
}

impl Condition {
    /// Every condition, from the most to the least severe.
    pub const ALL: [Self; 11] = [
        Self::LinkBlackout,
        Self::RelayBacklog,
        Self::LinkCongestion,
        Self::LinkLoss,
        Self::HostSecureDesktop,
        Self::EncoderSlow,
        Self::DecoderSlow,
        Self::DecoderRecovering,
        Self::EncoderSoftware,
        Self::DecoderSoftware,
        Self::RelayPath,
    ];

    /// Stable label for diagnostics and the bridge.
    pub fn label(self) -> &'static str {
        match self {
            Self::LinkBlackout => "link.blackout",
            Self::RelayBacklog => "relay.backlog",
            Self::LinkCongestion => "link.congestion",
            Self::LinkLoss => "link.loss",
            Self::HostSecureDesktop => "host.secureDesktop",
            Self::EncoderSlow => "encoder.slow",
            Self::DecoderSlow => "decoder.slow",
            Self::DecoderRecovering => "decoder.recovering",
            Self::EncoderSoftware => "encoder.software",
            Self::DecoderSoftware => "decoder.software",
            Self::RelayPath => "relay.path",
        }
    }

    fn bit(self) -> u16 {
        1 << self as u16
    }

    /// Reports in a row a rule must hold before the condition shows, and be
    /// clear before it goes. A state that is simply true (a relay, a
    /// software coder) shows at once; strain must last.
    fn hysteresis(self) -> (u8, u8) {
        match self {
            Self::LinkBlackout => (1, 1),
            Self::RelayBacklog => (3, 3),
            Self::LinkCongestion | Self::LinkLoss | Self::EncoderSlow | Self::DecoderSlow => (2, 3),
            Self::DecoderRecovering => (1, 5),
            Self::HostSecureDesktop
            | Self::EncoderSoftware
            | Self::DecoderSoftware
            | Self::RelayPath => (1, 1),
        }
    }
}

/// A set of [`Condition`]s, iterated from the most severe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ConditionSet(u16);

impl ConditionSet {
    pub fn contains(self, condition: Condition) -> bool {
        self.0 & condition.bit() != 0
    }
    pub fn insert(&mut self, condition: Condition) {
        self.0 |= condition.bit();
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn iter(self) -> impl Iterator<Item = Condition> {
        Condition::ALL
            .into_iter()
            .filter(move |condition| self.contains(*condition))
    }
    /// The most severe condition held.
    pub fn worst(self) -> Option<Condition> {
        self.iter().next()
    }
}

impl FromIterator<Condition> for ConditionSet {
    fn from_iter<I: IntoIterator<Item = Condition>>(conditions: I) -> Self {
        let mut set = Self::default();
        for condition in conditions {
            set.insert(condition);
        }
        set
    }
}

/// The measurement that set a condition: which signal, its value when the
/// condition entered and the threshold it crossed, in the signal's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    pub signal: &'static str,
    pub value: u64,
    pub threshold: u64,
}

impl Evidence {
    fn new(signal: &'static str, value: u64, threshold: u64) -> Self {
        Self {
            signal,
            value,
            threshold,
        }
    }
}

/// A condition held now: since when, on [`crate::diagnostics::monotonic_ms`],
/// and the evidence that set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionEntry {
    pub condition: Condition,
    pub since_ms: u64,
    pub evidence: Evidence,
}

/// What the host reports of itself beside its link figures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HostState {
    /// Whether its open encoder is hardware; `None` when it cannot tell.
    pub encoder_hardware: Option<bool>,
    pub secure_desktop: bool,
}

/// One report's inputs. The viewer samples them once a second.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Observation {
    pub elapsed_ms: u64,
    /// The controller's own measurements; `None` while it warms up after a
    /// start, a route change or a counter reset.
    pub signals: Option<Signals>,
    pub relay: bool,
    /// Pictures received so far: an unchanged count is silence.
    pub received_frames: u64,
    pub keyframe_pending: bool,
    /// Fresh within the feedback window; `None` without a report.
    pub host: Option<HostState>,
    pub decoder_software: bool,
    /// Decoder restarts so far.
    pub decoder_recoveries: u64,
}

/// No picture for this long is a blackout: an unchanged screen still sends
/// one a second.
const SILENT_BLACKOUT_MS: u64 = 2000;
/// Silence while a keyframe is owed is one sooner.
const SILENT_OWED_MS: u64 = 1000;
/// Send loss, per mille, that counts as loss; the controller's own.
const LOSS_PER_MILLE: u16 = 20;
/// Missed frames lasting this long count as loss; the controller's own.
const LASTING_LOSS_MS: u64 = 3000;
/// Video waiting in the host or its transport this long is congestion.
const QUEUE_MS: u64 = 150;
/// A relayed round trip this many times its floor, and at least
/// [`RELAY_BACKLOG_ADDED_MS`] over it, without loss, is a relay backlog.
/// The relay collapse grew 47 ms to seconds with no loss.
const RELAY_BACKLOG_FACTOR: u64 = 4;
const RELAY_BACKLOG_ADDED_MS: u64 = 500;
/// Send loss below this, per mille, is "no loss" for a relay backlog.
const RELAY_BACKLOG_LOSS: u16 = 5;

#[derive(Debug, Clone, Copy, Default)]
struct Tracked {
    entered: Option<(u64, Evidence)>,
    held: u8,
    clear: u8,
}

/// The live set with its hysteresis. One per viewer pipeline.
#[derive(Debug, Default)]
pub(crate) struct Conditions {
    tracked: [Tracked; Condition::ALL.len()],
    silent_ms: u64,
    last_frames: Option<u64>,
    last_recoveries: Option<u64>,
}

impl Conditions {
    /// Takes one report at `now_ms` and returns the set held after it.
    pub fn observe(&mut self, now_ms: u64, observation: &Observation) -> ConditionSet {
        let frames = observation.received_frames;
        self.silent_ms = match self.last_frames {
            Some(before) if frames == before => {
                self.silent_ms.saturating_add(observation.elapsed_ms)
            }
            _ => 0,
        };
        self.last_frames = Some(frames);
        let recoveries = observation.decoder_recoveries.saturating_sub(
            self.last_recoveries
                .unwrap_or(observation.decoder_recoveries),
        );
        self.last_recoveries = Some(observation.decoder_recoveries);
        let found = evaluate(observation, self.silent_ms, recoveries);
        for (index, condition) in Condition::ALL.into_iter().enumerate() {
            let (enter, leave) = condition.hysteresis();
            let tracked = &mut self.tracked[index];
            match found[index] {
                Some(evidence) => {
                    tracked.clear = 0;
                    tracked.held = tracked.held.saturating_add(1);
                    if tracked.entered.is_none() && tracked.held >= enter {
                        tracked.entered = Some((now_ms, evidence));
                    }
                }
                None => {
                    tracked.held = 0;
                    tracked.clear = tracked.clear.saturating_add(1);
                    if tracked.clear >= leave {
                        tracked.entered = None;
                    }
                }
            }
        }
        self.active()
    }

    pub fn active(&self) -> ConditionSet {
        Condition::ALL
            .into_iter()
            .zip(&self.tracked)
            .filter(|(_, tracked)| tracked.entered.is_some())
            .map(|(condition, _)| condition)
            .collect()
    }

    /// The held conditions with their evidence, from the most severe.
    pub fn entries(&self) -> Vec<ConditionEntry> {
        Condition::ALL
            .into_iter()
            .zip(&self.tracked)
            .filter_map(|(condition, tracked)| {
                tracked.entered.map(|(since_ms, evidence)| ConditionEntry {
                    condition,
                    since_ms,
                    evidence,
                })
            })
            .collect()
    }
}

/// Which rules hold for one report, without hysteresis, indexed as
/// [`Condition::ALL`]. `silent_ms` is how long no picture arrived and
/// `recoveries` the decoder restarts since the last report.
fn evaluate(
    observation: &Observation,
    silent_ms: u64,
    recoveries: u64,
) -> [Option<Evidence>; Condition::ALL.len()] {
    let mut found = [None; Condition::ALL.len()];
    let mut set = |condition: Condition, evidence: Option<Evidence>| {
        found[condition as usize] = evidence;
    };
    let blackout = if observation.keyframe_pending {
        SILENT_OWED_MS
    } else {
        SILENT_BLACKOUT_MS
    };
    set(
        Condition::LinkBlackout,
        (silent_ms >= blackout).then(|| Evidence::new("silent_ms", silent_ms, blackout)),
    );
    set(
        Condition::RelayPath,
        observation.relay.then(|| Evidence::new("relay", 1, 1)),
    );
    if let Some(host) = observation.host {
        set(
            Condition::EncoderSoftware,
            (host.encoder_hardware == Some(false))
                .then(|| Evidence::new("host_encoder_hardware", 0, 1)),
        );
        set(
            Condition::HostSecureDesktop,
            host.secure_desktop
                .then(|| Evidence::new("host_secure_desktop", 1, 1)),
        );
    }
    set(
        Condition::DecoderSoftware,
        observation
            .decoder_software
            .then(|| Evidence::new("decoder_hardware", 0, 1)),
    );
    set(
        Condition::DecoderRecovering,
        (recoveries > 0).then(|| Evidence::new("decoder_recoveries", recoveries, 1)),
    );
    let Some(signals) = observation.signals else {
        return found;
    };
    set(
        Condition::LinkLoss,
        if signals.loss_per_mille >= LOSS_PER_MILLE {
            Some(Evidence::new(
                "send_loss_permille",
                u64::from(signals.loss_per_mille),
                u64::from(LOSS_PER_MILLE),
            ))
        } else if signals.lasting_loss_ms >= LASTING_LOSS_MS {
            Some(Evidence::new(
                "viewer_loss_ms",
                signals.lasting_loss_ms,
                LASTING_LOSS_MS,
            ))
        } else {
            None
        },
    );
    let inflated = signals.inflated_rtt_ms;
    set(
        Condition::LinkCongestion,
        if signals.backlog_ms >= QUEUE_MS {
            Some(Evidence::new("backlog_ms", signals.backlog_ms, QUEUE_MS))
        } else if signals.host_queue_backed_up {
            Some(Evidence::new(
                "host_queue_ms",
                signals.host_queue_ms,
                QUEUE_MS,
            ))
        } else if signals.gate_dropping {
            Some(Evidence::new(
                "send_dropped_permille",
                signals.missed_per_mille,
                crate::adaptation::MISSED_PRESSURE,
            ))
        } else if signals.rtt_ms >= inflated && signals.loss_per_mille >= 5 {
            Some(Evidence::new("rtt_ms", signals.rtt_ms, inflated))
        } else if signals.rtt_ms >= inflated && signals.congestion_events > 0 {
            Some(Evidence::new(
                "congestion_events",
                signals.congestion_events,
                1,
            ))
        } else {
            None
        },
    );
    let floor = signals.rtt_floor_ms.max(1);
    let backlog = floor
        .saturating_mul(RELAY_BACKLOG_FACTOR)
        .max(floor.saturating_add(RELAY_BACKLOG_ADDED_MS));
    set(
        Condition::RelayBacklog,
        (observation.relay
            && signals.rtt_ms >= backlog
            && signals.loss_per_mille < RELAY_BACKLOG_LOSS)
            .then(|| Evidence::new("rtt_ms", signals.rtt_ms, backlog)),
    );
    set(
        Condition::EncoderSlow,
        signals
            .encoder_pressure
            .then(|| Evidence::new("encode_us", signals.encode_us, signals.picture_budget_us)),
    );
    set(
        Condition::DecoderSlow,
        if signals.decoder_pressure {
            Some(Evidence::new(
                "decode_us",
                signals.decode_us,
                signals.picture_budget_us,
            ))
        } else if signals.receiver_backed_up {
            Some(Evidence::new(
                "receiver_queue_ms",
                signals.receiver_queue_ms,
                QUEUE_MS,
            ))
        } else {
            None
        },
    );
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Signals {
        Signals {
            rtt_ms: 40,
            rtt_floor_ms: 40,
            inflated_rtt_ms: 120,
            picture_budget_us: 25_000,
            ..Default::default()
        }
    }

    /// One report a second, each with a fresh picture count, until `reports`.
    fn run(
        conditions: &mut Conditions,
        reports: u64,
        mut make: impl FnMut(u64) -> Observation,
    ) -> ConditionSet {
        let mut set = ConditionSet::default();
        for second in 0..reports {
            set = conditions.observe(second * 1000, &make(second));
        }
        set
    }

    fn flowing(second: u64, signals: Signals) -> Observation {
        Observation {
            elapsed_ms: 1000,
            signals: Some(signals),
            received_frames: second * 30,
            ..Default::default()
        }
    }

    #[test]
    fn a_healthy_direct_picture_holds_nothing() {
        let mut conditions = Conditions::default();
        assert!(run(&mut conditions, 10, |second| flowing(second, quiet())).is_empty());
    }

    #[test]
    fn send_loss_shows_after_two_reports_and_leaves_after_three_clear() {
        let mut conditions = Conditions::default();
        let lossy = Signals {
            loss_per_mille: 30,
            ..quiet()
        };
        let set = conditions.observe(0, &flowing(1, lossy));
        assert!(set.is_empty(), "one lossy second is not loss yet");
        let set = conditions.observe(1000, &flowing(2, lossy));
        assert_eq!(set.worst(), Some(Condition::LinkLoss));
        let entry = conditions.entries()[0];
        assert_eq!(entry.since_ms, 1000);
        assert_eq!(entry.evidence, Evidence::new("send_loss_permille", 30, 20));
        for second in 3..5 {
            assert!(conditions
                .observe(second * 1000, &flowing(second, quiet()))
                .contains(Condition::LinkLoss));
        }
        assert!(conditions.observe(5000, &flowing(5, quiet())).is_empty());
    }

    #[test]
    fn lasting_missed_frames_are_loss_at_the_controllers_threshold() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 2, |second| {
            flowing(
                second,
                Signals {
                    lasting_loss_ms: 3000,
                    ..quiet()
                },
            )
        });
        assert_eq!(set.worst(), Some(Condition::LinkLoss));
        assert_eq!(conditions.entries()[0].evidence.signal, "viewer_loss_ms");
    }

    #[test]
    fn a_backlog_queue_or_send_gate_drops_are_congestion() {
        for signals in [
            Signals {
                backlog_ms: 200,
                ..quiet()
            },
            Signals {
                host_queue_backed_up: true,
                host_queue_ms: 180,
                ..quiet()
            },
            Signals {
                gate_dropping: true,
                missed_per_mille: 80,
                ..quiet()
            },
            Signals {
                rtt_ms: 200,
                loss_per_mille: 6,
                ..quiet()
            },
            Signals {
                rtt_ms: 200,
                congestion_events: 3,
                ..quiet()
            },
        ] {
            let mut conditions = Conditions::default();
            let set = run(&mut conditions, 2, |second| flowing(second, signals));
            assert!(set.contains(Condition::LinkCongestion), "{signals:?}");
        }
    }

    #[test]
    fn round_trip_alone_or_congestion_events_alone_are_not_congestion() {
        for signals in [
            Signals {
                rtt_ms: 400,
                ..quiet()
            },
            Signals {
                congestion_events: 4,
                ..quiet()
            },
        ] {
            let mut conditions = Conditions::default();
            assert!(run(&mut conditions, 5, |second| flowing(second, signals)).is_empty());
        }
    }

    #[test]
    fn silence_is_a_blackout_at_once_and_ends_with_the_next_picture() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 4, |second| Observation {
            elapsed_ms: 1000,
            signals: Some(quiet()),
            received_frames: if second == 0 { 30 } else { 31 },
            ..Default::default()
        });
        assert_eq!(set.worst(), Some(Condition::LinkBlackout));
        assert_eq!(
            conditions.entries()[0].evidence.threshold,
            SILENT_BLACKOUT_MS
        );
        let set = conditions.observe(4000, &flowing(10, quiet()));
        assert!(!set.contains(Condition::LinkBlackout));
    }

    #[test]
    fn an_owed_keyframe_makes_one_silent_second_a_blackout() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 2, |_| Observation {
            elapsed_ms: 1000,
            received_frames: 30,
            keyframe_pending: true,
            ..Default::default()
        });
        assert_eq!(set.worst(), Some(Condition::LinkBlackout));
    }

    #[test]
    fn a_relay_is_informational_and_its_lossless_backlog_is_named() {
        let mut conditions = Conditions::default();
        let relay = |second, rtt_ms| Observation {
            relay: true,
            ..flowing(
                second,
                Signals {
                    rtt_ms,
                    rtt_floor_ms: 47,
                    inflated_rtt_ms: 141,
                    ..quiet()
                },
            )
        };
        assert_eq!(
            conditions
                .observe(0, &relay(1, 47))
                .iter()
                .collect::<Vec<_>>(),
            [Condition::RelayPath]
        );
        // The collapse: seconds of round trip, nothing lost.
        let set = run(&mut conditions, 3, |second| relay(second + 2, 4000));
        assert_eq!(set.worst(), Some(Condition::RelayBacklog));
        assert!(set.contains(Condition::RelayPath));
        assert!(!set.contains(Condition::LinkCongestion));
        assert_eq!(
            conditions.entries()[0].evidence,
            Evidence::new("rtt_ms", 4000, 547)
        );
    }

    #[test]
    fn a_lossy_relay_reports_congestion_and_not_a_backlog() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 4, |second| Observation {
            relay: true,
            ..flowing(
                second,
                Signals {
                    rtt_ms: 900,
                    rtt_floor_ms: 50,
                    inflated_rtt_ms: 150,
                    loss_per_mille: 40,
                    ..quiet()
                },
            )
        });
        assert!(set.contains(Condition::LinkCongestion));
        assert!(set.contains(Condition::LinkLoss));
        assert!(!set.contains(Condition::RelayBacklog));
    }

    #[test]
    fn slow_coders_are_named_by_side() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 2, |second| {
            flowing(
                second,
                Signals {
                    encoder_pressure: true,
                    encode_us: 40_000,
                    decoder_pressure: true,
                    decode_us: 30_000,
                    ..quiet()
                },
            )
        });
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Condition::EncoderSlow, Condition::DecoderSlow]
        );
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 2, |second| {
            flowing(
                second,
                Signals {
                    receiver_backed_up: true,
                    receiver_queue_ms: 200,
                    ..quiet()
                },
            )
        });
        assert_eq!(set.worst(), Some(Condition::DecoderSlow));
        assert_eq!(conditions.entries()[0].evidence.signal, "receiver_queue_ms");
    }

    #[test]
    fn software_coders_and_the_secure_desktop_come_from_their_reports() {
        let mut conditions = Conditions::default();
        let set = conditions.observe(
            0,
            &Observation {
                host: Some(HostState {
                    encoder_hardware: Some(false),
                    secure_desktop: true,
                }),
                decoder_software: true,
                ..flowing(1, quiet())
            },
        );
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [
                Condition::HostSecureDesktop,
                Condition::EncoderSoftware,
                Condition::DecoderSoftware
            ]
        );
        // An encoder that cannot tell is not called software.
        let mut conditions = Conditions::default();
        let set = conditions.observe(
            0,
            &Observation {
                host: Some(HostState::default()),
                ..flowing(1, quiet())
            },
        );
        assert!(set.is_empty());
    }

    #[test]
    fn a_decoder_restart_holds_recovering_for_five_reports() {
        let mut conditions = Conditions::default();
        let mut recoveries = 0;
        let mut observe = |second: u64, recoveries: u64| {
            conditions.observe(
                second * 1000,
                &Observation {
                    decoder_recoveries: recoveries,
                    ..flowing(second, quiet())
                },
            )
        };
        assert!(observe(1, recoveries).is_empty());
        recoveries += 1;
        assert!(observe(2, recoveries).contains(Condition::DecoderRecovering));
        for second in 3..7 {
            assert!(observe(second, recoveries).contains(Condition::DecoderRecovering));
        }
        assert!(observe(7, recoveries).is_empty());
    }

    #[test]
    fn warming_up_keeps_only_the_states_that_need_no_measurement() {
        let mut conditions = Conditions::default();
        let set = run(&mut conditions, 3, |second| Observation {
            relay: true,
            signals: None,
            ..flowing(second, quiet())
        });
        assert_eq!(set.iter().collect::<Vec<_>>(), [Condition::RelayPath]);
    }
}
