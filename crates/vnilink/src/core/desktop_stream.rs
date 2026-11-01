// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Desktop instances on a neutral parent. Every instance owns its stream,
//! queues and datagram epoch; session APIs cannot publish into this backend.
// trace: records every ignored failure
use super::drops::{dropped, input_dropped, DropPoint};

use super::host_input::{Facts, Holder, HostInputSession, InFlight, InputOutcome, Lease, Refusal};

use super::*;

use crate::input::{HeldInput, Outbound};

use crate::protocol::{ControlMessage, SessionStamp};

use crate::video::{
    EncodedFrame, KeyframeFlag, VideoInbox, VideoOutbox, VideoPush, VideoSubscription,
};

use crate::{CloseCause, CursorState, ServiceInfo, ServiceKind, ServicePhase};

use iroh::endpoint::{Connection, RecvStream, SendStream};

pub(super) const TAG: u8 = 10;

/// Live Desktop owners allowed on one connection. Each owner carries its
/// own selected display, codec, input, clipboard and cancellation, while
/// remote-control arbitration stays a single explicit lease. Four covers
/// realistic multi-monitor walls without unbounded encoder/stream state.
pub(super) const MAX_DESKTOP_OWNERS_PER_CONNECTION: usize = 4;

const VERSION: u8 = 1;

const VIDEO: u8 = 1;

const CURSOR: u8 = 2;

pub(super) const COMMAND: u8 = 3;

const RELEASE: u8 = 4;

const TOPOLOGY: u8 = 5;

const ACQUIRE_CONTROL: u8 = 6;

const YIELD_CONTROL: u8 = 7;

const CONTROL_STATE: u8 = 8;

const MEDIA_FEEDBACK: u8 = 9;

const RECORDING_STATE: u8 = 10;

/// The host's last message before it finishes the stream: one byte of
/// [`crate::StreamEnd`]. A stream finished without it ended for no reason
/// the host could give.
const END: u8 = 11;

/// How long a host waits to say why before finishing anyway: the link
/// may be the reason.
const END_PATIENCE: Duration = Duration::from_secs(1);

/// Host-confirmed input ownership. Contention never closes the video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopControlState {
    Viewing,
    Controlling,
    Contended,
}

impl DesktopControlState {
    fn encode(self) -> u8 {
        match self {
            Self::Viewing => 0,
            Self::Controlling => 1,
            Self::Contended => 2,
        }
    }
    fn decode(value: u8) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Viewing),
            1 => Ok(Self::Controlling),
            2 => Ok(Self::Contended),
            _ => Err(Error::Malformed),
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Viewing => "viewing",
            Self::Controlling => "controlling",
            Self::Contended => "contended",
        }
    }
}

const MAGIC: &[u8; 4] = b"DSK1";

/// Desktop Open request flags (byte 25): the viewer takes a separate cursor,
/// and its window shows still pictures.
const OPEN_CURSOR: u8 = 1;

const OPEN_STILLS: u8 = 2;

const PREFIX: usize = 20;

/// The longest message on the desktop's stream: a cursor, or a video message
/// whose tag, header, regions and payload are each at their largest.
const MAX: usize = {
    let video =
        1 + crate::video::VIDEO_HEADER + crate::video::MAX_REGION_BYTES + crate::MAX_VIDEO_FRAME;
    let longest = if video > crate::cursor::MAX_BODY {
        video
    } else {
        crate::cursor::MAX_BODY
    };
    longest + 64
};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopInfo {
    pub service: ServiceInfo,
    pub monitor_width: u16,
    pub monitor_height: u16,
    pub codec: u8,
    pub quality: Quality,
    pub separate_cursor: bool,
    /// The host sends AV1 4:4:4 still pictures of a still screen: the
    /// viewer's window shows them and the host's capture sends them. Agreed
    /// at Open for the owner's lifetime.
    pub stills: bool,
    /// The matrix the stream is converted and declared in, agreed at Open
    /// from the viewer's window and fixed for the owner's lifetime.
    pub matrix: crate::YuvMatrix,
}

pub(super) struct DesktopRuntime {
    recording_allowed: tokio::sync::watch::Sender<(bool, u64)>,
    recording_gate: Mutex<()>,
    pub(super) activity: Mutex<super::activity::Activity>,
    pub(super) info: DesktopInfo,
    pub(super) stamp: SessionStamp,
    quality: Mutex<Quality>,
    /// Viewer: the most bandwidth its user allows, whatever `info.quality`
    /// started the stream at.
    bitrate_limit: u32,
    /// What the link carries now, to size still pictures by.
    link_rate: Mutex<crate::video::LinkRate>,
    /// Host: the largest datagram the link last took, less its prefix; 0
    /// until the first, or while video goes on a stream.
    datagram: std::sync::atomic::AtomicUsize,
    adaptive: Mutex<crate::adaptation::Adaptive>,
    decode_duration: Mutex<Option<(u64, std::time::Instant)>>,
    producer_duration: Mutex<Option<(u64, u64, std::time::Instant)>>,
    host_feedback: Mutex<Option<(crate::adaptation::HostFeedback, std::time::Instant)>>,
    /// Viewer: what is wrong with the picture now; see [`crate::Condition`].
    conditions: Mutex<crate::video::Conditions>,
    /// Viewer: what the window's decoder reports of itself.
    decoder: Mutex<DecoderState>,
    /// Host: the encoder open for this viewer, once the producer said.
    encoder: Mutex<Option<crate::EncoderImplementation>>,
    /// Why the picture stopped: on the viewer what it concluded, on the
    /// host what it told the viewer. The first answer stands.
    end: Mutex<Option<crate::DesktopEnd>>,
    /// Viewer: whether the selected path went through a relay at the last
    /// sample, so a lost connection can say which.
    relayed: std::sync::atomic::AtomicBool,
    input: Arc<InputOutbox>,
    inbox: Arc<VideoInbox>,
    subscriber: Mutex<Weak<()>>,
    video: Arc<VideoOutbox>,
    cursor: tokio::sync::watch::Sender<Option<Arc<CursorState>>>,
    pub(super) keyframe: KeyframeFlag,
    release: KeyframeFlag,
    /// Losses, keyframes and encoder reopens, for diagnostics.
    pub(super) counters: crate::video::VideoCounters,
    /// Viewer: what it has lost since its last report to the host.
    report: crate::video::ReportTally,
    /// Host: what its viewer reports losing, smoothed.
    viewer_loss: Mutex<crate::video::ViewerLoss>,
    /// Host: the stream pictures its viewer has not yet acknowledged, which
    /// pace what the stream is given; datagram pictures are never in it.
    stream_flight: Mutex<crate::video::InFlight>,
    /// Viewer: the newest stream picture received, for the send loop to
    /// acknowledge.
    stream_received: tokio::sync::watch::Sender<Option<u32>>,
    /// Viewer: its datagrams' reassembly and its outstanding keyframe
    /// request, so losses while one is coming do not ask again.
    pictures: Mutex<crate::video::ViewerPictures>,
    control_state: tokio::sync::watch::Sender<(DesktopControlState, u64)>,
    control_commands: Mutex<std::collections::VecDeque<(u8, u64)>>,
    control_notify: tokio::sync::Notify,
    /// Host: the input it applied under this desktop's leases and has not
    /// released, apart from the viewer's own plan.
    pub(super) host_input: HostInputSession,
    /// Host: the newest pointer position applied, from either transport.
    pub(super) pointer_order: super::desktop_input::PointerOrder,
    pub(super) retired: std::sync::atomic::AtomicBool,
    /// The one display this owner shows. Empty means none, never a substitute.
    pub(super) selected: Mutex<Vec<u64>>,
    pub(super) display_fault: Mutex<Option<&'static str>>,
    /// Topology the host sent to this controller. Empty until the first snapshot.
    topology: Mutex<Vec<crate::DisplayInfo>>,
    pub(super) keyboard: Mutex<DesktopKeyboard>,
    pub(super) clipboard: super::desktop_clipboard::ClipboardState,
    #[cfg(any(test, feature = "test-probes"))]
    force_stream: std::sync::atomic::AtomicBool,
}

/// The window decoder's own report: whether it decodes in software, and
/// how often it restarted from a keyframe after an error.
#[derive(Debug, Clone, Copy, Default)]
struct DecoderState {
    /// `None` until the window said.
    software: Option<bool>,
    recoveries: u64,
    /// Still pictures the window showed.
    stills_shown: u64,
}

pub(super) struct DesktopKeyboard {
    pub(super) pointer: crate::input::PointerPlan,
    pub(super) plan: crate::keyboard::KeyPlan,
    pub(super) held: HeldInput,
    pub(super) mode: crate::KeyboardMode,
    pub(super) swap: bool,
}

impl DesktopKeyboard {
    pub(super) fn idle(&self) -> bool {
        !self.plan.is_held() && self.held.release_events().is_empty()
    }
}

