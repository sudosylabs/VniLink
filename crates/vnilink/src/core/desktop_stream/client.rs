// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The viewer's side of the Desktop stream: sending commands and receiving pictures.

use super::*;

pub(super) async fn client_send(
    shared: &Shared,
    link: &Connection,
    desktop: &DesktopRuntime,
    send: &mut SendStream,
) -> Result<(), Error> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reports = tokio::time::interval(crate::video::REPORT_EVERY);
    let mut pacing = crate::video::ReportPacing::default();
    reports.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sampled_at = std::time::Instant::now();
    let mut last_path = None;
    let mut received = desktop.stream_received.subscribe();
    let mut acknowledged = None;
    // Samples go as datagrams, numbered; the newest pointer sent that way
    // and not yet repeated on the stream is repeated before the next button.
    let mut sample_seq = 0u32;
    let mut unrepeated: Option<(crate::InputEvent, std::time::Instant)> = None;
    // A display selection is on the stream, so the pointer that follows it
    // goes there too, behind it: on a display not yet selected it is refused.
    let mut after_selection = false;
    loop {
        // The newest stream picture received is said once, as it changes.
        let newest = *received.borrow_and_update();
        if let Some(picture) = newest.filter(|picture| acknowledged != Some(*picture)) {
            acknowledged = Some(picture);
            let mut bytes = vec![COMMAND];
            bytes.extend_from_slice(&desktop.control_state.borrow().1.to_be_bytes());
            bytes.extend_from_slice(
                &ControlMessage::Received {
                    stamp: desktop.stamp,
                    picture,
                }
                .encode(),
            );
            write(send, &bytes).await?;
        }
        // Lease requests precede input batches. Admission takes a free lease;
        // subsequent acquire/yield operations are explicit and bounded.
        let controls: Vec<_> = lock(&desktop.control_commands).drain(..).collect();
        for (control, epoch) in controls {
            let mut bytes = vec![control];
            bytes.extend_from_slice(&epoch.to_be_bytes());
            write(send, &bytes).await?;
        }
        let (release, commands, lease_epoch) = desktop.take_input_epoch();
        if release {
            let mut bytes = vec![RELEASE];
            bytes.extend_from_slice(&lease_epoch.to_be_bytes());
            write(send, &bytes).await?;
        }
        // Where the pointer came to rest is said reliably: the datagram that
        // said it may have been lost, and nothing follows it to correct that.
        if let Some((event, _)) = unrepeated.filter(|(_, at)| at.elapsed() >= POINTER_SETTLE) {
            let message = ControlMessage::Input {
                stamp: desktop.stamp,
                event,
            };
            write(send, &command_bytes(lease_epoch, &message)).await?;
            unrepeated = None;
        }
        let path_rtt = link
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .map_or(Duration::ZERO, |path| path.rtt());
        let frame_period = frame_period(desktop);
        let (ask, retry) = {
            let mut pictures = lock(&desktop.pictures);
            let now = std::time::Instant::now();
            pictures.give_up_quiet(now, path_rtt, frame_period);
            let ask = pictures.should_ask(&desktop.keyframe, now, path_rtt, frame_period);
            let wake = [
                pictures.retry_at(path_rtt, frame_period),
                pictures.quiet_at(path_rtt, frame_period),
            ]
            .into_iter()
            .flatten()
            .min();
            (ask, wake)
        };
        if pacing.due(desktop.report.urgent(), std::time::Instant::now()) {
            let report = desktop.report.take();
            if !report.is_empty() {
                let mut bytes = vec![COMMAND];
                bytes.extend_from_slice(&lease_epoch.to_be_bytes());
                bytes.extend_from_slice(
                    &ControlMessage::VideoReport {
                        stamp: desktop.stamp,
                        report,
                    }
                    .encode(),
                );
                write(send, &bytes).await?;
            }
        }
        if ask {
            desktop.counters.keyframe_requested();
            let mut bytes = vec![COMMAND];
            bytes.extend_from_slice(&lease_epoch.to_be_bytes());
            bytes.extend_from_slice(
                &ControlMessage::Keyframe {
                    stamp: desktop.stamp,
                }
                .encode(),
            );
            write(send, &bytes).await?;
        }
        let notified = desktop.input.notified();
        tokio::pin!(notified);
        for command in commands {
            let kind = match &command {
                Outbound::Event(event) => Some(event.kind()),
                Outbound::Text(_) | Outbound::Chord(_) => Some("typed text"),
                Outbound::Quality(_) | Outbound::Displays(_) => None,
            };
            if let Some(kind) = kind.filter(|_| dropped(shared, DropPoint::Outbox)) {
                input_dropped(desktop.info.service.id, kind, DropPoint::Outbox);
                continue;
            }
            if let Outbound::Event(event) = &command {
                let pointer = matches!(event, crate::InputEvent::Pointer { .. });
                let message = ControlMessage::Input {
                    stamp: desktop.stamp,
                    event: *event,
                };
                if event.sampled() && !(after_selection && pointer) {
                    let mut body = vec![COMMAND];
                    body.extend_from_slice(&lease_epoch.to_be_bytes());
                    body.extend_from_slice(&message.encode());
                    let datagram = super::desktop_input::encode(sample_seq, &body);
                    if link
                        .max_datagram_size()
                        .is_some_and(|max| datagram.len() <= max)
                        && link.send_datagram(datagram).is_ok()
                    {
                        if pointer {
                            unrepeated = Some((*event, std::time::Instant::now()));
                        }
                        sample_seq = sample_seq.wrapping_add(1);
                        continue;
                    }
                } else if pointer {
                    write(send, &command_bytes(lease_epoch, &message)).await?;
                    sample_seq = sample_seq.wrapping_add(1);
                    unrepeated = None;
                    after_selection = false;
                    continue;
                } else if matches!(event, crate::InputEvent::Button { .. }) {
                    if let Some((pointer, _)) = unrepeated.take() {
                        let message = ControlMessage::Input {
                            stamp: desktop.stamp,
                            event: pointer,
                        };
                        write(send, &command_bytes(lease_epoch, &message)).await?;
                    }
                }
            }
            after_selection = match &command {
                Outbound::Displays(_) => true,
                Outbound::Event(crate::InputEvent::Pointer { .. }) => false,
                _ => after_selection,
            };
            let message = match command {
                Outbound::Event(event) => ControlMessage::Input {
                    stamp: desktop.stamp,
                    event,
                },
                Outbound::Text(text) => ControlMessage::Text {
                    stamp: desktop.stamp,
                    text: crate::protocol::KeyText(text),
                },
                Outbound::Chord(text) => ControlMessage::Chord {
                    stamp: desktop.stamp,
                    text: crate::protocol::KeyText(text),
                },
                Outbound::Quality(quality) => ControlMessage::Quality {
                    stamp: desktop.stamp,
                    quality,
                },
                Outbound::Displays(displays) => ControlMessage::SelectDisplays {
                    stamp: desktop.stamp,
                    displays,
                },
            };
            let mut bytes = vec![COMMAND];
            bytes.extend_from_slice(&lease_epoch.to_be_bytes());
            bytes.extend_from_slice(&message.encode());
            write(send, &bytes).await?;
        }
        tokio::select! {
            _ = &mut notified => {}, _ = received.changed() => {}, _ = desktop.keyframe.notified() => {}, _ = desktop.release.notified() => {}, _ = desktop.control_notify.notified() => {},
            // The report itself goes at the top of the loop, when it is due.
            _ = reports.tick() => {},
            _ = async {
                match unrepeated {
                    Some((_, at)) => tokio::time::sleep_until((at + POINTER_SETTLE).into()).await,
                    None => std::future::pending().await,
                }
            } => {},
            // An unanswered keyframe request is asked again when its patience runs out,
            // and a frame gone quiet is given up when its silence does.
            _ = async {
                match retry {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending().await,
                }
            } => {},
            _ = interval.tick() => {
                if let Some(path) = link.paths().iter().find(|p| p.is_selected()) {
                    let now = std::time::Instant::now();
                    let elapsed = now.saturating_duration_since(sampled_at).as_millis().min(u64::MAX as u128) as u64;
                    sampled_at = now;
                    desktop.relayed.store(path.is_relay(), std::sync::atomic::Ordering::Release);
                    let path_changed = last_path.is_some_and(|previous| previous != path.id());
                    last_path = Some(path.id());
                    let host = lock(&desktop.host_feedback).as_ref()
                        .filter(|(_, at)| now.saturating_duration_since(*at) <= Duration::from_secs(3))
                        .map(|(feedback, _)| *feedback);
                    let current = *lock(&desktop.quality);
                    let feedback = crate::adaptation::Feedback {
                        receiver: desktop.inbox.telemetry(), host,
                        frames_lost: desktop.counters.read().lost.total(),
                        rtt_ms: path.rtt().as_millis().min(u64::MAX as u128) as u64,
                        path_changed,
                        decode_us: lock(&desktop.decode_duration).as_ref()
                            .filter(|(_, at)| now.saturating_duration_since(*at) <= Duration::from_secs(3))
                            .map(|(us, _)| *us),
                    };
                    let (next, signals) = {
                        let mut adaptive = lock(&desktop.adaptive);
                        // The quality the host agreed at Accept is what
                        // the viewer chose, within the host's limits.
                        let limits = crate::adaptation::Limits {
                            ceiling: Quality {
                                max_bitrate_kbps: desktop.bitrate_limit,
                                ..desktop.info.quality
                            },
                            codec: crate::VideoCodec::from_wire_id(desktop.info.codec)
                                .unwrap_or(crate::VideoCodec::H264),
                            monitor: (desktop.info.monitor_width, desktop.info.monitor_height),
                            relayed: path.is_relay(),
                        };
                        let next = adaptive.step(current, limits, elapsed, feedback);
                        (next, adaptive.signals)
                    };
                    let decoder = *lock(&desktop.decoder);
                    let observation = crate::video::Observation {
                        elapsed_ms: elapsed,
                        signals,
                        relay: path.is_relay(),
                        received_frames: feedback.receiver.received_frames,
                        keyframe_pending: lock(&desktop.pictures).keyframe_pending(),
                        host: host.map(|host| host.host_state()),
                        decoder_software: decoder.software == Some(true),
                        decoder_recoveries: decoder.recoveries,
                    };
                    lock(&desktop.conditions).observe(crate::diagnostics::monotonic_ms(), &observation);
                    if let Some(next) = next {
                        // Controller-side step: monitor clamp only. The host
                        // applies its encoder caps when the update arrives.
                        let next = crate::session::agree_quality(
                            &next,
                            (desktop.info.monitor_width, desktop.info.monitor_height),
                            &crate::video_caps::VideoCodecCaps::default(),
                        );
                        tracing::info!(
                            target: crate::trace::STREAM,
                            service = desktop.info.service.id,
                            bitrate_kbps = next.max_bitrate_kbps,
                            frame_rate = next.max_frame_rate,
                            width = next.max_width,
                            height = next.max_height,
                            reason = ?lock(&desktop.adaptive).status.reason,
                            "quality changed"
                        );
                        desktop.input.push_quality(next)?;
                        *lock(&desktop.quality) = next;
                    }
                }
            }
        }
    }
}

