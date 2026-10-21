//! Desktop instances on a neutral parent. Every instance owns its stream,
//! queues and datagram epoch; session APIs cannot publish into this backend.
use super::*;
use crate::input::{HeldInput, Outbound};
use crate::protocol::{ControlMessage, SessionStamp};
use crate::video::{EncodedFrame, KeyframeFlag, VideoPush, VideoSubscription};
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
const COMMAND: u8 = 3;
const RELEASE: u8 = 4;
const TOPOLOGY: u8 = 5;
const ACQUIRE_CONTROL: u8 = 6;
const YIELD_CONTROL: u8 = 7;
const CONTROL_STATE: u8 = 8;
const MEDIA_FEEDBACK: u8 = 9;
const RECORDING_STATE: u8 = 10;

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
const PREFIX: usize = 20;
const MAX: usize = crate::cursor::MAX_BODY + 64;
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopInfo {
    pub service: ServiceInfo,
    pub monitor_width: u16,
    pub monitor_height: u16,
    pub codec: u8,
    pub quality: Quality,
    pub separate_cursor: bool,
}

pub(super) struct DesktopRuntime {
    recording_allowed: tokio::sync::watch::Sender<(bool, u64)>,
    recording_gate: Mutex<()>,
    pub(super) activity: Mutex<super::activity::Activity>,
    pub(super) info: DesktopInfo,
    stamp: SessionStamp,
    quality: Mutex<Quality>,
    adaptive: Mutex<crate::adaptation::Adaptive>,
    decode_duration: Mutex<Option<(u64, std::time::Instant)>>,
    producer_duration: Mutex<Option<(u64, u64, std::time::Instant)>>,
    host_feedback: Mutex<Option<(crate::adaptation::HostFeedback, std::time::Instant)>>,
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
    /// The viewer's outstanding keyframe request, so losses while one is
    /// coming do not ask again.
    keyframe_requests: Mutex<crate::video::KeyframeRequests>,
    control_state: tokio::sync::watch::Sender<(DesktopControlState, u64)>,
    control_commands: Mutex<std::collections::VecDeque<(u8, u64)>>,
    control_notify: tokio::sync::Notify,
    /// Actual host-applied input, separate from controller queue state.
    host_held: Arc<Mutex<HeldInput>>,
    releasing: Arc<std::sync::atomic::AtomicBool>,
    in_flight: AtomicUsize,
    cleanup_slot: Mutex<Option<Arc<super::input_cleanup::AdmissionSlot>>>,
    retired: std::sync::atomic::AtomicBool,
    /// The one display this owner shows. Empty means none, never a substitute.
    pub(super) selected: Mutex<Vec<u64>>,
    pub(super) display_fault: Mutex<Option<&'static str>>,
    /// Topology the host sent to this controller. Empty until the first snapshot.
    topology: Mutex<Vec<crate::DisplayInfo>>,
    pub(super) keyboard: Mutex<DesktopKeyboard>,
    pub(super) clipboard: super::desktop_clipboard::ClipboardState,
    #[cfg(test)]
    force_stream: std::sync::atomic::AtomicBool,
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
    fn new(info: DesktopInfo, stamp: SessionStamp) -> Arc<Self> {
        Arc::new(Self {
            recording_allowed: tokio::sync::watch::channel((false, 0)).0,
            recording_gate: Mutex::new(()),
            activity: Mutex::new(super::activity::Activity::new(std::time::Instant::now())),
            quality: Mutex::new(info.quality),
            adaptive: Mutex::new(crate::adaptation::Adaptive::default()),
            decode_duration: Mutex::new(None),
            producer_duration: Mutex::new(None),
            host_feedback: Mutex::new(None),
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
            keyframe_requests: Mutex::new(crate::video::KeyframeRequests::default()),
            control_state: tokio::sync::watch::channel((DesktopControlState::Viewing, 0)).0,
            control_commands: Mutex::new(std::collections::VecDeque::new()),
            control_notify: tokio::sync::Notify::new(),
            host_held: Arc::new(Mutex::new(HeldInput::default())),
            releasing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            in_flight: AtomicUsize::new(0),
            cleanup_slot: Mutex::new(None),
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
            #[cfg(test)]
            force_stream: std::sync::atomic::AtomicBool::new(false),
        })
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
        self.input.clear_pending_input();
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
            self.input.clear_pending_input();
            self.release.take();
            keyboard.plan.clear();
            keyboard.pointer.reset();
            keyboard.held = HeldInput::default();
            self.control_state.send_replace((status, epoch));
        }
    }
    pub(super) fn metadata(&self) -> DesktopInfo {
        let mut info = self.info.clone();
        info.quality = *lock(&self.quality);
        info
    }
    /// A frame arrived for the viewer. A keyframe answers the outstanding
    /// request; a full queue asks for one.
    fn receive_video(&self, frame: EncodedFrame) {
        let keyframe = frame.keyframe;
        if matches!(self.inbox.push(frame), VideoPush::KeyframeNeeded) {
            self.keyframe.request();
        }
        if keyframe {
            lock(&self.keyframe_requests).keyframe_arrived();
        }
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
            viewer_fragment_loss_permille: reported.map(|loss| permille(loss.fragment_loss)),
            viewer_frame_loss_permille: reported.map(|loss| permille(loss.frame_loss)),
            selected_display: lock(&self.selected).first().copied(),
            fault: lock(&self.display_fault).map(str::to_string),
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
fn epoch(stamp: SessionStamp) -> [u8; PREFIX] {
    let mut bytes = [0; PREFIX];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4..12].copy_from_slice(&stamp.id.to_be_bytes());
    bytes[12..20].copy_from_slice(&stamp.generation.to_be_bytes());
    bytes
}

/// Splits the 20-byte owner prefix (magic + stamp) from a datagram body.
/// Unprefixed traffic belongs to the legacy session path, never an owner.
fn split_datagram(bytes: &[u8]) -> Option<(SessionStamp, &[u8])> {
    if bytes.len() <= PREFIX || bytes[..4] != *MAGIC {
        return None;
    }
    let stamp = SessionStamp::new(
        u64::from_be_bytes(bytes[4..12].try_into().ok()?),
        u64::from_be_bytes(bytes[12..20].try_into().ok()?),
    );
    if stamp.id == 0 {
        return None;
    }
    Some((stamp, &bytes[PREFIX..]))
}

/// Starts the connection's sole desktop-datagram reader, once per
/// (connection, generation). Per-owner readers would split fragments
/// between siblings; the router reassembles per stamp and feeds each
/// owner's inbox. The task exits when the connection closes or the
/// session ends, and releases its key so a later generation can route.
fn ensure_datagram_router(
    shared: &Arc<Shared>,
    link: Connection,
    connection_id: u64,
    generation: u64,
) {
    if !lock(&shared.desktop_routers).insert((connection_id, generation)) {
        return;
    }
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        route_desktop_datagrams(&shared, &link, connection_id, generation).await;
        lock(&shared.desktop_routers).remove(&(connection_id, generation));
    });
}