impl DesktopRuntime {
    pub(super) fn set_recording_allowed(&self, allowed: bool) {
        let _gate = lock(&self.recording_gate);
        let (previous, epoch) = *self.recording_allowed.borrow();
        if previous == allowed {
            return;
        }
        let next = epoch.saturating_add(1);
        self.recording_allowed
            .send_replace((allowed && next > epoch, next));
        self.inbox.close_taps();
    }
    fn receive_recording_state(&self, allowed: bool, epoch: u64) -> Result<(), Error> {
        let _gate = lock(&self.recording_gate);
        let previous = *self.recording_allowed.borrow();
        if epoch < previous.1 || epoch == previous.1 && allowed != previous.0 {
            return Err(Error::StaleSession);
        }
        if epoch != previous.1 {
            self.inbox.close_taps();
        }
        self.recording_allowed.send_replace((allowed, epoch));
        Ok(())
    }
    fn new(info: DesktopInfo, stamp: SessionStamp, bitrate_limit: u32) -> Arc<Self> {
        Arc::new(Self {
            recording_allowed: tokio::sync::watch::channel((false, 0)).0,
            recording_gate: Mutex::new(()),
            activity: Mutex::new(super::activity::Activity::new(std::time::Instant::now())),
            quality: Mutex::new(info.quality),
            bitrate_limit,
            link_rate: Mutex::new(crate::video::LinkRate::default()),
            datagram: std::sync::atomic::AtomicUsize::new(0),
            adaptive: Mutex::new(crate::adaptation::Adaptive::default()),
            decode_duration: Mutex::new(None),
            producer_duration: Mutex::new(None),
            host_feedback: Mutex::new(None),
            conditions: Mutex::new(Default::default()),
            decoder: Mutex::new(DecoderState::default()),
            encoder: Mutex::new(None),
            end: Mutex::new(None),
            relayed: std::sync::atomic::AtomicBool::new(false),
            info,
            stamp,
            input: Arc::new(InputOutbox::new()),
            inbox: Arc::new(VideoInbox::new()),
            subscriber: Mutex::new(Weak::new()),
            video: Arc::new(VideoOutbox::new()),
            cursor: tokio::sync::watch::channel(None).0,
            keyframe: KeyframeFlag::new(),
            release: KeyframeFlag::new(),
            counters: crate::video::VideoCounters::default(),
            report: Default::default(),
            viewer_loss: Mutex::new(Default::default()),
            stream_flight: Mutex::new(Default::default()),
            stream_received: tokio::sync::watch::channel(None).0,
            pictures: Mutex::new(crate::video::ViewerPictures::new()),
            control_state: tokio::sync::watch::channel((DesktopControlState::Viewing, 0)).0,
            control_commands: Mutex::new(std::collections::VecDeque::new()),
            control_notify: tokio::sync::Notify::new(),
            host_input: HostInputSession::new(),
            pointer_order: Default::default(),
            retired: std::sync::atomic::AtomicBool::new(false),
            selected: Mutex::new(Vec::new()),
            display_fault: Mutex::new(None),
            topology: Mutex::new(Vec::new()),
            keyboard: Mutex::new(DesktopKeyboard {
                pointer: crate::input::PointerPlan::default(),
                plan: crate::keyboard::KeyPlan::new(),
                held: HeldInput::default(),
                mode: crate::KeyboardMode::Character,
                swap: false,
            }),
            clipboard: super::desktop_clipboard::ClipboardState::new(),
            #[cfg(any(test, feature = "test-probes"))]
            force_stream: std::sync::atomic::AtomicBool::new(false),
        })
    }
    /// Records why the picture stopped, unless an earlier answer stands.
    /// The first answer is also the computer's last end, for diagnostics.
    fn ended(&self, shared: &Shared, hosted: bool, end: crate::DesktopEnd) {
        let mut slot = lock(&self.end);
        if slot.is_none() {
            tracing::info!(
                target: crate::trace::SESSION,
                service = self.info.service.id,
                hosted,
                reason = end.label(),
                "desktop ended"
            );
            *slot = Some(end);
            *lock(&shared.last_desktop_end) = Some(crate::diagnostics::EndSummary {
                service_id: self.info.service.id,
                hosted,
                reason: end.label().into(),
                at_ms: crate::diagnostics::monotonic_ms(),
            });
        }
    }
    /// Whether the host said it ended the whole connection, so the
    /// desktop went with its parent rather than by itself.
    fn host_ended_connection(&self) -> bool {
        matches!(
            *lock(&self.end),
            Some(crate::DesktopEnd::Host(
                crate::StreamEnd::Disconnected | crate::StreamEnd::Shutdown
            ))
        )
    }
    pub(super) fn close(&self) {
        self.retired
            .store(true, std::sync::atomic::Ordering::Release);
        let mut keyboard = lock(&self.keyboard);
        keyboard.plan.clear();
        keyboard.pointer.reset();
        keyboard.held = HeldInput::default();
        self.inbox.close();
        self.input.clear();
        self.input.fail(&Error::NoSession);
        self.cursor.send_replace(None);
        self.clipboard.close();
    }
    fn plan_key(&self, key: crate::LocalKey<'_>) -> Result<(), Error> {
        let mut keyboard = lock(&self.keyboard);
        let mut trial = keyboard.plan.clone();
        let items = trial
            .apply(keyboard.mode, keyboard.swap, key)?
            .into_iter()
            .map(crate::keyboard::Planned::outbound)
            .collect::<Vec<_>>();
        self.input.push_batch(&items)?;
        for item in &items {
            if let Outbound::Event(event) = item {
                keyboard.held.observe(*event);
                if self.control_state.borrow().0 == DesktopControlState::Controlling {
                    lock(&self.activity).input(*event, std::time::Instant::now());
                }
            }
        }
        if !items.is_empty() && self.control_state.borrow().0 == DesktopControlState::Controlling {
            lock(&self.activity).touch(std::time::Instant::now());
        }
        keyboard.plan = trial;
        Ok(())
    }
    fn release_input(&self) {
        let mut keyboard = lock(&self.keyboard);
        let dropped = self.input.clear_pending_input();
        if dropped > 0 {
            tracing::debug!(
                target: crate::trace::INPUT,
                service = self.info.service.id,
                dropped,
                why = "input was released",
                "input discarded"
            );
        }
        keyboard.plan.clear();
        keyboard.pointer.reset();
        keyboard.held = HeldInput::default();
        self.release.request();
    }
    #[cfg(test)]
    fn take_input(&self) -> (bool, Vec<Outbound>) {
        let (release, commands, _) = self.take_input_epoch();
        (release, commands)
    }
    fn take_input_epoch(&self) -> (bool, Vec<Outbound>, u64) {
        // Epoch, release and input are sampled under the same lock used when
        // a host ownership update invalidates pending controller input.
        let _keyboard = lock(&self.keyboard);
        (
            self.release.take(),
            self.input.drain(),
            self.control_state.borrow().1,
        )
    }
    fn receive_control_state(&self, status: DesktopControlState, epoch: u64) {
        let mut keyboard = lock(&self.keyboard);
        let previous = *self.control_state.borrow();
        if previous != (status, epoch) {
            tracing::info!(
                target: crate::trace::INPUT,
                service = self.info.service.id,
                state = ?status,
                epoch,
                "control changed"
            );
            // Input queued under the old control is not sent under the new.
            let dropped = self.input.clear_pending_input();
            if dropped > 0 {
                tracing::debug!(
                    target: crate::trace::INPUT,
                    service = self.info.service.id,
                    dropped,
                    why = "control changed before it was sent",
                    "input discarded"
                );
            }
            self.release.take();
            keyboard.plan.clear();
            keyboard.pointer.reset();
            keyboard.held = HeldInput::default();
            self.control_state.send_replace((status, epoch));
        }
    }
    /// What a still picture, and anything queued ahead of it, may take on
    /// the wire now: what the link carries, capped at the agreed bitrate,
    /// within [`crate::STILL_DRAIN`]. The send gate holds a still to this.
    fn still_drain(&self) -> usize {
        let agreed = u64::from(lock(&self.quality).max_bitrate_kbps) * 1000;
        let carried = lock(&self.link_rate).bits_per_second().unwrap_or(agreed);
        crate::video::still_drain(agreed.min(carried))
    }
    /// The largest still picture the link takes now, in encoded bytes: the
    /// drain less what the gate's datagrams and parity add to it, so a still
    /// made to this size is not refused for its own framing.
    fn still_budget(&self) -> usize {
        let wire = self.still_drain();
        match self.datagram.load(std::sync::atomic::Ordering::Relaxed) {
            0 => wire
                .saturating_sub(crate::video::VIDEO_HEADER)
                .min(crate::MAX_VIDEO_FRAME),
            max => {
                let protection =
                    lock(&self.viewer_loss).protection(true, false, std::time::Instant::now());
                crate::video::still_payload(wire, max, PREFIX, protection)
            }
        }
    }
    /// Viewer: input queued to send, for diagnostics.
    pub(super) fn input_queued(&self) -> usize {
        self.input.depth()
    }
    pub(super) fn metadata(&self) -> DesktopInfo {
        let mut info = self.info.clone();
        info.quality = *lock(&self.quality);
        info
    }
    /// A picture arrived for the viewer, by datagrams or on the stream.
    /// Pictures its number says are missing or late are counted lost and
    /// reported to the host; see [`crate::video::ViewerPictures::picture`].
    fn receive_video(&self, shared: &Shared, body: crate::video::VideoBody) {
        if dropped(shared, DropPoint::Inbox) {
            return;
        }
        let order =
            lock(&self.pictures).picture(body.sequence, body.frame, &self.inbox, &self.keyframe);
        let lost = match order {
            crate::video::Order::Next => return,
            crate::video::Order::Gap(missing) => crate::video::Losses {
                gap: u64::from(missing),
                ..Default::default()
            },
            crate::video::Order::Stale => crate::video::Losses {
                stale: 1,
                ..Default::default()
            },
        };
        self.report.frames_lost(lost.total());
        self.counters.lost(lost);
    }
    /// Point-in-time pipeline facts for diagnostics: labels, stage
    /// durations, queue depths and drops. Never frame bytes. Durations
    /// older than [`PIPELINE_FRESH_SECS`](crate::diagnostics::PIPELINE_FRESH_SECS)
    /// read as unknown.
    pub(super) fn pipeline_summary(
        &self,
        service_id: u64,
        hosted: bool,
        now: std::time::Instant,
    ) -> crate::diagnostics::PipelineSummary {
        use crate::diagnostics::PIPELINE_FRESH_SECS;
        let fresh = |at: &std::time::Instant| {
            now.saturating_duration_since(*at)
                <= std::time::Duration::from_secs(PIPELINE_FRESH_SECS)
        };
        let producer = lock(&self.producer_duration)
            .as_ref()
            .filter(|(_, _, at)| fresh(at))
            .map(|(capture, encode, _)| (*capture, *encode));
        let decode = lock(&self.decode_duration)
            .as_ref()
            .filter(|(_, at)| fresh(at))
            .map(|(us, _)| *us);
        let out = self.video.telemetry();
        let inbound = self.inbox.telemetry();
        let counts = self.counters.read();
        let viewer = *lock(&self.viewer_loss);
        let decoder = *lock(&self.decoder);
        let encoder = *lock(&self.encoder);
        let reported = viewer.heard.filter(|at| fresh(at)).map(|_| viewer);
        let permille = |share: f64| (share * 1000.0).round() as u32;
        crate::diagnostics::PipelineSummary {
            service_id,
            hosted,
            codec: match self.info.codec {
                crate::protocol::CODEC_H264 => "h264",
                crate::protocol::CODEC_H264_HIGH => "h264-high",
                id => crate::VideoCodec::from_wire_id(id)
                    .map(|c| c.label())
                    .unwrap_or("unknown"),
            }
            .into(),
            width: self.info.monitor_width,
            height: self.info.monitor_height,
            control: self.control_state.borrow().0.label().into(),
            capture_us: producer.map(|timing| timing.0),
            encode_us: producer.map(|timing| timing.1),
            decode_us: decode,
            out_queued: out.queued,
            out_dropped: out.dropped,
            out_oldest_ms: out.oldest_ms,
            in_queued: inbound.queued,
            in_dropped: inbound.dropped,
            in_oldest_ms: inbound.oldest_ms,
            keyframe_pending: self.keyframe.pending(),
            lost_expired: counts.lost.expired,
            lost_evicted: counts.lost.evicted,
            lost_gap: counts.lost.gap,
            lost_stale: counts.lost.stale,
            keyframes_requested: counts.keyframes_requested,
            keyframes_sent: counts.keyframes_sent,
            keyframe_bytes: counts.keyframe_bytes,
            encoder_reopens: counts.encoder_reopens,
            send_dropped: counts.send_dropped,
            stills_sent: counts.stills_sent,
            still_bytes: counts.still_bytes,
            stills_dropped: counts.stills_dropped,
            viewer_fragment_loss_permille: reported.map(|loss| permille(loss.fragment_loss)),
            viewer_frame_loss_permille: reported.map(|loss| permille(loss.frame_loss)),
            selected_display: lock(&self.selected).first().copied(),
            fault: lock(&self.display_fault).map(str::to_string),
            conditions: lock(&self.conditions)
                .entries()
                .into_iter()
                .map(Into::into)
                .collect(),
            end: lock(&self.end).map(|end| end.label().into()),
            encoder: encoder.map(|encoder| encoder.name.into()),
            encoder_hardware: encoder.and_then(|encoder| encoder.hardware),
            encoder_input: encoder
                .and_then(|encoder| encoder.input)
                .map(|input| input.label().into()),
            decoder_hardware: decoder.software.map(|software| !software),
            decoder_recoveries: decoder.recoveries,
            stills_shown: decoder.stills_shown,
        }
    }
}