pub(super) async fn client_receive(
    shared: &Arc<Shared>,
    desktop: &Arc<DesktopRuntime>,
    recv: &mut RecvStream,
) -> Result<(), Error> {
    let stream = async {
        loop {
            let body = read(recv).await?;
            match body[0] {
                CONTROL_STATE if body.len() == 10 => {
                    let status = DesktopControlState::decode(body[1])?;
                    let epoch = u64::from_be_bytes(body[2..10].try_into().unwrap());
                    desktop.receive_control_state(status, epoch);
                }
                RECORDING_STATE => {
                    if body.len() != 10 || body[1] > 1 {
                        return Err(Error::Malformed);
                    }
                    desktop.receive_recording_state(
                        body[1] == 1,
                        u64::from_be_bytes(body[2..10].try_into().unwrap()),
                    )?;
                }
                END => {
                    if body.len() != 2 {
                        return Err(Error::Malformed);
                    }
                    desktop.ended(
                        shared,
                        false,
                        crate::DesktopEnd::Host(crate::StreamEnd::decode(body[1])?),
                    );
                    return Ok(());
                }
                MEDIA_FEEDBACK => {
                    let feedback = crate::adaptation::HostFeedback::decode(&body[1..])?;
                    *lock(&desktop.host_feedback) = Some((feedback, std::time::Instant::now()));
                }
                VIDEO => {
                    let body = crate::video::decode_video_body(&body[1..])?;
                    if !body
                        .frame
                        .belongs_to(desktop.info.codec, desktop.info.stills)
                    {
                        return Err(Error::Malformed);
                    }
                    if body.frame.role != crate::FrameRole::Still {
                        desktop.stream_received.send_replace(Some(body.sequence));
                    }
                    desktop.receive_video(shared, body);
                }
                TOPOLOGY => {
                    let ControlMessage::Topology {
                        stamp,
                        revision,
                        displays,
                    } = ControlMessage::decode(&body[1..])?
                    else {
                        return Err(Error::Malformed);
                    };
                    if stamp == desktop.stamp {
                        *lock(&desktop.topology) = displays.clone();
                        super::topology::store_service_topology(
                            shared,
                            desktop.info.service.connection_id,
                            desktop.info.service.generation,
                            revision,
                            displays,
                        );
                    }
                }
                CURSOR if desktop.info.separate_cursor => {
                    let mut cursor = crate::cursor::decode(&body[1..])?;
                    if cursor.image.is_none() {
                        cursor.image = desktop
                            .cursor
                            .borrow()
                            .as_ref()
                            .and_then(|c| c.image.clone());
                    }
                    desktop.cursor.send_replace(Some(Arc::new(cursor)));
                }
                _ => return Err(Error::Malformed),
            }
        }
    };
    // Datagrams arrive through the connection's router, which owns
    // `read_datagram` so sibling owners never split fragments. This loop
    // consumes only the framed control stream.
    stream.await
}