/// How often a viewer tells its host what it lost.
const REPORT_EVERY: Duration = Duration::from_millis(250);

/// How often the router checks its connection is still the live one; between
/// checks a closed owner is noticed by its own retired flag.
const ROUTER_LIVENESS: Duration = Duration::from_millis(100);
/// Datagrams taken from the connection at once.
const ROUTER_BATCH: usize = 32;

async fn route_desktop_datagrams(
    shared: &Arc<Shared>,
    link: &Connection,
    connection_id: u64,
    generation: u64,
) {
    let mut fragments: std::collections::HashMap<(u64, u64), crate::video::Reassembler> =
        std::collections::HashMap::new();
    // Each owner's runtime, found once under the state lock and kept until
    // it retires, so a datagram takes no shared lock.
    let mut owners: std::collections::HashMap<(u64, u64), Arc<DesktopRuntime>> =
        std::collections::HashMap::new();
    let mut checked: Option<std::time::Instant> = None;
    let mut batch = vec![bytes::Bytes::new(); ROUTER_BATCH];
    loop {
        if checked.is_none_or(|at| at.elapsed() >= ROUTER_LIVENESS) {
            let state = lock(&shared.state);
            let live = !state.shut_down
                && state.outgoing.get(&connection_id).is_some_and(|session| {
                    session.generation == generation && !finished(&session.phase)
                });
            if !live {
                return;
            }
            checked = Some(std::time::Instant::now());
        }
        let count = match link.read_many_datagrams(&mut batch).await {
            Ok(count) => count,
            Err(_) => return,
        };
        for bytes in &batch[..count] {
            let Some((stamp, fragment)) = split_datagram(bytes) else {
                continue;
            };
            let key = (stamp.id, stamp.generation);
            let cached = owners
                .get(&key)
                .filter(|desktop| !desktop.retired.load(std::sync::atomic::Ordering::Acquire))
                .cloned();
            let target = cached.or_else(|| {
                let state = lock(&shared.state);
                state.services.values().find_map(|record| {
                    record
                        .desktop
                        .as_ref()
                        .filter(|desktop| {
                            record.info.connection_id == connection_id
                                && record.info.generation == generation
                                && !record.info.phase.finished()
                                && desktop.stamp == stamp
                        })
                        .map(Arc::clone)
                })
            });
            let Some(desktop) = target else {
                owners.remove(&key);
                continue;
            };
            // One reassembly context and cached owner per stamp; stale
            // contexts expire inside the reassembler, and neither map can
            // outgrow the owner bound plus one transient entry for a closing
            // sibling.
            if fragments.len() > MAX_DESKTOP_OWNERS_PER_CONNECTION + 1 {
                fragments.clear();
            }
            if owners.len() > MAX_DESKTOP_OWNERS_PER_CONNECTION + 1 {
                owners.clear();
            }
            owners.insert(key, Arc::clone(&desktop));
            let reassembly = fragments
                .entry(key)
                .or_insert_with(crate::video::Reassembler::new);
            let body = reassembly.push(fragment);
            desktop.report.fragments(reassembly.take_fragments());
            if body.is_some() {
                desktop.report.frame_completed();
            }
            // A lost frame: what follows waits for a keyframe, which is asked
            // for now rather than at the next periodic one.
            let losses = reassembly.take_loss();
            if losses.any() {
                desktop.report.frames_lost(losses.total());
                desktop.counters.lost(losses);
                desktop.inbox.lost();
                desktop.keyframe.request();
            }
            if let Some(body) = body {
                if let Ok(frame) = crate::video::decode_video_body(&body) {
                    if Some(frame.codec) != crate::VideoCodec::from_wire_id(desktop.info.codec) {
                        continue;
                    }
                    desktop.receive_video(frame);
                }
            }
        }
    }
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
        state.lock_watch.note_controlling();
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
        Ok(runtime(&*self.alive_state()?, id, generation, true)?.metadata())
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
    /// The host opened its encoder again for this viewer's new settings.
    pub fn report_desktop_encoder_reopen(&self, id: u64, generation: u64) -> Result<(), Error> {
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
        frame.check()?;
        let desktop = {
            let state = self.alive_state()?;
            runtime(&state, id, generation, true)?
        };
        if Some(frame.codec) != crate::VideoCodec::from_wire_id(desktop.info.codec) {
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
        let state = self.alive_state()?;
        let desktop = runtime(&state, id, generation, true)?;
        if !desktop.info.separate_cursor {
            return Err(Error::Unsupported("separate cursor"));
        }
        desktop.cursor.send_replace(Some(Arc::new(cursor)));
        Ok(())
    }
    pub fn take_desktop_keyframe_request(&self, id: u64, generation: u64) -> Result<bool, Error> {
        Ok(runtime(&*self.alive_state()?, id, generation, true)?
            .keyframe
            .take())
    }
}

/// Reconcile a durable grant change while holding the same state lock used
/// for input application. Release held keys before another owner can acquire.
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
    if !grants.view || !grants.control {
        for id in incoming {
            if let Some(session) = state.incoming.get_mut(&id) {
                // Fence an in-flight native operation even when it has not
                // yet recorded its key-down in the held set.
                session
                    .input_epoch
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                if lock(&session.host_held).release_events().is_empty() {
                    if !grants.view && !session.services_only && !session.files_only {
                        session.desktop_stop.send_replace(true);
                    }
                    continue;
                }
                // The worker owns this snapshot; late native work settles its
                // own newly held input under the same adapter guard.
                let slot = session.incoming_slot.clone();
                let held = std::mem::take(&mut *lock(&session.host_held));
                let adapter = shared.desktop.clone();
                let pending = shared.input_cleanup.clone();
                let events = shared.events.clone();
                pending.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                let reservation = super::input_cleanup::Reservation {
                    pending,
                    releasing: None,
                };
                shared.cleanup_executor.submit(move || {
                    let _slot = slot;
                    let mut adapter = lock(&adapter);
                    if let Some(adapter) = adapter.as_mut() {
                        for event in held.release_events() {
                            if adapter.apply(event).is_err() {
                                events.publish(CoreEvent::InputFailed {
                                    id,
                                    message: "a held key or button could not be released",
                                });
                            }
                        }
                    }
                    drop(reservation);
                });
                if !session.services_only && !session.files_only {
                    if !grants.view {
                        session.desktop_stop.send_replace(true);
                    }
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
        self.desktop.as_ref().ok_or(Error::Unsupported(
            "scoped operations on a legacy Desktop bridge",
        ))
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
    pub fn media_status(&self) -> Result<crate::DesktopMediaStatus, Error> {
        Ok(lock(&self.active_desktop()?.adaptive).status)
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
        return Err(Error::Unsupported("H.264 decoder"));
    }
    let (link, stamp, stop) = {
        let state = core.alive_state()?;
        let session = live_session(&state, false, lease.info().connection_id)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        if !session.services_only {
            return Err(Error::Unsupported("neutral Desktop transport"));
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
    let offer = core.desktop_quality_defaults()?;
    let decoder_caps = {
        let caps = core.video_codecs()?;
        caps.for_window(core.alive_state()?.window_decoders.as_deref())
    };
    let codec_preference = core.desktop_codec_default()?;
    let codec_offer = decoder_caps.preferred_offer(codec_preference)?;
    // Reserve once per owner, including while Open awaits a reply.
    let reservation = DesktopRuntime::new(
        DesktopInfo {
            service: lease.info().clone(),
            monitor_width: 0,
            monitor_height: 0,
            codec: 1,
            quality: offer,
            separate_cursor: false,
        },
        SessionStamp::new(0, 0),
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
                    request.push(u8::from(cursor));
                    request.extend_from_slice(
                        &ControlMessage::Hello {
                            version: crate::protocol::PROTOCOL_VERSION,
                            codec: codec_offer,
                            quality: offer,
                            decode: true,
                            files_only: false,
                            services_only: false,
                            separate_cursor: cursor,
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
                        if codec_preference.is_some_and(|c| {
                            crate::VideoCodec::from_wire_id(
                                codec & !crate::protocol::SEPARATE_CURSOR,
                            ) != Some(c)
                        }) {
                            return Err(Error::IncompatibleVersion);
                        }
                        let info = DesktopInfo {
                            service: owner_info.clone(),
                            monitor_width,
                            monitor_height,
                            codec: codec & !crate::protocol::SEPARATE_CURSOR,
                            quality,
                            separate_cursor: codec & crate::protocol::SEPARATE_CURSOR != 0,
                        };
                        if !decoder_caps.supports_decode(
                            crate::VideoCodec::from_wire_id(info.codec)
                                .ok_or(Error::IncompatibleVersion)?,
                        ) {
                            return Err(Error::IncompatibleVersion);
                        }
                        let desktop = DesktopRuntime::new(info, epoch);
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
                    let _ = ready.send(Ok(desktop.clone()));
                    ensure_datagram_router(
                        &shared,
                        link.clone(),
                        owner_info.connection_id,
                        generation,
                    );
                    let _ = lease
                        .run(async {
                            let mut clipboard = tokio::task::JoinSet::new();
                            clipboard.spawn(super::desktop_clipboard::client(
                                shared.clone(),
                                link.clone(),
                                desktop.clone(),
                                desktop.stamp,
                            ));
                            let control = async {
                                tokio::select! {
                                    result = client_send(&link, &desktop, &mut send) => result,
                                    result = client_receive(&shared, &desktop, &mut recv) => result,
                                }
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
                } else {
                    let _ = ready.send(Err(Error::NoSession));
                }
                let _ = send.finish();
                desktop.close();
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
                let _ = ready.send(Err(error));
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
        desktop: Some(desktop),
    })
}

async fn client_send(
    link: &Connection,
    desktop: &DesktopRuntime,
    send: &mut SendStream,
) -> Result<(), Error> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reports = tokio::time::interval(REPORT_EVERY);
    reports.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut sampled_at = std::time::Instant::now();
    let mut last_path = None;
    loop {
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
        if desktop.keyframe.take() {
            lock(&desktop.keyframe_requests).want();
        }
        let path_rtt = link
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .map_or(Duration::ZERO, |path| path.rtt());
        let frame_period = frame_period(desktop);
        let ask = lock(&desktop.keyframe_requests).should_send(
            std::time::Instant::now(),
            path_rtt,
            frame_period,
        );
        let retry = lock(&desktop.keyframe_requests).retry_at(path_rtt, frame_period);
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
            _ = &mut notified => {}, _ = desktop.keyframe.notified() => {}, _ = desktop.release.notified() => {}, _ = desktop.control_notify.notified() => {},
            _ = reports.tick() => {
                let report = desktop.report.take();
                if !report.is_empty() {
                    let mut bytes = vec![COMMAND];
                    bytes.extend_from_slice(&lease_epoch.to_be_bytes());
                    bytes.extend_from_slice(
                        &ControlMessage::VideoReport { stamp: desktop.stamp, report }.encode(),
                    );
                    write(send, &bytes).await?;
                }
            },
            // An unanswered keyframe request is asked again when its patience runs out.
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
                    let next = lock(&desktop.adaptive).step(current,
                        (desktop.info.monitor_width, desktop.info.monitor_height), elapsed, feedback);
                    if let Some(next) = next {
                        // Controller-side step: monitor clamp only. The host
                        // applies its encoder caps when the update arrives.
                        let next = crate::session::agree_quality(
                            &next,
                            (desktop.info.monitor_width, desktop.info.monitor_height),
                            &crate::video_caps::VideoCodecCaps::default(),
                        );
                        desktop.input.push_quality(next)?; *lock(&desktop.quality) = next;
                    }
                }
            }
        }
    }
}
async fn client_receive(
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
                MEDIA_FEEDBACK => {
                    let feedback = crate::adaptation::HostFeedback::decode(&body[1..])?;
                    *lock(&desktop.host_feedback) = Some((feedback, std::time::Instant::now()));
                }
                VIDEO => {
                    let frame = crate::video::decode_video_body(&body[1..])?;
                    if Some(frame.codec) != crate::VideoCodec::from_wire_id(desktop.info.codec) {
                        return Err(Error::Malformed);
                    }
                    desktop.receive_video(frame);
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

struct HostedDesktopGuard {
    shared: Arc<Shared>,
    id: u64,
    generation: u64,
    /// Revocation removes the runtime from the service record. Keep the actual
    /// applied input alive until teardown has released it.
    desktop: Arc<DesktopRuntime>,
}
impl Drop for HostedDesktopGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        release_desktop_input(&self.shared, &self.desktop);
        if state.desktop_control == Some(self.desktop.stamp) {
            state.desktop_control = None;
        }
        if state
            .hosted_services
            .get(&self.id)
            .is_some_and(|r| r.info.generation == self.generation)
        {
            if let Some(mut record) = state.hosted_services.remove(&self.id) {
                // Backstop for a serve loop that ended without an explicit
                // close: explicit paths set the cause first and `change`
                // keeps the first one. A live parent means the desktop
                // backend or loop itself died.
                let cause = if state.shut_down {
                    CloseCause::Shutdown
                } else {
                    match state.incoming.get(&record.info.connection_id) {
                        Some(session) if session.phase == SessionPhase::Active => {
                            CloseCause::DeviceLoss
                        }
                        _ => CloseCause::ParentLoss,
                    }
                };
                record.change(self.generation, ServicePhase::Closed(cause));
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
    }
}
struct StreamCount<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for StreamCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn release_desktop_input(shared: &Shared, desktop: &DesktopRuntime) {
    use std::sync::atomic::Ordering;
    // Called under state, which also admits native work. Do not block on a
    // held-set lock that a late native release might currently own.
    if desktop.in_flight.load(Ordering::Acquire) == 0
        && desktop
            .host_held
            .try_lock()
            .is_ok_and(|held| held.release_events().is_empty())
    {
        return;
    }
    if desktop.releasing.swap(true, Ordering::AcqRel) {
        return;
    }
    shared.input_cleanup.fetch_add(1, Ordering::AcqRel);
    let reservation = super::input_cleanup::Reservation {
        pending: shared.input_cleanup.clone(),
        releasing: Some(desktop.releasing.clone()),
    };
    let slot = lock(&desktop.cleanup_slot).clone();
    let adapter = shared.desktop.clone();
    let held = desktop.host_held.clone();
    let events = shared.events.clone();
    let id = desktop.info.service.connection_id;
    shared.cleanup_executor.submit(move || {
        let mut adapter = lock(&adapter);
        let releases = std::mem::take(&mut *lock(&held));
        if let Some(adapter) = adapter.as_mut() {
            for event in releases.release_events() {
                if adapter.apply(event).is_err() {
                    events.publish(CoreEvent::InputFailed {
                        id,
                        message: "a held key or button could not be released",
                    });
                }
            }
        }
        // Both reservation and owner slot settle before the adapter is yielded.
        drop(reservation);
        drop(slot);
    });
}

pub(super) async fn serve(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    mut send: SendStream,
    mut recv: RecvStream,
    body: &[u8],
    mut parent_stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    if body.len() < 27 || body[0] != VERSION || body[25] > 1 {
        write(&mut send, &[VERSION, 5]).await?;
        return Ok(());
    }
    let parent = SessionStamp::new(
        u64::from_be_bytes(body[1..9].try_into().unwrap()),
        u64::from_be_bytes(body[9..17].try_into().unwrap()),
    );
    let remote_id = u64::from_be_bytes(body[17..25].try_into().unwrap());
    let ControlMessage::Hello {
        version,
        codec,
        quality,
        decode,
        files_only: false,
        services_only: false,
        ..
    } = ControlMessage::decode(&body[26..])?
    else {
        return Err(Error::Malformed);
    };
    if remote_id == 0 || version != crate::protocol::PROTOCOL_VERSION {
        write(&mut send, &[VERSION, 5]).await?;
        return Ok(());
    }
    // One connection admits a bounded set of simultaneous monitor owners;
    // each carries its own runtime while the lease records are counted at
    // Open. Other connections admit concurrently; `desktop_streams` keeps
    // adapter swaps out until every stream, including teardown, has finished.
    let outcome = {
        let mut state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if !session.services_only || session.peer_key != peer || session.wire_stamp != parent {
            Err(3)
        } else if !state.trust.service_granted(&peer, ServiceKind::Desktop) {
            Err(6)
        } else if shared
            .desktop_owners
            .load(std::sync::atomic::Ordering::Acquire)
            >= 16
            || state.hosted_services.len() >= 16
            || state
                .hosted_services
                .values()
                .filter(|r| {
                    r.info.connection_id == connection_id
                        && r.info.kind == ServiceKind::Desktop
                        && r.desktop.is_some()
                })
                .count()
                >= MAX_DESKTOP_OWNERS_PER_CONNECTION
        {
            Err(4)
        } else if !decode || !lock(&shared.video_caps).can_encode() {
            Err(1)
        } else {
            let monitor = monitor_size(&shared);
            let caps = lock(&shared.video_caps).clone();
            if monitor.0 == 0 || monitor.1 == 0 {
                Err(1)
            } else if let Some(agreed) = crate::video_caps::negotiate(codec, &caps) {
                let codec = agreed.codec;
                let peer_id = session.peer_id.clone();
                let id = state.next_service_id;
                state.next_service_id = id.checked_add(1).ok_or(Error::Busy)?;
                let info = ServiceInfo {
                    id,
                    connection_id,
                    generation: parent.generation,
                    peer_id,
                    kind: ServiceKind::Desktop,
                    phase: ServicePhase::Active,
                };
                let separate = body[25] == 1
                    && lock(&shared.capture)
                        .as_ref()
                        .is_some_and(|c| c.separate_cursor());
                let desktop = DesktopRuntime::new(
                    DesktopInfo {
                        service: info.clone(),
                        monitor_width: monitor.0,
                        monitor_height: monitor.1,
                        codec,
                        quality: crate::session::agree_quality(&quality, monitor, &caps),
                        separate_cursor: separate,
                    },
                    SessionStamp::new(id, parent.generation),
                );
                shared
                    .desktop_owners
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                *lock(&desktop.cleanup_slot) = Some(Arc::new(super::input_cleanup::AdmissionSlot(
                    shared.desktop_owners.clone(),
                )));
                desktop.set_recording_allowed(
                    state.trust.get(&peer).is_some() && state.trust.sharing.recording,
                );
                let stop = tokio::sync::watch::channel(false).0;
                let receiver = stop.subscribe();
                // Opening requests control once; a busy host admits a viewer.
                if !state.trust.granted(&peer, crate::session::Grant::Control) {
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Viewing, 0));
                } else if state.input_holder().is_none() {
                    state.desktop_control = Some(desktop.stamp);
                    state.lock_watch.note_controlling();
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Controlling, 1));
                } else {
                    desktop
                        .control_state
                        .send_replace((DesktopControlState::Contended, 0));
                }
                state.hosted_services.insert(
                    id,
                    super::services::ServiceRecord {
                        info: info.clone(),
                        remote_id: Some(remote_id),
                        stream_open: false,
                        stop,
                        desktop: Some(desktop.clone()),
                    },
                );
                shared
                    .events
                    .publish(CoreEvent::HostedServiceChanged { service: info });
                Ok((desktop, receiver))
            } else {
                Err(1)
            }
        }
    };
    let (desktop, mut service_stop) = match outcome {
        Ok(value) => value,
        Err(status) => {
            write(&mut send, &[VERSION, status]).await?;
            return Ok(());
        }
    };
    shared
        .desktop_streams
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let _streams = StreamCount(&shared.desktop_streams);
    let _guard = HostedDesktopGuard {
        shared: shared.clone(),
        id: desktop.info.service.id,
        generation: parent.generation,
        desktop: desktop.clone(),
    };
    let control = *desktop.control_state.borrow();
    let mut response = vec![VERSION, 0, control.0.encode()];
    response.extend_from_slice(&control.1.to_be_bytes());
    let recording = *desktop.recording_allowed.borrow();
    response.push(u8::from(recording.0));
    response.extend_from_slice(&recording.1.to_be_bytes());
    response.extend_from_slice(
        &ControlMessage::Accept {
            stamp: desktop.stamp,
            monitor_width: desktop.info.monitor_width,
            monitor_height: desktop.info.monitor_height,
            codec: desktop.info.codec
                | if desktop.info.separate_cursor {
                    crate::protocol::SEPARATE_CURSOR
                } else {
                    0
                },
            quality: desktop.info.quality,
            hostname: None,
            os: crate::OperatingSystem::local(),
        }
        .encode(),
    );
    write(&mut send, &response).await?;
    if !*parent_stop.borrow() && !*service_stop.borrow() {
        tokio::select! {
            biased;
            _ = parent_stop.changed() => {}, _ = service_stop.changed() => {},
            _ = host_receive(&shared, &desktop, &mut recv) => {},
            _ = host_send(&shared, &desktop, &mut send) => {},
        }
    }
    desktop.close();
    let _ = send.finish();
    Ok(())
}

async fn host_receive(
    shared: &Arc<Shared>,
    desktop: &Arc<DesktopRuntime>,
    recv: &mut RecvStream,
) -> Result<(), Error> {
    loop {
        let body = read(recv).await?;
        let native = match body.first() {
            Some(&RELEASE) | Some(&ACQUIRE_CONTROL) | Some(&YIELD_CONTROL) => true,
            Some(&COMMAND) if body.len() >= 9 => matches!(
                ControlMessage::decode(&body[9..]),
                Ok(ControlMessage::Input { .. }
                    | ControlMessage::Text { .. }
                    | ControlMessage::Chord { .. })
            ),
            _ => false,
        };
        if native {
            let shared = shared.clone();
            let desktop = desktop.clone();
            tokio::task::spawn_blocking(move || process_host_command(&shared, &desktop, &body))
                .await
                .map_err(|_| Error::NoSession)??;
        } else {
            process_host_command(shared, desktop, &body)?;
        }
    }
}

fn wait_input_cleanup(shared: &Shared) {
    while shared
        .input_cleanup
        .load(std::sync::atomic::Ordering::Acquire)
        != 0
    {
        drop(lock(&shared.desktop));
        std::thread::yield_now();
    }
}

/// Wait for the adapter and old cleanup on a blocking worker. Never wait for
/// Core state while holding the adapter: state transactions may inspect held
/// input and must remain responsive. Recheck exact owner, permission and epoch at native admission.
struct NativeInputGuard<'a> {
    adapter: std::sync::MutexGuard<'a, Option<Box<dyn crate::HostDesktop>>>,
    in_flight: &'a AtomicUsize,
}
impl std::ops::Deref for NativeInputGuard<'_> {
    type Target = Option<Box<dyn crate::HostDesktop>>;
    fn deref(&self) -> &Self::Target {
        &self.adapter
    }
}
impl std::ops::DerefMut for NativeInputGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.adapter
    }
}
impl Drop for NativeInputGuard<'_> {
    fn drop(&mut self) {
        self.in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

fn native_input_adapter<'a>(
    shared: &'a Shared,
    desktop: &'a DesktopRuntime,
    epoch: u64,
    pointer: Option<crate::InputEvent>,
) -> Result<Option<NativeInputGuard<'a>>, Error> {
    loop {
        #[cfg(test)]
        if std::thread::current().name() == Some("v9-pointer-probe") {
            v9_pointer_wait_probe::WAITING.store(true, std::sync::atomic::Ordering::Release);
        }
        let adapter = lock(&shared.desktop);
        let state = match shared.state.try_lock() {
            Ok(state) => state,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                drop(adapter);
                drop(lock(&shared.state));
                continue;
            }
        };
        runtime(
            &state,
            desktop.info.service.id,
            desktop.info.service.generation,
            true,
        )?;
        let peer = parse_endpoint_id(&desktop.info.service.peer_id)?;
        if !state.trust.service_granted(&peer, ServiceKind::Desktop)
            || !state.trust.granted(&peer, crate::session::Grant::Control)
            || state.desktop_control != Some(desktop.stamp)
            || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch)
            || desktop.retired.load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(None);
        }
        if shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        {
            if let Some(event) = pointer {
                let selected = lock(&desktop.selected);
                if let Err(error) = super::topology::accept_pointer(shared, event, &selected) {
                    super::keys::note_input_failure(
                        shared,
                        desktop.info.service.connection_id,
                        &error,
                    );
                    return Ok(None);
                }
            }
            desktop
                .in_flight
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            drop(state);
            return Ok(Some(NativeInputGuard {
                adapter,
                in_flight: &desktop.in_flight,
            }));
        }
        drop(state);
        drop(adapter);
        std::thread::yield_now();
    }
}