pub(super) async fn write(send: &mut SendStream, bytes: &[u8]) -> Result<(), Error> {
    if bytes.is_empty() || bytes.len() > MAX {
        return Err(Error::Malformed);
    }
    send.write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(Error::network_debug)?;
    send.write_all(bytes).await.map_err(Error::network_debug)?;
    Ok(())
}

pub(super) async fn read(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    let mut size = [0; 4];
    recv.read_exact(&mut size)
        .await
        .map_err(Error::network_debug)?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > MAX {
        return Err(Error::Malformed);
    }
    let mut bytes = vec![0; size];
    // Silence is permitted; once a length is sent the body has a deadline.
    tokio::time::timeout(TIMEOUT, recv.read_exact(&mut bytes))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    Ok(bytes)
}

pub(super) fn runtime(
    state: &State,
    id: u64,
    generation: u64,
    hosted: bool,
) -> Result<Arc<DesktopRuntime>, Error> {
    let records = if hosted {
        &state.hosted_services
    } else {
        &state.services
    };
    let record = records.get(&id).ok_or(Error::NoSession)?;
    if record.info.generation != generation {
        return Err(Error::StaleSession);
    }
    if record.info.phase != ServicePhase::Active || *record.stop.borrow() {
        return Err(Error::NoSession);
    }
    let parent = if hosted {
        state.incoming.get(&record.info.connection_id)
    } else {
        state.outgoing.get(&record.info.connection_id)
    };
    if !parent.is_some_and(|s| s.generation == generation && s.phase == SessionPhase::Active) {
        return Err(Error::NoSession);
    }
    record.desktop.clone().ok_or(Error::NoSession)
}

impl Core {
    /// Local host-directed handoff. Both owners must still be active; a stale
    /// caller cannot revoke a replacement's lease. Release completes before the
    /// target can inject input, and no remote viewer may force this operation.
    pub fn transfer_desktop_control(
        &self,
        from_id: u64,
        from_generation: u64,
        to_id: u64,
        to_generation: u64,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        let from = runtime(&state, from_id, from_generation, true)?;
        let to = runtime(&state, to_id, to_generation, true)?;
        if state.desktop_control != Some(from.stamp) {
            return Err(Error::Busy);
        }
        if !state.trust.service_granted(
            &parse_endpoint_id(&to.info.service.peer_id)?,
            ServiceKind::Desktop,
        ) || !state.trust.granted(
            &parse_endpoint_id(&to.info.service.peer_id)?,
            crate::session::Grant::Control,
        ) {
            return Err(Error::Unauthorized);
        }
        if from.stamp == to.stamp {
            return Ok(());
        }
        let epoch = to
            .control_state
            .borrow()
            .1
            .checked_add(1)
            .ok_or(Error::Busy)?;
        release_desktop_input(&self.shared, &from);
        state.desktop_control = Some(to.stamp);
        let old_epoch = from.control_state.borrow().1.saturating_add(1);
        from.control_state
            .send_replace((DesktopControlState::Viewing, old_epoch));
        // Publish the new controlling lease only after old native keys have
        // settled. Revalidate because revoke/close stays live during this wait.
        drop(state);
        wait_input_cleanup(&self.shared);
        let state = self.alive_state()?;
        runtime(&state, to_id, to_generation, true)?;
        if state.desktop_control != Some(to.stamp)
            || !state.trust.granted(
                &parse_endpoint_id(&to.info.service.peer_id)?,
                crate::session::Grant::Control,
            )
        {
            return Err(Error::StaleSession);
        }
        to.control_state
            .send_replace((DesktopControlState::Controlling, epoch));
        Ok(())
    }
    pub fn hosted_desktop_info(&self, id: u64, generation: u64) -> Result<DesktopInfo, Error> {
        let _engine = self.engine();
        Ok(runtime(&*self.alive_state()?, id, generation, true)?.metadata())
    }
    /// The largest still picture, in encoded bytes, this hosted owner's link
    /// takes now within [`crate::STILL_DRAIN`]: what the link carries,
    /// capped at the agreed bitrate. A larger one is not sent.
    pub fn desktop_still_budget(&self, id: u64, generation: u64) -> Result<usize, Error> {
        let _engine = self.engine();
        Ok(runtime(&*self.alive_state()?, id, generation, true)?.still_budget())
    }
    /// Measured successful encode/preparation and native capture read costs.
    /// Capture read may include waiting for a frame; it is diagnostic, not a
    /// CPU congestion signal. Scope is the exact hosted Desktop owner.
    pub fn report_desktop_producer_duration(
        &self,
        id: u64,
        generation: u64,
        capture_us: u64,
        encode_us: u64,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        if capture_us > 5_000_000 || encode_us > 5_000_000 {
            return Err(Error::InvalidSetting("producer duration is out of range"));
        }
        let desktop = runtime(&*self.alive_state()?, id, generation, true)?;
        let mut measured = lock(&desktop.producer_duration);
        let now = std::time::Instant::now();
        let average = measured
            .filter(|(_, _, at)| now.saturating_duration_since(*at) <= Duration::from_secs(3))
            .map_or((capture_us, encode_us), |(capture, encode, _)| {
                ((capture * 7 + capture_us) / 8, (encode * 7 + encode_us) / 8)
            });
        *measured = Some((average.0, average.1, now));
        Ok(())
    }
    /// The encoder the producer opened for this viewer, at open and at every
    /// reopen; the viewer learns whether it is hardware.
    pub fn report_desktop_encoder(
        &self,
        id: u64,
        generation: u64,
        encoder: crate::EncoderImplementation,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        *lock(&runtime(&*self.alive_state()?, id, generation, true)?.encoder) = Some(encoder);
        tracing::info!(
            target: crate::trace::STREAM,
            service = id,
            generation,
            encoder = ?encoder,
            "encoder opened"
        );
        Ok(())
    }
    /// The platform backend's view of whether the screen is on a desktop
    /// it cannot capture. Every viewer learns it with its next feedback.
    pub fn report_secure_desktop(&self, active: bool) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        self.shared
            .secure_desktop
            .store(active, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    /// The host opened its encoder again for this viewer's new settings.
    pub fn report_desktop_encoder_reopen(&self, id: u64, generation: u64) -> Result<(), Error> {
        let _engine = self.engine();
        runtime(&*self.alive_state()?, id, generation, true)?
            .counters
            .encoder_reopened();
        Ok(())
    }
    pub fn publish_desktop_video(
        &self,
        id: u64,
        generation: u64,
        mut frame: EncodedFrame,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        frame.check()?;
        let desktop = {
            let state = self.alive_state()?;
            runtime(&state, id, generation, true)?
        };
        if frame.role == crate::FrameRole::Still && !desktop.info.stills {
            return Err(Error::Unsupported("still pictures for this viewer"));
        }
        if !frame.belongs_to(desktop.info.codec, desktop.info.stills) {
            return Err(Error::InvalidSetting(
                "frame codec does not match Desktop owner",
            ));
        }
        // Backstop a zero revision from the live topology. The display id
        // is never invented here: only the producer knows which display an
        // unselected ("follows capture") target actually receives, and a
        // whole-desktop picture honestly names no single display.
        if frame.revision == 0 {
            frame.revision = lock(&self.shared.topology).revision;
        }
        if matches!(desktop.video.push(frame), VideoPush::KeyframeNeeded) {
            desktop.keyframe.request();
        }
        Ok(())
    }
    pub fn publish_desktop_cursor(
        &self,
        id: u64,
        generation: u64,
        cursor: CursorState,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let desktop = runtime(&state, id, generation, true)?;
        if !desktop.info.separate_cursor {
            return Err(Error::Unsupported("separate cursor"));
        }
        desktop.cursor.send_replace(Some(Arc::new(cursor)));
        Ok(())
    }
    pub fn take_desktop_keyframe_request(&self, id: u64, generation: u64) -> Result<bool, Error> {
        let _engine = self.engine();
        Ok(runtime(&*self.alive_state()?, id, generation, true)?
            .keyframe
            .take())
    }
}

/// Reconcile a durable grant change while holding the same state lock used
/// for input admission. Release held keys before another owner can acquire.
pub(super) fn reconcile_grants(
    shared: &Shared,
    state: &mut State,
    peer: &[u8; 32],
    grants: crate::Grants,
) {
    let incoming: Vec<u64> = state
        .incoming
        .values()
        .filter(|session| &session.peer_key == peer)
        .map(|session| session.id)
        .collect();
    let desktops: Vec<_> = state
        .hosted_services
        .values()
        .filter(|record| {
            incoming.contains(&record.info.connection_id)
                && record.info.kind == ServiceKind::Desktop
        })
        .filter_map(|record| record.desktop.clone())
        .collect();
    for desktop in desktops {
        if !grants.control || !grants.view {
            release_desktop_input(shared, &desktop);
            if state.desktop_control == Some(desktop.stamp) {
                state.desktop_control = None;
            }
            let epoch = desktop.control_state.borrow().1.saturating_add(1);
            desktop
                .control_state
                .send_replace((DesktopControlState::Viewing, epoch));
        }
        if !grants.view {
            if let Some(record) = state.hosted_services.get_mut(&desktop.info.service.id) {
                if record.change(
                    record.info.generation,
                    ServicePhase::Closed(CloseCause::Revoked),
                ) {
                    shared.events.publish(CoreEvent::HostedServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
        }
    }
}

impl RemoteDesktopService {
    pub(super) fn active_desktop(&self) -> Result<&Arc<DesktopRuntime>, Error> {
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        Ok(&self.desktop)
    }
    /// Returns the latest host-confirmed state, not an optimistic local grant.
    pub fn control_state(&self) -> Result<DesktopControlState, Error> {
        Ok(self.active_desktop()?.control_state.borrow().0)
    }
    /// Requests control without preempting another controller. Poll control_state
    /// for the host's decision. Contended requests are not queued for takeover.
    pub fn acquire_control(&self) -> Result<(), Error> {
        self.queue_control(ACQUIRE_CONTROL)
    }
    /// Relinquishes host input after releasing held keys and buttons. Viewing
    /// and all other services continue. A new acquisition is required afterward.
    pub fn yield_control(&self) -> Result<(), Error> {
        self.queue_control(YIELD_CONTROL)
    }
    fn queue_control(&self, command: u8) -> Result<(), Error> {
        let desktop = self.active_desktop()?;
        let mut commands = lock(&desktop.control_commands);
        if commands.len() >= 16 {
            return Err(Error::Busy);
        }
        desktop.release_input();
        commands.push_back((command, desktop.control_state.borrow().1));
        desktop.control_notify.notify_one();
        Ok(())
    }
    pub fn desktop_info(&self) -> Result<DesktopInfo, Error> {
        Ok(self.active_desktop()?.metadata())
    }
    /// Native decoder reports only successfully decoded pictures on this exact
    /// owner. Stale/closed owners cannot alter another session's feedback.
    pub fn report_decode_duration(&self, microseconds: u64) -> Result<(), Error> {
        if microseconds > 5_000_000 {
            return Err(Error::InvalidSetting("decode duration is out of range"));
        }
        let desktop = self.active_desktop()?;
        let mut measured = lock(&desktop.decode_duration);
        let now = std::time::Instant::now();
        let average = measured
            .filter(|(_, at)| now.saturating_duration_since(*at) <= Duration::from_secs(3))
            .map_or(microseconds, |(before, _)| (before * 7 + microseconds) / 8);
        *measured = Some((average, now));
        Ok(())
    }
    /// The window's decoder reports whether it decodes in software and how
    /// many times it restarted from a keyframe after an error, in total.
    /// A count that goes back is refused: it belongs to another decoder.
    pub fn report_decoder(
        &self,
        software: bool,
        recoveries: u64,
        stills_shown: u64,
    ) -> Result<(), Error> {
        let desktop = self.active_desktop()?;
        let mut decoder = lock(&desktop.decoder);
        if recoveries < decoder.recoveries || stills_shown < decoder.stills_shown {
            return Err(Error::InvalidSetting("decoder counts went back"));
        }
        *decoder = DecoderState {
            software: Some(software),
            recoveries,
            stills_shown,
        };
        Ok(())
    }
    pub fn media_status(&self) -> Result<crate::DesktopMediaStatus, Error> {
        let desktop = self.active_desktop()?;
        let mut status = lock(&desktop.adaptive).status;
        status.conditions = lock(&desktop.conditions).active();
        Ok(status)
    }
    /// Why the picture stopped, once it did; `None` while it lives, or when
    /// this computer closed it. Readable after the service closed.
    pub fn end(&self) -> Option<crate::DesktopEnd> {
        *lock(&self.desktop.end)
    }
    /// The conditions held now, with since when and their evidence.
    pub fn conditions(&self) -> Result<Vec<crate::ConditionEntry>, Error> {
        Ok(lock(&self.active_desktop()?.conditions).entries())
    }
    /// The one display this desktop stream will show and control.
    pub fn select_displays(&self, ids: &[u64]) -> Result<(), Error> {
        let ids = super::topology::normalize_selection(ids)?;
        self.active_desktop()?.input.push_displays(ids)
    }

    /// Displays the host has sent on this stream. Empty until the first snapshot.
    pub fn topology(&self) -> Result<Vec<crate::DisplayInfo>, Error> {
        Ok(lock(&self.active_desktop()?.topology).clone())
    }

    pub fn submit_input(&self, event: crate::InputEvent) -> Result<(), Error> {
        let desktop = self.active_desktop()?;
        let mut keyboard = lock(&desktop.keyboard);
        let event = if keyboard.swap {
            crate::input::swap_command_key(event)
        } else {
            event
        };
        let mut trial = keyboard.pointer.clone();
        if let Some(event) = trial.apply(event)? {
            desktop.input.push(event)?;
            if desktop.control_state.borrow().0 == DesktopControlState::Controlling {
                lock(&desktop.activity).input(event, std::time::Instant::now());
            }
            keyboard.held.observe(event);
        }
        keyboard.pointer = trial;
        Ok(())
    }
    pub fn pointer_settings(&self) -> Result<crate::PointerSettings, Error> {
        Ok(lock(&self.active_desktop()?.keyboard).pointer.settings)
    }
    pub fn set_pointer_settings(&self, settings: crate::PointerSettings) -> Result<(), Error> {
        let mut keyboard = lock(&self.active_desktop()?.keyboard);
        let buttons_held = keyboard.held.buttons_held();
        keyboard.pointer.set(settings, buttons_held)
    }
    pub fn submit_local_key(&self, key: crate::LocalKey<'_>) -> Result<(), Error> {
        self.active_desktop()?.plan_key(key)
    }
    pub fn keyboard_mode(&self) -> Result<crate::KeyboardMode, Error> {
        Ok(lock(&self.active_desktop()?.keyboard).mode)
    }
    pub fn set_keyboard_mode(&self, mode: crate::KeyboardMode) -> Result<(), Error> {
        let mut keyboard = lock(&self.active_desktop()?.keyboard);
        if !keyboard.idle() {
            return Err(Error::InvalidSetting(
                "release held input before changing keyboard mode",
            ));
        }
        keyboard.mode = mode;
        Ok(())
    }
    pub fn swap_control_and_command(&self) -> Result<bool, Error> {
        Ok(lock(&self.active_desktop()?.keyboard).swap)
    }
    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), Error> {
        let mut keyboard = lock(&self.active_desktop()?.keyboard);
        if !keyboard.idle() {
            return Err(Error::InvalidSetting(
                "release held input before changing Control/Command swap",
            ));
        }
        keyboard.swap = enabled;
        Ok(())
    }
    pub fn submit_text(&self, text: &str) -> Result<(), Error> {
        let desktop = self.active_desktop()?;
        let _keyboard = lock(&desktop.keyboard);
        desktop.input.push_text(text.to_owned())?;
        if !text.is_empty() && desktop.control_state.borrow().0 == DesktopControlState::Controlling
        {
            lock(&desktop.activity).touch(std::time::Instant::now());
        }
        Ok(())
    }
    pub fn release_input(&self) -> Result<(), Error> {
        // What the release discards is recorded as this engine's.
        let span = self.shared.upgrade().map(|shared| shared.span.clone());
        let _engine = span.as_ref().map(tracing::Span::enter);
        self.active_desktop()?.release_input();
        Ok(())
    }
    pub fn request_keyframe(&self) -> Result<(), Error> {
        self.active_desktop()?.keyframe.request();
        Ok(())
    }
    pub fn cursor(&self) -> Result<Option<Arc<CursorState>>, Error> {
        Ok(self.active_desktop()?.cursor.borrow().clone())
    }
    /// Latest host-confirmed recording consent, independent from viewing/input.
    pub fn recording_allowed(&self) -> Result<bool, Error> {
        Ok(self.active_desktop()?.recording_allowed.borrow().0)
    }
    /// Nonvisual bounded feed gated by recording consent. Keeps the viewer subscription.
    pub fn recording_video_tap(&self) -> Result<crate::RecordingVideoTap, Error> {
        let desktop = self.active_desktop()?;
        let _gate = lock(&desktop.recording_gate);
        if !desktop.recording_allowed.borrow().0 {
            return Err(Error::Unauthorized);
        }
        let tap = desktop.inbox.recording_tap()?;

        desktop.keyframe.request();
        Ok(tap)
    }
    pub fn subscribe_video(&self) -> Result<VideoSubscription, Error> {
        let desktop = self.active_desktop()?;
        let mut slot = lock(&desktop.subscriber);
        if slot.upgrade().is_some() {
            return Err(Error::Busy);
        }
        let alive = Arc::new(());
        *slot = Arc::downgrade(&alive);
        Ok(VideoSubscription::new(desktop.inbox.clone(), alive))
    }
}

pub(super) fn open(core: &Core, id: u64, generation: u64) -> Result<RemoteDesktopService, Error> {
    let mut lease = core.service_lease(id, generation)?;
    if lease.info().kind != ServiceKind::Desktop {
        return Err(Error::InvalidSetting("this is not a Desktop service"));
    }
    if !lock(&core.shared.video_caps).can_decode() {
        return Err(Error::Unsupported("video decoder"));
    }
    let (link, stamp, stop) = {
        let state = core.alive_state()?;
        let session = live_session(&state, false, lease.info().connection_id)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        let record = state.services.get(&id).ok_or(Error::NoSession)?;
        if record.desktop.is_some() {
            return Err(Error::Busy);
        }
        (
            session.link.clone().ok_or(Error::NoSession)?,
            session.wire_stamp,
            record.stop.clone(),
        )
    };
    let limits = core.desktop_quality_defaults()?;
    let offer = crate::session::dial_quality(limits);
    let decoder_caps = {
        let caps = core.video_codecs()?;
        caps.for_window(core.alive_state()?.window_decoders.as_deref())
    };
    let codec_preference = core.desktop_codec_default()?;
    let codec_offer = decoder_caps.preferred_offer(codec_preference)?;
    let window_decoders = core.alive_state()?.window_decoders.clone();
    let bt601 = crate::video_caps::VideoCodecCaps::bt601_mask(window_decoders.as_deref());
    // Reserve once per owner, including while Open awaits a reply.
    let reservation = DesktopRuntime::new(
        DesktopInfo {
            service: lease.info().clone(),
            monitor_width: 0,
            monitor_height: 0,
            codec: 1,
            quality: offer,
            separate_cursor: false,
            stills: false,
            matrix: crate::YuvMatrix::Bt709,
        },
        SessionStamp::new(0, 0),
        limits.max_bitrate_kbps,
    );
    {
        let mut state = core.alive_state()?;
        let record = state.services.get_mut(&id).ok_or(Error::NoSession)?;
        if record.info.generation != generation || *record.stop.borrow() {
            return Err(Error::StaleSession);
        }
        if record.desktop.is_some() {
            return Err(Error::Busy);
        }
        record.desktop = Some(reservation);
    }
    let cursor = core
        .shared
        .cursor_receiver
        .load(std::sync::atomic::Ordering::Relaxed);
    let stills = decoder_caps.chroma444;
    let shared = core.shared.clone();
    let owner_info = lease.info().clone();
    let (ready, result) = std::sync::mpsc::sync_channel(1);
    core.runtime.spawn(async move {
        let opened = lease
            .run(async {
                tokio::time::timeout(TIMEOUT, async {
                    let mut request = vec![TAG, VERSION];
                    request.extend_from_slice(&stamp.id.to_be_bytes());
                    request.extend_from_slice(&stamp.generation.to_be_bytes());
                    request.extend_from_slice(&id.to_be_bytes());
                    request.push(
                        if cursor { OPEN_CURSOR } else { 0 } | if stills { OPEN_STILLS } else { 0 },
                    );
                    request.extend_from_slice(
                        &ControlMessage::Hello {
                            version: crate::protocol::PROTOCOL_VERSION,
                            codec: codec_offer,
                            quality: offer,
                            decode: true,
                            separate_cursor: cursor,
                            bt601,
                            hostname: None,
                            os: crate::OperatingSystem::local(),
                        }
                        .encode(),
                    );
                    loop {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, &request).await?;
                        let response = read(&mut recv).await?;
                        if response.len() < 2 || response[0] != VERSION {
                            return Err(Error::IncompatibleVersion);
                        }
                        match response[1] {
                            0 => {}
                            4 => {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                continue;
                            }
                            6 => return Err(Error::Unauthorized),
                            3 => return Err(Error::StaleSession),
                            1 => return Err(Error::Unsupported("remote Desktop backend")),
                            _ => return Err(Error::IncompatibleVersion),
                        }
                        if response.len() < 20 {
                            return Err(Error::Malformed);
                        }
                        let control_status = DesktopControlState::decode(response[2])?;
                        let control_epoch = u64::from_be_bytes(response[3..11].try_into().unwrap());
                        let ControlMessage::Accept {
                            stamp: epoch,
                            monitor_width,
                            monitor_height,
                            codec,
                            quality,
                            ..
                        } = ControlMessage::decode(&response[20..])?
                        else {
                            return Err(Error::Malformed);
                        };
                        if epoch.id == 0
                            || epoch.generation != stamp.generation
                            || monitor_width == 0
                            || monitor_height == 0
                        {
                            return Err(Error::Malformed);
                        }
                        let flags = crate::protocol::SEPARATE_CURSOR | crate::protocol::STILLS;
                        // Stills the window cannot show were never asked for.
                        if codec & crate::protocol::STILLS != 0 && !stills {
                            return Err(Error::Malformed);
                        }
                        if codec_preference.is_some_and(|c| {
                            crate::VideoCodec::from_wire_id(codec & !flags) != Some(c)
                        }) {
                            return Err(Error::IncompatibleVersion);
                        }
                        let info = DesktopInfo {
                            service: owner_info.clone(),
                            monitor_width,
                            monitor_height,
                            codec: codec & !flags,
                            quality,
                            separate_cursor: codec & crate::protocol::SEPARATE_CURSOR != 0,
                            stills: codec & crate::protocol::STILLS != 0,
                            matrix: crate::VideoCodec::from_wire_id(codec & !flags)
                                .map_or(crate::YuvMatrix::Bt709, |agreed| {
                                    crate::YuvMatrix::of_codec(bt601, agreed)
                                }),
                        };
                        if !decoder_caps.supports_decode(
                            crate::VideoCodec::from_wire_id(info.codec)
                                .ok_or(Error::IncompatibleVersion)?,
                        ) {
                            return Err(Error::IncompatibleVersion);
                        }
                        let desktop = DesktopRuntime::new(info, epoch, limits.max_bitrate_kbps);
                        desktop
                            .control_state
                            .send_replace((control_status, control_epoch));
                        if response[11] > 1 {
                            return Err(Error::Malformed);
                        }
                        desktop.receive_recording_state(
                            response[11] == 1,
                            u64::from_be_bytes(response[12..20].try_into().unwrap()),
                        )?;
                        {
                            let mut keyboard = lock(&desktop.keyboard);
                            keyboard.pointer = crate::input::PointerPlan::with_settings(*lock(
                                &shared.pointer_defaults,
                            ));
                            keyboard.mode = crate::KeyboardMode::from_u8(
                                shared
                                    .keyboard_mode
                                    .load(std::sync::atomic::Ordering::Relaxed),
                            );
                            keyboard.swap = shared
                                .swap_control_command
                                .load(std::sync::atomic::Ordering::Relaxed);
                        }
                        return Ok((send, recv, desktop));
                    }
                })
                .await
                .map_err(|_| Error::Timeout)?
            })
            .await;
        match opened {
            Ok((mut send, mut recv, desktop)) => {
                let installed = {
                    let mut state = lock(&shared.state);
                    if let Some(record) = state.services.get_mut(&id).filter(|r| {
                        r.info.generation == generation
                            && r.info.phase == ServicePhase::Active
                            && !*r.stop.borrow()
                    }) {
                        record.desktop = Some(desktop.clone());
                        true
                    } else {
                        false
                    }
                };
                if installed {
                    crate::ignored!(
                        crate::trace::STREAM,
                        "the opener stopped waiting for the desktop",
                        ready.send(Ok(desktop.clone()))
                    );
                    ensure_datagram_router(
                        &shared,
                        link.clone(),
                        owner_info.connection_id,
                        generation,
                    );
                    // The desktop's end is recorded below, whichever way it went.
                    let _ended = lease
                        .run(async {
                            let mut clipboard = tokio::task::JoinSet::new();
                            clipboard.spawn(super::desktop_clipboard::client(
                                shared.clone(),
                                link.clone(),
                                desktop.clone(),
                                desktop.stamp,
                            ));
                            let control = async {
                                let sent = tokio::select! {
                                    result = client_send(&shared, &link, &desktop, &mut send) => result,
                                    result = client_receive(&shared, &desktop, &mut recv) => return result,
                                };
                                // A host ending the stream stops reading it before
                                // its `END` arrives here, so sending can fail first:
                                // the `END` is still worth a moment's wait.
                                match tokio::time::timeout(
                                    END_PATIENCE,
                                    client_receive(&shared, &desktop, &mut recv),
                                )
                                .await
                                {
                                    Ok(received) => {
                                        crate::ignored!(
                                            crate::trace::STREAM,
                                            "the stream is ending; an END read here said why",
                                            received
                                        );
                                    }
                                    Err(_) => tracing::debug!(
                                        target: crate::trace::STREAM,
                                        service = id,
                                        "the host said nothing more after sending stopped"
                                    ),
                                }
                                sent
                            };
                            tokio::pin!(control);
                            loop {
                                tokio::select! {
                                    result = &mut control => break result,
                                    _ = clipboard.join_next(), if !clipboard.is_empty() => {},
                                }
                            }
                        })
                        .await;
                    // An `END` already said why. Otherwise the symptoms do:
                    // a lost connection, or a stream finished in silence.
                    if !lease.is_cancelled() {
                        desktop.ended(
                            &shared,
                            false,
                            if link.close_reason().is_none() {
                                crate::DesktopEnd::PeerClosed
                            } else if desktop.relayed.load(std::sync::atomic::Ordering::Acquire) {
                                crate::DesktopEnd::RelayUnreachable
                            } else {
                                crate::DesktopEnd::LinkLost
                            },
                        );
                    }
                } else {
                    crate::ignored!(
                        crate::trace::STREAM,
                        "the opener stopped waiting for the desktop",
                        ready.send(Err(Error::NoSession))
                    );
                }
                crate::ignored!(
                    crate::trace::STREAM,
                    "a closing stream that cannot finish is closed by the link",
                    send.finish()
                );
                desktop.close();
                super::desktop_service::finish_controller(
                    &shared,
                    id,
                    generation,
                    link.close_reason().is_none(),
                    if lease.is_cancelled() {
                        CloseCause::OwnerClose
                    } else if desktop.host_ended_connection() {
                        CloseCause::ParentLoss
                    } else {
                        CloseCause::PeerClose
                    },
                );
            }
            Err(error) => {
                let definite = matches!(
                    error,
                    Error::Unsupported(_) | Error::Unauthorized | Error::IncompatibleVersion
                );
                if definite {
                    let mut state = lock(&shared.state);
                    if let Some(record) = state
                        .services
                        .get_mut(&id)
                        .filter(|r| r.info.generation == generation)
                    {
                        record.desktop = None;
                    }
                } else {
                    super::desktop_service::finish_controller(
                        &shared,
                        id,
                        generation,
                        link.close_reason().is_none(),
                        if lease.is_cancelled() {
                            CloseCause::OwnerClose
                        } else {
                            CloseCause::PeerClose
                        },
                    );
                }
                crate::ignored!(
                    crate::trace::STREAM,
                    "the opener stopped waiting for the desktop",
                    ready.send(Err(error))
                );
            }
        }
    });
    let desktop = result
        .recv_timeout(TIMEOUT + Duration::from_secs(1))
        .map_err(|_| Error::Timeout)??;
    Ok(RemoteDesktopService {
        lease: core.service_lease(id, generation)?,
        stop,
        shared: Arc::downgrade(&core.shared),
        desktop,
    })
}

/// A `COMMAND` message for `message`, sent under lease `epoch`.
fn command_bytes(epoch: u64, message: &ControlMessage) -> Vec<u8> {
    let mut bytes = vec![COMMAND];
    bytes.extend_from_slice(&epoch.to_be_bytes());
    bytes.extend_from_slice(&message.encode());
    bytes
}

#[cfg(any(test, feature = "test-probes"))]
impl Core {
    /// Sends hosted Desktop `id`'s pictures on its stream rather than as
    /// datagrams, for tests of the stream path a link without datagrams
    /// takes.
    pub fn force_desktop_stream(&self, id: u64, generation: u64) -> Result<(), Error> {
        let _engine = self.engine();
        runtime(&*self.alive_state()?, id, generation, true)?
            .force_stream
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

mod client;
pub(crate) use client::*;
mod datagrams;
pub(crate) use datagrams::*;
mod input;
pub(crate) use input::*;
mod send;
pub(crate) use send::*;
mod serve;
pub(crate) use serve::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn command(epoch: u64, event: crate::InputEvent) -> HostCommand {
        let mut bytes = vec![COMMAND];
        bytes.extend_from_slice(&epoch.to_be_bytes());
        bytes.extend_from_slice(
            &ControlMessage::Input {
                stamp: SessionStamp {
                    id: 7,
                    generation: 3,
                },
                event,
            }
            .encode(),
        );
        HostCommand::new(bytes)
    }

    fn kinds(batch: &[HostCommand]) -> Vec<(u64, &'static str)> {
        batch
            .iter()
            .map(|item| match &item.command {
                Some(Ok(ControlMessage::Input { event, .. })) => (
                    u64::from_be_bytes(item.body[1..9].try_into().unwrap()),
                    event.kind(),
                ),
                other => panic!("not an input: {other:?}"),
            })
            .collect()
    }

    /// A burst of pointer positions is applied as the newest one; the keys,
    /// buttons and wheel between them keep their place.
    #[test]
    fn a_burst_of_pointer_moves_is_applied_as_the_newest() {
        use crate::InputEvent;
        let at = |x| InputEvent::pointer(x, 0, 1, 1);
        let mut batch = vec![
            command(1, at(1)),
            command(1, at(2)),
            command(1, InputEvent::button_down(crate::PointerButton::Left)),
            command(1, at(3)),
            command(1, at(4)),
            command(1, at(5)),
            command(1, InputEvent::wheel(0, 1)),
            command(1, at(6)),
            command(2, at(7)),
        ];
        assert!(batch.iter().all(HostCommand::native));
        coalesce_pointer_moves(&mut batch);
        assert_eq!(
            kinds(&batch),
            vec![
                (1, "pointer"),
                (1, "button down"),
                (1, "pointer"),
                (1, "wheel"),
                (1, "pointer"),
                (2, "pointer"),
            ],
            "the last move of each run stays; a move under another lease is its own"
        );
        let moved: Vec<_> = batch
            .iter()
            .filter_map(|item| match &item.command {
                Some(Ok(ControlMessage::Input {
                    event: InputEvent::Pointer { x, .. },
                    ..
                })) => Some(*x),
                _ => None,
            })
            .collect();
        assert_eq!(moved, vec![2, 5, 6, 7]);
    }

    #[test]
    fn a_frame_waits_behind_at_most_three_frames_of_video() {
        // 8 Mb/s at 50 fps is 20 kB a frame: the floor of 64 KiB applies.
        assert_eq!(backlog_budget(8_000, 50), 64 * 1024);
        // 24 Mb/s at 30 fps is 100 kB a frame.
        assert_eq!(backlog_budget(24_000, 30), 300_000);
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        // Nothing waiting: any frame that fits goes, a large keyframe too.
        assert!(admits_frame(buffer, 500_000, 64 * 1024));
        // Something waiting: only while the backlog stays within budget.
        assert!(admits_frame(buffer - 40_000, 20_000, 64 * 1024));
        assert!(!admits_frame(buffer - 60_000, 20_000, 64 * 1024));
        // Never a frame the transport would make room for by dropping.
        assert!(!admits_frame(10_000, 20_000, usize::MAX));
    }

    #[test]
    fn a_dropped_keyframe_asks_for_one_more_only_once_the_link_could_take_it() {
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        let budget = 64 * 1024;
        let backlog = buffer - 40_000;
        let mut gate = SendGate::default();
        gate.sent(true, 83_000);
        assert!(!gate.should_ask(buffer, budget), "nothing is owed");
        // A predicted frame is dropped behind a backlog: the keyframe owed
        // for it is not asked for while the backlog would drop it too.
        gate.dropped(false, 5_000);
        assert!(gate.skips(false));
        assert!(!gate.skips(true));
        assert!(!gate.should_ask(backlog, budget));
        assert!(gate.owed());
        // Drained: asked for once, however many frames pass before it comes.
        assert!(gate.should_ask(buffer, budget));
        assert!(!gate.owed());
        assert!(!gate.should_ask(buffer, budget));
        // The keyframe met a new backlog and was dropped: one more is owed,
        // again only once the link has room for it.
        gate.dropped(true, 83_000);
        assert!(!gate.should_ask(backlog, budget));
        assert!(gate.should_ask(buffer, budget));
        // It went out: predicted frames flow again.
        gate.sent(true, 83_000);
        assert!(!gate.skips(false));
        assert!(!gate.owed());
    }

    #[test]
    fn the_frames_behind_a_large_keyframe_go_while_it_drains() {
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        let budget = 64 * 1024;
        let mut gate = SendGate::default();
        assert!(gate.admits(buffer, 92_000, budget));
        gate.sent(true, 92_000);
        // Most of the keyframe still waits: a predicted frame goes behind it.
        assert!(gate.admits(buffer - 72_000, 8_000, budget));
        gate.sent(false, 8_000);
        // Drained back within the budget: the budget alone applies again.
        assert!(gate.admits(buffer - 30_000, 8_000, budget));
        assert!(!gate.admits(buffer - 60_000, 8_000, budget));
        // Nor does a predicted frame's backlog earn the allowance.
        let mut gate = SendGate::default();
        gate.sent(false, 8_000);
        assert!(!gate.admits(buffer - 72_000, 8_000, budget));
    }

    /// A full-quality refresh larger than the budget, sent without being
    /// asked for, does not make the gate wait for an empty queue before
    /// asking for the small recovery keyframe it owes next.
    #[test]
    fn a_large_unasked_keyframe_does_not_hold_back_the_next_ask() {
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        let budget = 64 * 1024;
        let mut gate = SendGate::default();
        // A recovery keyframe answers an ask.
        gate.dropped(false, 5_000);
        assert!(gate.should_ask(buffer, budget));
        gate.sent(true, 30_000);
        // A full-quality refresh goes out unasked.
        gate.sent(true, 90_000);
        // A drop behind a backlog: the recovery keyframe owed would fit.
        gate.dropped(false, 5_000);
        assert!(gate.should_ask(buffer - 30_000, budget));
        // A recovery keyframe that grew is learnt when it answers.
        gate.sent(true, 60_000);
        gate.dropped(false, 5_000);
        assert!(!gate.should_ask(buffer - 30_000, budget));
    }

    /// The frames behind a still wait for it rather than drop, and it
    /// neither pays nor teaches the keyframe owed.
    #[test]
    fn the_frames_behind_a_still_go_while_it_drains_and_nothing_owed_changes() {
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        let budget = 64 * 1024;
        let mut gate = SendGate::default();
        gate.sent(true, 20_000);
        gate.still_sent(200_000);
        assert!(gate.admits(buffer - 190_000, 8_000, budget));
        assert!(!gate.owed() && !gate.skips(false));
        gate.dropped(false, 5_000);
        gate.still_sent(200_000);
        assert!(gate.skips(false), "a still is not the keyframe owed");
        assert!(gate.should_ask(buffer - 40_000, budget), "nor its size");
    }

    #[test]
    fn a_small_keyframe_is_asked_for_behind_a_backlog_it_fits_within() {
        let buffer = crate::core::host::DATAGRAM_SEND_BUFFER;
        let mut gate = SendGate::default();
        gate.sent(true, 20_000);
        gate.dropped(false, 5_000);
        assert!(gate.should_ask(buffer - 40_000, 64 * 1024));
    }

    fn desktop() -> Arc<DesktopRuntime> {
        DesktopRuntime::new(
            DesktopInfo {
                service: ServiceInfo {
                    id: 1,
                    connection_id: 2,
                    generation: 3,
                    peer_id: "test".into(),
                    kind: ServiceKind::Desktop,
                    phase: ServicePhase::Active,
                },
                monitor_width: 1920,
                monitor_height: 1080,
                codec: crate::CODEC_H264,
                quality: crate::session::controller_offer(),
                separate_cursor: false,
                stills: false,
                matrix: crate::YuvMatrix::Bt709,
            },
            SessionStamp::new(1, 3),
            crate::protocol::MAX_BITRATE_KBPS,
        )
    }
    #[test]
    fn recording_epochs_close_old_feeds_even_if_watch_updates_coalesce() {
        let host = desktop();
        let client = desktop();
        host.set_recording_allowed(true);
        let (allowed, epoch) = *host.recording_allowed.borrow();
        client.receive_recording_state(allowed, epoch).unwrap();
        let old = client.inbox.recording_tap().unwrap();
        host.set_recording_allowed(false);
        host.set_recording_allowed(true);
        let (allowed, epoch) = *host.recording_allowed.borrow();
        client.receive_recording_state(allowed, epoch).unwrap();
        assert!(old.is_closed());
        assert!(client.recording_allowed.borrow().0);
        assert!(client.inbox.recording_tap().is_ok());
        assert!(matches!(
            client.receive_recording_state(true, epoch - 1),
            Err(Error::StaleSession)
        ));
        assert!(matches!(
            client.receive_recording_state(false, epoch),
            Err(Error::StaleSession)
        ));
    }
    #[test]
    fn delayed_input_release_and_yield_cannot_touch_a_reacquired_grant() {
        use crate::test_support::{loopback_core, pair_for_control, RecordingDesktop};
        let host = loopback_core();
        let peer = loopback_core();
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(RecordingDesktop {
            events: applied.clone(),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let parent = crate::test_support::wait_active(&peer, parent.id);
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let handle = peer
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let hosted = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == ServiceKind::Desktop)
            .unwrap();
        let desktop = runtime(
            &lock(&host.shared.state),
            hosted.id,
            hosted.generation,
            true,
        )
        .unwrap();
        let action = |tag, epoch: u64| {
            let mut bytes = vec![tag];
            bytes.extend_from_slice(&epoch.to_be_bytes());
            bytes
        };
        let input = |epoch, event| {
            let mut bytes = action(COMMAND, epoch);
            bytes.extend_from_slice(
                &ControlMessage::Input {
                    stamp: desktop.stamp,
                    event,
                }
                .encode(),
            );
            bytes
        };
        let down = crate::InputEvent::key_down(crate::input::usage::A);
        let up = crate::InputEvent::key_up(crate::input::usage::A);
        process_host_command(&host.shared, &desktop, &input(1, down)).unwrap();
        process_host_command(&host.shared, &desktop, &action(YIELD_CONTROL, 1)).unwrap();
        process_host_command(&host.shared, &desktop, &action(ACQUIRE_CONTROL, 1)).unwrap();
        process_host_command(&host.shared, &desktop, &input(2, down)).unwrap();
        process_host_command(
            &host.shared,
            &desktop,
            &input(1, crate::InputEvent::key_down(crate::input::usage::B)),
        )
        .unwrap();
        process_host_command(&host.shared, &desktop, &action(RELEASE, 1)).unwrap();
        process_host_command(&host.shared, &desktop, &action(YIELD_CONTROL, 1)).unwrap();
        assert_eq!(
            *desktop.control_state.borrow(),
            (DesktopControlState::Controlling, 2)
        );
        assert_eq!(*applied.lock().unwrap(), vec![down, up, down]);
        assert!(matches!(
            process_host_command(&host.shared, &desktop, &[]),
            Err(Error::Malformed)
        ));
        process_host_command(&host.shared, &desktop, &input(2, up)).unwrap();
        assert_eq!(*applied.lock().unwrap(), vec![down, up, down, up]);
        process_host_command(&host.shared, &desktop, &action(YIELD_CONTROL, 2)).unwrap();
        process_host_command(&host.shared, &desktop, &action(ACQUIRE_CONTROL, 1)).unwrap();
        assert_eq!(
            *desktop.control_state.borrow(),
            (DesktopControlState::Viewing, 2)
        );
        process_host_command(&host.shared, &desktop, &action(ACQUIRE_CONTROL, 2)).unwrap();
        assert_eq!(
            *desktop.control_state.borrow(),
            (DesktopControlState::Controlling, 3)
        );
        handle.close();
    }

    #[test]
    fn ownership_update_discards_pending_input_but_preserves_view_commands() {
        let desktop = desktop();
        desktop.receive_control_state(DesktopControlState::Controlling, 1);
        desktop
            .input
            .push_quality(crate::session::controller_offer())
            .unwrap();
        desktop
            .plan_key(crate::LocalKey::new(
                crate::input::usage::LEFT_CONTROL,
                true,
                "",
            ))
            .unwrap();
        let (_, old_batch, epoch) = desktop.take_input_epoch();
        assert_eq!(epoch, 1);
        assert_eq!(old_batch.len(), 2);
        desktop
            .plan_key(crate::LocalKey::new(crate::input::usage::A, true, "a"))
            .unwrap();
        desktop.input.push_displays(vec![1]).unwrap();
        desktop.release_input();
        desktop.receive_control_state(DesktopControlState::Controlling, 2);
        let (release, items, epoch) = desktop.take_input_epoch();
        assert!(!release);
        assert_eq!(epoch, 2);
        assert_eq!(items, vec![Outbound::Displays(vec![1])]);
        assert!(lock(&desktop.keyboard).idle());
    }

    #[test]
    fn failed_input_batch_does_not_commit_keyboard_plan() {
        let desktop = desktop();
        for _ in 0..crate::input::INPUT_QUEUE {
            desktop
                .input
                .push(crate::InputEvent::key_down(crate::input::usage::A))
                .unwrap();
        }
        let key = crate::LocalKey::new(crate::input::usage::LEFT_CONTROL, true, "");
        assert!(matches!(desktop.plan_key(key), Err(Error::Busy)));
        assert!(lock(&desktop.keyboard).idle());
        desktop.take_input();
        desktop.plan_key(key).unwrap();
        assert!(!lock(&desktop.keyboard).idle());
        let (_, items) = desktop.take_input();
        assert_eq!(
            items,
            vec![Outbound::Event(crate::InputEvent::key_down(
                crate::input::usage::LEFT_CONTROL
            ))]
        );
    }
    #[test]
    fn release_barrier_orders_new_keys_and_preserves_quality_and_faults() {
        let desktop = desktop();
        let quality = crate::session::controller_offer();
        desktop.input.push_quality(quality).unwrap();
        desktop
            .plan_key(crate::LocalKey::new(
                crate::input::usage::LEFT_CONTROL,
                true,
                "",
            ))
            .unwrap();
        desktop.release_input();
        assert!(lock(&desktop.keyboard).idle());
        desktop
            .plan_key(crate::LocalKey::new(crate::input::usage::B, true, ""))
            .unwrap();
        let (release, items) = desktop.take_input();
        assert!(release);
        assert_eq!(
            items,
            vec![
                Outbound::Quality(quality),
                Outbound::Event(crate::InputEvent::key_down(crate::input::usage::B))
            ]
        );
        assert!(!desktop.take_input().0);
        desktop.close();
        desktop.release_input();
        assert!(desktop
            .input
            .push(crate::InputEvent::key_down(crate::input::usage::A))
            .is_err());
    }
    #[test]
    fn desktop_epoch_rejects_old_and_truncated_datagrams() {
        let current = SessionStamp {
            id: 7,
            generation: 3,
        };
        let mut bytes = epoch(current).to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(split_datagram(&bytes), Some((current, &[1, 2, 3][..])));
        for end in 0..=PREFIX {
            assert!(split_datagram(&bytes[..end]).is_none());
        }
        bytes[0] ^= 1;
        assert!(split_datagram(&bytes).is_none());
        let mut zeroed = epoch(SessionStamp { id: 0, ..current }).to_vec();
        zeroed.extend_from_slice(&[1, 2, 3]);
        assert!(split_datagram(&zeroed).is_none());
    }
}

#[cfg(test)]
mod v8_native_input_lock_probe {
    use super::*;
    struct ParkedInput {
        applied: Arc<Mutex<Vec<crate::InputEvent>>>,
        once: std::sync::atomic::AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl crate::HostDesktop for ParkedInput {
        fn apply(&mut self, event: crate::InputEvent) -> Result<(), Error> {
            if self.once.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                lock(&self.proceed)
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            lock(&self.applied).push(event);
            Ok(())
        }
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
    }
    #[test]
    fn slow_desktop_input_must_not_hold_core_state() {
        parked_input(false);
    }
    #[test]
    fn slow_stream_input_revokes_and_releases_after_async_close() {
        parked_input(true);
    }
    fn parked_input(stream: bool) {
        use crate::test_support::{loopback_core, pair_for_control};
        let host = loopback_core();
        let peer = loopback_core();
        let (entered, ready) = std::sync::mpsc::sync_channel(1);
        let (proceed, wait) = std::sync::mpsc::channel();
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(ParkedInput {
            applied: applied.clone(),
            once: std::sync::atomic::AtomicBool::new(true),
            entered,
            proceed: Mutex::new(wait),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let parent = crate::test_support::wait_active(&peer, parent.id);
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let _handle = peer
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let hosted = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == ServiceKind::Desktop)
            .unwrap();
        let desktop = runtime(
            &lock(&host.shared.state),
            hosted.id,
            hosted.generation,
            true,
        )
        .unwrap();
        let mut bytes = vec![COMMAND];
        bytes.extend_from_slice(&1u64.to_be_bytes());
        bytes.extend_from_slice(
            &ControlMessage::Input {
                stamp: desktop.stamp,
                event: crate::InputEvent::key_down(crate::input::usage::A),
            }
            .encode(),
        );
        let shared = host.shared.clone();
        let worker = if stream {
            crate::test_support::wait_until(
                "the desktop owner to control",
                Duration::from_secs(5),
                || _handle.control_state().unwrap() == DesktopControlState::Controlling,
            );
            _handle
                .submit_input(crate::InputEvent::key_down(crate::input::usage::A))
                .unwrap();
            None
        } else {
            Some(std::thread::spawn(move || {
                process_host_command(&shared, &desktop, &bytes)
            }))
        };
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        // Network bookkeeping can briefly own state. A parked native call
        // must leave it acquirable before native work is allowed to continue.
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        let state_available = loop {
            if host.shared.state.try_lock().is_ok() {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::yield_now();
        };
        // Revocation must settle while the native adapter remains parked.
        host.set_desktop_sharing(crate::Grants::default()).unwrap();
        proceed.send(()).unwrap();
        if let Some(worker) = worker {
            worker.join().unwrap().unwrap();
        }
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while host
            .shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(
            *lock(&applied),
            vec![
                crate::InputEvent::key_down(crate::input::usage::A),
                crate::InputEvent::key_up(crate::input::usage::A)
            ]
        );
        host.shutdown();
        peer.shutdown();
        assert!(state_available,"slow native input holds global Core state, preventing local stop/revocation/sibling admission");
    }
}

#[cfg(test)]
mod v9_pointer_wait_probe {
    use super::*;
    pub(super) static WAITING: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    #[test]
    fn pointer_wait_must_recheck_topology_before_native_apply() {
        for change in 0..3 {
            queued_pointer(change);
        }
    }
    fn queued_pointer(change: u8) {
        use crate::test_support::{loopback_core, pair_for_control, RecordingDesktop};
        use std::sync::atomic::Ordering;
        let host = loopback_core();
        let peer = loopback_core();
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(RecordingDesktop {
            events: applied.clone(),
        }))
        .unwrap();
        let display = crate::DisplayInfo {
            id: 1,
            origin_x: 0,
            origin_y: 0,
            width: 640,
            height: 480,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 1,
        };
        host.set_topology(1, &[display]).unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let parent = crate::test_support::wait_active(&peer, parent.id);
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let _handle = peer
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let hosted = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.kind == ServiceKind::Desktop)
            .unwrap();
        let desktop = runtime(
            &lock(&host.shared.state),
            hosted.id,
            hosted.generation,
            true,
        )
        .unwrap();
        *lock(&desktop.selected) = vec![1];
        let selection = desktop.clone();
        let mut bytes = vec![COMMAND];
        bytes.extend_from_slice(&1u64.to_be_bytes());
        bytes.extend_from_slice(
            &ControlMessage::Input {
                stamp: desktop.stamp,
                event: crate::InputEvent::Pointer {
                    x: 10,
                    y: 10,
                    display: 1,
                    revision: 1,
                },
            }
            .encode(),
        );
        WAITING.store(false, Ordering::Release);
        let adapter = lock(&host.shared.desktop);
        let shared = host.shared.clone();
        let worker = std::thread::Builder::new()
            .name("v9-pointer-probe".into())
            .spawn(move || process_host_command(&shared, &desktop, &bytes))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !WAITING.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        // Observation hook fires only after original pointer admission and
        // immediately before the existing adapter mutex wait.
        match change {
            0 => host
                .set_topology(
                    2,
                    &[crate::DisplayInfo {
                        id: 2,
                        revision: 2,
                        ..display
                    }],
                )
                .unwrap(),
            1 => host
                .set_topology(
                    2,
                    &[crate::DisplayInfo {
                        width: 320,
                        height: 240,
                        revision: 2,
                        ..display
                    }],
                )
                .unwrap(),
            _ => {
                let _state = lock(&host.shared.state);
                *lock(&selection.selected) = Vec::new();
            }
        }
        drop(adapter);
        worker.join().unwrap().unwrap();
        let events = applied.lock().unwrap().clone();
        host.shutdown();
        peer.shutdown();
        assert!(
            events.is_empty(),
            "revision1 pointer applied after topology2 publication: {events:?}"
        );
    }
}

#[cfg(test)]
mod v10_cleanup_churn_probe {
    use super::*;
    struct CleanupDesktop(Arc<Mutex<Vec<(std::thread::ThreadId, crate::InputEvent)>>>);
    impl crate::HostDesktop for CleanupDesktop {
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
        fn apply(&mut self, event: crate::InputEvent) -> Result<(), Error> {
            lock(&self.0).push((std::thread::current().id(), event));
            Ok(())
        }
    }
    #[test]
    fn empty_desktop_churn_must_bound_retired_cleanup() {
        use crate::test_support::{loopback_core, pair_for_control, RecordingDesktop};
        use std::sync::atomic::Ordering;
        let host = loopback_core();
        let peer = loopback_core();
        host.set_desktop(Box::new(RecordingDesktop {
            events: Arc::new(Mutex::new(Vec::new())),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let parent = crate::test_support::wait_active(&peer, parent.id);
        // Models a native operation parked while holding its existing adapter
        // mutex. Every churned owner is empty: no input is sent by the peer.
        let adapter = lock(&host.shared.desktop);
        let mut peak = 0;
        for index in 0..32 {
            let owner = peer
                .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
                .unwrap();
            let handle = peer
                .open_desktop_service(owner.id, owner.generation)
                .unwrap();
            peer.close_controller_service(owner.id, owner.generation)
                .unwrap();
            drop(handle);
            peer.forget_controller_service(owner.id).unwrap();
            crate::test_support::wait_until(
                &format!("cleanup to retire owner {index}"),
                Duration::from_secs(5),
                || {
                    host.hosted_services()
                        .unwrap()
                        .iter()
                        .all(|service| service.kind != ServiceKind::Desktop)
                },
            );
            peak = peak.max(host.shared.input_cleanup.load(Ordering::Acquire));
        }
        drop(adapter);
        crate::test_support::wait_until("input_cleanup to drain", Duration::from_secs(5), || {
            host.shared.input_cleanup.load(Ordering::Acquire) == 0
        });
        host.shutdown();
        peer.shutdown();
        assert_eq!(peak, 0, "empty owners must not queue native cleanup");
    }
    #[test]
    fn nonempty_retired_owners_keep_admission_slots() {
        use crate::test_support::{loopback_core, pair_for_control};
        use std::sync::atomic::Ordering;
        let host = loopback_core();
        let peer = loopback_core();
        let calls = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(CleanupDesktop(calls.clone())))
            .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let parent = crate::test_support::wait_active(&peer, parent.id);
        // Models a native operation parked while holding its existing adapter
        // mutex. Seed the applied held-set to isolate retirement accounting.
        let adapter = lock(&host.shared.desktop);
        let mut peak = 0;
        for index in 0..16 {
            let owner = peer
                .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
                .unwrap();
            let handle = peer
                .open_desktop_service(owner.id, owner.generation)
                .unwrap();
            let hosted = host
                .hosted_services()
                .unwrap()
                .into_iter()
                .find(|s| s.kind == ServiceKind::Desktop)
                .unwrap();
            let desktop = runtime(
                &lock(&host.shared.state),
                hosted.id,
                hosted.generation,
                true,
            )
            .unwrap();
            desktop
                .host_input
                .hold(crate::InputEvent::key_down(crate::input::usage::A));
            drop(desktop);
            peer.close_controller_service(owner.id, owner.generation)
                .unwrap();
            drop(handle);
            peer.forget_controller_service(owner.id).unwrap();
            crate::test_support::wait_until(
                &format!("cleanup to retire owner {index}"),
                Duration::from_secs(5),
                || {
                    host.hosted_services()
                        .unwrap()
                        .iter()
                        .all(|service| service.kind != ServiceKind::Desktop)
                },
            );
            peak = peak.max(host.shared.input_cleanup.load(Ordering::Acquire));
        }
        assert_eq!(peak, 16);
        assert_eq!(host.shared.desktop_owners.load(Ordering::Acquire), 16);
        let rejected = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        assert!(peer
            .open_desktop_service(rejected.id, rejected.generation)
            .is_err());
        drop(adapter);
        crate::test_support::wait_until("input_cleanup to drain", Duration::from_secs(5), || {
            host.shared.input_cleanup.load(Ordering::Acquire) == 0
        });
        crate::test_support::wait_until("desktop_owners to drain", Duration::from_secs(5), || {
            host.shared.desktop_owners.load(Ordering::Acquire) == 0
        });
        let calls = lock(&calls);
        assert_eq!(calls.len(), 16);
        assert!(calls.iter().all(|(worker, event)| *worker == calls[0].0
            && *event == crate::InputEvent::key_up(crate::input::usage::A)));
        drop(calls);
        let next = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let handle = peer.open_desktop_service(next.id, next.generation).unwrap();
        peer.close_controller_service(next.id, next.generation)
            .unwrap();
        drop(handle);
        host.shutdown();
        peer.shutdown();
        assert_eq!(peak, 16, "retired work must stay inside admission capacity");
    }
}