fn release_late_input(
    shared: &Shared,
    desktop: &DesktopRuntime,
    epoch: u64,
    adapter: &mut dyn crate::HostDesktop,
) -> bool {
    let stale = desktop.retired.load(std::sync::atomic::Ordering::Acquire)
        || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch);
    if stale {
        let mut held = lock(&desktop.host_held);
        for event in held.release_events() {
            if let Err(error) = adapter.apply(event) {
                super::keys::note_input_failure(shared, desktop.info.service.connection_id, &error);
            }
        }
        *held = HeldInput::default();
    }
    stale
}

/// Validate under Core state, take the adapter without waiting, then release
/// Core state before native work. Late input is released under the same adapter
/// guard when retirement or a lease transition overtakes the native call.
fn process_host_command(
    shared: &Shared,
    desktop: &DesktopRuntime,
    body: &[u8],
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    runtime(
        &state,
        desktop.info.service.id,
        desktop.info.service.generation,
        true,
    )?;
    if !state.trust.service_granted(
        &parse_endpoint_id(&desktop.info.service.peer_id)?,
        ServiceKind::Desktop,
    ) {
        return Err(Error::Unauthorized);
    }
    if body.len() < 9 {
        return Err(Error::Malformed);
    }
    let epoch = u64::from_be_bytes(body[1..9].try_into().unwrap());
    let current_epoch = desktop.control_state.borrow().1;
    let input_allowed = state.trust.granted(
        &parse_endpoint_id(&desktop.info.service.peer_id)?,
        crate::session::Grant::Control,
    );
    if body.len() == 9 && body[0] == RELEASE {
        if epoch == current_epoch {
            release_desktop_input(shared, desktop);
        }
        return Ok(());
    }
    if body.len() == 9 && body[0] == ACQUIRE_CONTROL {
        if epoch != current_epoch {
            return Ok(());
        }
        if shared
            .input_cleanup
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            drop(state);
            wait_input_cleanup(shared);
            return process_host_command(shared, desktop, body);
        }
        if !input_allowed {
            desktop
                .control_state
                .send_replace((DesktopControlState::Viewing, current_epoch));
            return Ok(());
        }
        let status =
            if state.desktop_control == Some(desktop.stamp) || state.input_holder().is_none() {
                DesktopControlState::Controlling
            } else {
                DesktopControlState::Contended
            };
        let next_epoch = if status == DesktopControlState::Controlling
            && desktop.control_state.borrow().0 != status
        {
            current_epoch.checked_add(1).ok_or(Error::Busy)?
        } else {
            current_epoch
        };
        if status == DesktopControlState::Controlling {
            state.desktop_control = Some(desktop.stamp);
            state.lock_watch.note_controlling();
        }
        desktop.control_state.send_replace((status, next_epoch));
        return Ok(());
    }
    if body.len() == 9 && body[0] == YIELD_CONTROL {
        if epoch == current_epoch {
            release_desktop_input(shared, desktop);
            if state.desktop_control == Some(desktop.stamp) {
                state.desktop_control = None;
            }
            drop(state);
            wait_input_cleanup(shared);
            // A concurrent reacquisition/revoke can replace this lease.
            let state = lock(&shared.state);
            if state.desktop_control != Some(desktop.stamp)
                && desktop.control_state.borrow().1 == current_epoch
            {
                desktop
                    .control_state
                    .send_replace((DesktopControlState::Viewing, current_epoch));
            }
        }
        return Ok(());
    }
    if body[0] != COMMAND {
        return Err(Error::Malformed);
    }
    let command = ControlMessage::decode(&body[9..])?;
    runtime(
        &state,
        desktop.info.service.id,
        desktop.info.service.generation,
        true,
    )?;
    // Contended input is discarded without applying or observing held
    // state; viewers stay connected and request acquisition after the
    // current controller yields. Release/keyframe/quality are unaffected.
    let contended =
        !input_allowed || state.desktop_control != Some(desktop.stamp) || epoch != current_epoch;
    match command {
        ControlMessage::Input { stamp, event } if stamp == desktop.stamp => {
            if contended {
                return Ok(());
            }
            let selected = lock(&desktop.selected).clone();
            if let Err(error) = super::topology::accept_pointer(shared, event, &selected) {
                super::keys::note_input_failure(shared, desktop.info.service.connection_id, &error);
                return Ok(());
            }
            drop(state);
            let Some(mut adapter) = native_input_adapter(shared, desktop, epoch, Some(event))?
            else {
                return Ok(());
            };
            if desktop.retired.load(std::sync::atomic::Ordering::Acquire)
                || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch)
            {
                return Ok(());
            }
            match adapter.as_mut() {
                None => return Err(Error::Unsupported("desktop input adapter")),
                Some(adapter) => {
                    if let Err(error) = adapter.apply(event) {
                        release_late_input(shared, desktop, epoch, adapter.as_mut());
                        super::keys::note_input_failure(
                            shared,
                            desktop.info.service.connection_id,
                            &error,
                        );
                        if matches!(error, Error::InputFailed(_)) {
                            return Ok(());
                        }
                        return Err(error);
                    }
                }
            }
            let mut held = lock(&desktop.host_held);
            held.observe(event);
            if desktop.retired.load(std::sync::atomic::Ordering::Acquire)
                || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch)
            {
                if let Some(adapter) = adapter.as_mut() {
                    for release in held.release_events() {
                        let _ = adapter.apply(release);
                    }
                }
                *held = HeldInput::default();
            } else {
                lock(&desktop.activity).input(event, std::time::Instant::now());
            }
        }
        ControlMessage::SelectDisplays { stamp, displays } if stamp == desktop.stamp => {
            let snapshot = lock(&shared.topology).clone();
            match super::topology::apply_selection(&snapshot.displays, &displays) {
                Ok(ids) => {
                    let changed = {
                        let mut selected = lock(&desktop.selected);
                        let changed = *selected != ids;
                        *selected = ids;
                        changed
                    };
                    *lock(&desktop.display_fault) = None;
                    if changed {
                        desktop.keyframe.request();
                    }
                }
                Err(error) => super::keys::note_input_failure(
                    shared,
                    desktop.info.service.connection_id,
                    &error,
                ),
            }
        }
        ControlMessage::Text { stamp, text } if stamp == desktop.stamp => {
            if contended {
                return Ok(());
            }
            if let Some(problem) = crate::input::key_text_problem(&text.0) {
                return Err(Error::InputFailed(problem));
            }
            drop(state);
            let Some(mut adapter) = native_input_adapter(shared, desktop, epoch, None)? else {
                return Ok(());
            };
            if desktop.retired.load(std::sync::atomic::Ordering::Acquire)
                || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch)
            {
                return Ok(());
            }
            let adapter = adapter
                .as_mut()
                .ok_or(Error::Unsupported("desktop input adapter"))?;
            let result = if text.0.is_empty() {
                Ok(())
            } else {
                adapter.type_text(&text.0)
            };
            let stale = release_late_input(shared, desktop, epoch, adapter.as_mut());
            result?;
            if !stale && !text.0.is_empty() {
                lock(&desktop.activity).touch(std::time::Instant::now());
            }
        }
        ControlMessage::Chord { stamp, text } if stamp == desktop.stamp => {
            if contended {
                return Ok(());
            }
            if let Some(problem) = crate::input::key_text_problem(&text.0) {
                return Err(Error::InputFailed(problem));
            }
            drop(state);
            let Some(mut adapter) = native_input_adapter(shared, desktop, epoch, None)? else {
                return Ok(());
            };
            if desktop.retired.load(std::sync::atomic::Ordering::Acquire)
                || *desktop.control_state.borrow() != (DesktopControlState::Controlling, epoch)
            {
                return Ok(());
            }
            let adapter = adapter
                .as_mut()
                .ok_or(Error::Unsupported("desktop input adapter"))?;
            let result = if text.0.is_empty() {
                Ok(())
            } else {
                adapter.type_chord(&text.0)
            };
            let stale = release_late_input(shared, desktop, epoch, adapter.as_mut());
            result?;
            if !stale && !text.0.is_empty() {
                lock(&desktop.activity).touch(std::time::Instant::now());
            }
        }
        ControlMessage::Keyframe { stamp } if stamp == desktop.stamp => desktop.keyframe.request(),
        ControlMessage::VideoReport { stamp, report } if stamp == desktop.stamp => {
            lock(&desktop.viewer_loss).observe(&report, std::time::Instant::now());
        }
        ControlMessage::Quality { stamp, quality } if stamp == desktop.stamp => {
            let caps = lock(&shared.video_caps).clone();
            // No keyframe here: the encoder takes a new rate as it runs, and
            // a new size opens it again, which starts from a keyframe.
            *lock(&desktop.quality) = crate::session::agree_quality(
                &quality,
                (desktop.info.monitor_width, desktop.info.monitor_height),
                &caps,
            );
        }
        _ => return Err(Error::StaleSession),
    }
    Ok(())
}
async fn write_control_state(
    send: &mut SendStream,
    status: (DesktopControlState, u64),
) -> Result<(), Error> {
    let mut bytes = vec![CONTROL_STATE, status.0.encode()];
    bytes.extend_from_slice(&status.1.to_be_bytes());
    write(send, &bytes).await
}

/// Bytes of video a desktop may have waiting in the transport: three
/// frames at the agreed bitrate, and never under 64 KiB. Beyond it a new
/// frame would only add latency.
fn backlog_budget(bitrate_kbps: u32, frame_rate: u8) -> usize {
    let per_frame = u64::from(bitrate_kbps) * 1000 / 8 / u64::from(frame_rate.max(1));
    usize::try_from(per_frame * 3)
        .unwrap_or(usize::MAX)
        .max(64 * 1024)
}

/// Whether a frame of `wire` bytes goes out now. It must fit the space the
/// transport has without dropping older datagrams, and, unless nothing is
/// waiting, keep the backlog within `budget`: a keyframe larger than the
/// budget still goes once the link has caught up.
fn admits_frame(space: usize, wire: usize, budget: usize) -> bool {
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
struct SendGate {
    /// Frames are dropped until a keyframe goes out.
    awaiting: bool,
    /// The owed keyframe was asked for, and has not been sent or dropped.
    asked: bool,
    /// Wire size of the latest keyframe, to judge when the next would fit.
    keyframe_wire: usize,
    /// Backlog a keyframe just sent may hold on top of the budget, until
    /// the link drains back within it.
    draining: usize,
}

impl SendGate {
    /// Whether a frame is dropped without reaching the gate: a predicted
    /// frame while a keyframe is owed.
    fn skips(&self, keyframe: bool) -> bool {
        self.awaiting && !keyframe
    }

    /// Whether a frame of `wire` bytes goes out now. The frames behind a
    /// keyframe larger than the budget wait for it whatever the gate does,
    /// and dropping one would only cost another keyframe: so they may add
    /// to its backlog until it drains back within the budget.
    fn admits(&mut self, space: usize, wire: usize, budget: usize) -> bool {
        if super::host::DATAGRAM_SEND_BUFFER.saturating_sub(space) <= budget {
            self.draining = 0;
        }
        admits_frame(space, wire, budget.saturating_add(self.draining))
    }

    /// The gate dropped a frame. A dropped keyframe leaves the one owed
    /// still to ask for.
    fn dropped(&mut self, keyframe: bool, wire: usize) {
        self.awaiting = true;
        self.asked = false;
        if keyframe {
            self.keyframe_wire = wire;
        }
    }

    /// A frame went out; a keyframe pays what was owed.
    fn sent(&mut self, keyframe: bool, wire: usize) {
        if keyframe {
            *self = Self {
                keyframe_wire: wire,
                draining: wire,
                ..Self::default()
            };
        }
    }

    /// Whether a keyframe is owed and not yet asked for.
    fn owed(&self) -> bool {
        self.awaiting && !self.asked
    }

    /// Whether to ask the encoder for the owed keyframe now: the link has
    /// room for one as large as the latest.
    fn should_ask(&mut self, space: usize, budget: usize) -> bool {
        if !self.owed() || !admits_frame(space, self.keyframe_wire, budget) {
            return false;
        }
        self.asked = true;
        true
    }
}

/// The video send backlog allowed at the agreed quality.
fn send_budget(desktop: &DesktopRuntime) -> usize {
    let quality = *lock(&desktop.quality);
    backlog_budget(quality.max_bitrate_kbps, quality.max_frame_rate)
}

/// The interval between frames at the agreed frame rate.
fn frame_period(desktop: &DesktopRuntime) -> Duration {
    Duration::from_secs(1) / u32::from(lock(&desktop.quality).max_frame_rate.max(1))
}

async fn host_send(
    shared: &Shared,
    desktop: &DesktopRuntime,
    send: &mut SendStream,
) -> Result<(), Error> {
    let link = lock(&shared.state)
        .incoming
        .get(&desktop.info.service.connection_id)
        .and_then(|s| s.link.clone())
        .ok_or(Error::NoSession)?;
    let mut seq = 0u32;
    let mut gate = SendGate::default();
    let mut congestion = crate::video::Congestion::default();
    let mut path = None;
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
        for frame in desktop.video.drain() {
            // After a frame was dropped, the predicted frames that follow
            // reference it: they go too, until the keyframe owed for it.
            if gate.skips(frame.keyframe) {
                desktop.counters.send_dropped();
                continue;
            }
            // A new path starts its round trip floor afresh.
            let selected = link
                .paths()
                .iter()
                .find(|candidate| candidate.is_selected())
                .map(|selected| (selected.id(), selected.rtt()));
            let rtt = selected.map_or(Duration::ZERO, |(_, rtt)| rtt);
            if selected.map(|(id, _)| id) != path {
                path = selected.map(|(id, _)| id);
                congestion = crate::video::Congestion::default();
            }
            congestion.rtt(rtt);
            let body = crate::video::encode_video_body(&frame);
            let max = link.max_datagram_size();
            #[cfg(test)]
            let max = if desktop
                .force_stream
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                None
            } else {
                max
            };
            if let Some(max) = max.filter(|max| *max > PREFIX + crate::video::FRAG_HEADER + 64) {
                let now = std::time::Instant::now();
                let protection = lock(&desktop.viewer_loss).protection(
                    frame.keyframe,
                    congestion.congested(rtt),
                    now,
                );
                let datagrams = crate::video::fragment(&body, seq, max - PREFIX, protection)
                    .into_iter()
                    .map(|fragment| {
                        let mut bytes = Vec::with_capacity(PREFIX + fragment.len());
                        bytes.extend_from_slice(&epoch(desktop.stamp));
                        bytes.extend_from_slice(&fragment);
                        bytes::Bytes::from(bytes)
                    })
                    .collect::<Vec<_>>();
                let wire: usize = datagrams.iter().map(bytes::Bytes::len).sum();
                if !gate.admits(
                    link.datagram_send_buffer_space(),
                    wire,
                    send_budget(desktop),
                ) {
                    // The link is behind: sending would queue stale video, or
                    // make the transport drop older fragments and so split
                    // frames. Drop this one whole and start again from a
                    // keyframe once the backlog drains.
                    desktop.counters.send_dropped();
                    gate.dropped(frame.keyframe, wire);
                    continue;
                }
                link.send_many_datagrams(&datagrams)
                    .map_err(Error::network_debug)?;
                seq = seq.wrapping_add(1);
                gate.sent(frame.keyframe, wire);
                if frame.keyframe {
                    desktop.counters.keyframe_sent(frame.payload.len());
                }
            } else {
                let mut bytes = vec![VIDEO];
                bytes.extend_from_slice(&body);
                write(send, &bytes).await?;
                gate.sent(frame.keyframe, bytes.len());
                if frame.keyframe {
                    desktop.counters.keyframe_sent(frame.payload.len());
                }
            }
        }
        if gate.should_ask(link.datagram_send_buffer_space(), send_budget(desktop)) {
            desktop.keyframe.request();
        }
        // While a keyframe is owed, look at the backlog again each frame
        // period: a still screen may bring no frame to wake on.
        let owed_check = gate.owed().then(|| frame_period(desktop));
        let current = cursor.borrow_and_update().clone();
        if current != previous {
            if let Some(current) = &current {
                let mut bytes = vec![CURSOR];
                bytes.extend_from_slice(&crate::cursor::encode(
                    current,
                    previous
                        .as_ref()
                        .and_then(|c: &Arc<CursorState>| c.image.as_ref())
                        != current.image.as_ref(),
                ));
                write(send, &bytes).await?;
            }
            previous = current;
        }
        tokio::select! {
            _ = metrics.tick() => {
                let stats = link.stats();
                let queue = desktop.video.telemetry();
                let timing = lock(&desktop.producer_duration).as_ref()
                    .filter(|(_, _, at)| at.elapsed() <= Duration::from_secs(3))
                    .map(|(capture, encode, _)| (*capture, *encode)).unwrap_or_default();
                let feedback = crate::adaptation::HostFeedback {
                    sent_bytes: stats.udp_tx.bytes, lost_bytes: stats.lost_bytes,
                    dropped: queue.dropped, queued: queue.queued, oldest_ms: queue.oldest_ms,
                    send_dropped: desktop.counters.read().send_dropped,
                    capture_us: timing.0, encode_us: timing.1,
                };
                let mut bytes = vec![MEDIA_FEEDBACK];
                bytes.extend(feedback.encode());
                write(send, &bytes).await?;
            }
            _ = &mut notified => {},
            () = tokio::time::sleep(owed_check.unwrap_or_default()), if owed_check.is_some() => {},
            _ = cursor.changed() => {},
            changed = recording_rx.changed() => {
                changed.map_err(|_|Error::NoSession)?;
                let allowed=*recording_rx.borrow_and_update();
                let mut bytes=vec![RECORDING_STATE,u8::from(allowed.0)];bytes.extend_from_slice(&allowed.1.to_be_bytes());write(send,&bytes).await?;
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

fn topology_bytes(shared: &Shared, desktop: &DesktopRuntime) -> Option<Vec<u8>> {
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

#[cfg(test)]
impl Core {
    pub(crate) fn force_desktop_stream(&self, id: u64, generation: u64) {
        runtime(&*self.alive_state().unwrap(), id, generation, true)
            .unwrap()
            .force_stream
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            SessionStamp::new(1, 3),
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
        use crate::tests::support::{pair_for_control, start_direct, RecordingDesktop};
        let host = start_direct();
        let peer = start_direct();
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(RecordingDesktop {
            events: applied.clone(),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let parent = peer.controller_session(parent.id).unwrap();
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
    fn v8_slow_desktop_input_must_not_hold_core_state() {
        parked_input(false);
    }
    #[test]
    fn v8_slow_stream_input_revokes_and_releases_after_async_close() {
        parked_input(true);
    }
    fn parked_input(stream: bool) {
        use crate::tests::support::{pair_for_control, start_direct};
        let host = start_direct();
        let peer = start_direct();
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
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let parent = peer.controller_session(parent.id).unwrap();
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
            while _handle.control_state().unwrap() != DesktopControlState::Controlling {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
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
    fn v9_pointer_wait_must_recheck_topology_before_native_apply() {
        for change in 0..3 {
            queued_pointer(change);
        }
    }
    fn queued_pointer(change: u8) {
        use crate::tests::support::{pair_for_control, start_direct, RecordingDesktop};
        use std::sync::atomic::Ordering;
        let host = start_direct();
        let peer = start_direct();
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
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let parent = peer.controller_session(parent.id).unwrap();
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
    fn v10_empty_desktop_churn_must_bound_retired_cleanup() {
        use crate::tests::support::{pair_for_control, start_direct, RecordingDesktop};
        use std::sync::atomic::Ordering;
        let host = start_direct();
        let peer = start_direct();
        host.set_desktop(Box::new(RecordingDesktop {
            events: Arc::new(Mutex::new(Vec::new())),
        }))
        .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let parent = peer.controller_session(parent.id).unwrap();
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
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !host
                .hosted_services()
                .unwrap()
                .iter()
                .all(|service| service.kind != ServiceKind::Desktop)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "cleanup did not retire owner {index}"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            peak = peak.max(host.shared.input_cleanup.load(Ordering::Acquire));
        }
        drop(adapter);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.shared.input_cleanup.load(Ordering::Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        host.shutdown();
        peer.shutdown();
        assert_eq!(peak, 0, "empty owners must not queue native cleanup");
    }
    #[test]
    fn v10_nonempty_retired_owners_keep_admission_slots() {
        use crate::tests::support::{pair_for_control, start_direct};
        use std::sync::atomic::Ordering;
        let host = start_direct();
        let peer = start_direct();
        let calls = Arc::new(Mutex::new(Vec::new()));
        host.set_desktop(Box::new(CleanupDesktop(calls.clone())))
            .unwrap();
        pair_for_control(&host, &peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while peer.controller_session(parent.id).unwrap().phase != SessionPhase::Active {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let parent = peer.controller_session(parent.id).unwrap();
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
            lock(&desktop.host_held).observe(crate::InputEvent::key_down(crate::input::usage::A));
            drop(desktop);
            peer.close_controller_service(owner.id, owner.generation)
                .unwrap();
            drop(handle);
            peer.forget_controller_service(owner.id).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !host
                .hosted_services()
                .unwrap()
                .iter()
                .all(|service| service.kind != ServiceKind::Desktop)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "cleanup did not retire owner {index}"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
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
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.shared.input_cleanup.load(Ordering::Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while host.shared.desktop_owners.load(Ordering::Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
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
