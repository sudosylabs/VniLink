//! Desktop instances on a neutral parent. Every instance owns its stream,
//! queues and datagram epoch; session APIs cannot publish into this backend.
use super::*;
use crate::input::{HeldInput, Outbound};
use crate::protocol::{ControlMessage, SessionStamp};
use crate::video::{EncodedFrame, KeyframeFlag, VideoPush, VideoSubscription};
use crate::{CursorState, ServiceInfo, ServiceKind, ServicePhase};
use iroh::endpoint::{Connection, RecvStream, SendStream};

pub(super) const TAG: u8 = 10;
const VERSION: u8 = 1;
const VIDEO: u8 = 1;
const CURSOR: u8 = 2;
const COMMAND: u8 = 3;
const RELEASE: u8 = 4;
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
    pub(super) info: DesktopInfo,
    stamp: SessionStamp,
    quality: Mutex<Quality>,
    input: Arc<InputOutbox>,
    inbox: Arc<VideoInbox>,
    subscriber: Mutex<Weak<()>>,
    video: Arc<VideoOutbox>,
    cursor: tokio::sync::watch::Sender<Option<Arc<CursorState>>>,
    keyframe: KeyframeFlag,
    release: KeyframeFlag,
    keyboard: Mutex<DesktopKeyboard>,
    pub(super) clipboard: super::desktop_clipboard::ClipboardState,
    #[cfg(test)]
    force_stream: std::sync::atomic::AtomicBool,
}
struct DesktopKeyboard {
    plan: crate::keyboard::KeyPlan,
    held: HeldInput,
    mode: crate::KeyboardMode,
    swap: bool,
}
impl DesktopKeyboard {
    fn idle(&self) -> bool {
        !self.plan.is_held() && self.held.release_events().is_empty()
    }
}
impl DesktopRuntime {
    fn new(info: DesktopInfo, stamp: SessionStamp) -> Arc<Self> {
        Arc::new(Self {
            quality: Mutex::new(info.quality),
            info,
            stamp,
            input: Arc::new(InputOutbox::new()),
            inbox: Arc::new(VideoInbox::new()),
            subscriber: Mutex::new(Weak::new()),
            video: Arc::new(VideoOutbox::new()),
            cursor: tokio::sync::watch::channel(None).0,
            keyframe: KeyframeFlag::new(),
            release: KeyframeFlag::new(),
            keyboard: Mutex::new(DesktopKeyboard {
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
        let mut keyboard = lock(&self.keyboard);
        keyboard.plan.clear();
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
            }
        }
        keyboard.plan = trial;
        Ok(())
    }
    fn release_input(&self) {
        let mut keyboard = lock(&self.keyboard);
        self.input.clear_pending_input();
        keyboard.plan.clear();
        keyboard.held = HeldInput::default();
        self.release.request();
    }
    fn take_input(&self) -> (bool, Vec<Outbound>) {
        // Release and the following batch must be sampled together: a new key
        // submitted after focus loss must travel after its release barrier.
        let _keyboard = lock(&self.keyboard);
        (self.release.take(), self.input.drain())
    }
    fn metadata(&self) -> DesktopInfo {
        let mut info = self.info.clone();
        info.quality = *lock(&self.quality);
        info
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

fn packet<'a>(bytes: &'a [u8], stamp: SessionStamp) -> Option<&'a [u8]> {
    (bytes.len() > PREFIX && bytes[..PREFIX] == epoch(stamp)).then(|| &bytes[PREFIX..])
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
        state.incoming.as_ref()
    } else {
        state.outgoing.get(&record.info.connection_id)
    };
    if !parent.is_some_and(|s| {
        s.id == record.info.connection_id
            && s.generation == generation
            && s.phase == SessionPhase::Active
    }) {
        return Err(Error::NoSession);
    }
    record.desktop.clone().ok_or(Error::NoSession)
}
impl Core {
    pub fn hosted_desktop_info(&self, id: u64, generation: u64) -> Result<DesktopInfo, Error> {
        Ok(runtime(&*self.alive_state()?, id, generation, true)?.metadata())
    }
    pub fn publish_desktop_video(
        &self,
        id: u64,
        generation: u64,
        frame: EncodedFrame,
    ) -> Result<(), Error> {
        frame.check()?;
        let state = self.alive_state()?;
        let desktop = runtime(&state, id, generation, true)?;
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
impl RemoteDesktopService {
    pub(super) fn active_desktop(&self) -> Result<&Arc<DesktopRuntime>, Error> {
        if self.lease.is_cancelled() {
            return Err(Error::NoSession);
        }
        self.desktop.as_ref().ok_or(Error::Unsupported(
            "scoped operations on a legacy Desktop bridge",
        ))
    }
    pub fn desktop_info(&self) -> Result<DesktopInfo, Error> {
        Ok(self.active_desktop()?.metadata())
    }
    pub fn submit_input(&self, event: crate::InputEvent) -> Result<(), Error> {
        let desktop = self.active_desktop()?;
        let mut keyboard = lock(&desktop.keyboard);
        let event = if keyboard.swap {
            crate::input::swap_command_key(event)
        } else {
            event
        };
        desktop.input.push(event)?;
        keyboard.held.observe(event);
        Ok(())
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
        desktop.input.push_text(text.to_owned())
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
    if !core
        .shared
        .h264_decode
        .load(std::sync::atomic::Ordering::Relaxed)
    {
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
    // Reserve once per owner, including while Open awaits a reply.
    let reservation = DesktopRuntime::new(
        DesktopInfo {
            service: lease.info().clone(),
            monitor_width: 0,
            monitor_height: 0,
            codec: 1,
            quality: crate::session::controller_offer(),
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
                            codec: crate::protocol::CODEC_H264_HIGH,
                            quality: crate::session::controller_offer(),
                            decode: true,
                            files_only: false,
                            services_only: false,
                            hostname: None,
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
                        let ControlMessage::Accept {
                            stamp: epoch,
                            monitor_width,
                            monitor_height,
                            codec,
                            quality,
                            ..
                        } = ControlMessage::decode(&response[2..])?
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
                        let info = DesktopInfo {
                            service: owner_info.clone(),
                            monitor_width,
                            monitor_height,
                            codec: codec & !crate::protocol::SEPARATE_CURSOR,
                            quality,
                            separate_cursor: codec & crate::protocol::SEPARATE_CURSOR != 0,
                        };
                        if !matches!(
                            info.codec,
                            crate::protocol::CODEC_H264 | crate::protocol::CODEC_H264_HIGH
                        ) {
                            return Err(Error::IncompatibleVersion);
                        }
                        let desktop = DesktopRuntime::new(info, epoch);
                        {
                            let mut keyboard = lock(&desktop.keyboard);
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
                                    result = client_receive(&link, &desktop, &mut recv) => result,
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
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let mut dropped = 0;
    loop {
        let (release, commands) = desktop.take_input();
        if release {
            write(send, &[RELEASE]).await?;
        }
        if desktop.keyframe.take() {
            let mut bytes = vec![COMMAND];
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
            };
            let mut bytes = vec![COMMAND];
            bytes.extend_from_slice(&message.encode());
            write(send, &bytes).await?;
        }
        tokio::select! {
            _ = &mut notified => {}, _ = desktop.keyframe.notified() => {}, _ = desktop.release.notified() => {},
            _ = interval.tick() => {
                if let Some(path) = link.paths().iter().find(|p| p.is_selected()) {
                    let now_dropped = desktop.inbox.dropped();
                    let current = *lock(&desktop.quality);
                    if let Some(next) = crate::session::suggest_quality(&current, path.rtt().as_millis().try_into().unwrap_or(u64::MAX), path.is_relay(), desktop.inbox.len(), now_dropped.saturating_sub(dropped)) {
                        let next = crate::session::agree_quality(&next, (desktop.info.monitor_width, desktop.info.monitor_height));
                        desktop.input.push_quality(next)?; *lock(&desktop.quality) = next;
                    }
                    dropped = now_dropped;
                }
            }
        }
    }
}
async fn client_receive(
    link: &Connection,
    desktop: &DesktopRuntime,
    recv: &mut RecvStream,
) -> Result<(), Error> {
    let stream = async {
        loop {
            let body = read(recv).await?;
            match body[0] {
                VIDEO => {
                    let frame = crate::video::decode_video_body(&body[1..])?;
                    if matches!(desktop.inbox.push(frame), VideoPush::KeyframeNeeded) {
                        desktop.keyframe.request();
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
    let datagrams = async {
        let mut fragments = crate::video::Reassembler::new();
        loop {
            let bytes = link.read_datagram().await.map_err(Error::network_debug)?;
            if let Some(fragment) = packet(&bytes, desktop.stamp) {
                if let Some(body) = fragments.push(fragment) {
                    if let Ok(frame) = crate::video::decode_video_body(&body) {
                        if matches!(desktop.inbox.push(frame), VideoPush::KeyframeNeeded) {
                            desktop.keyframe.request();
                        }
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), Error>(())
    };
    // Framed stream reads must never be cancelled by arrival of a datagram:
    // dropping a partially consumed length/body would desynchronize the stream.
    tokio::select! { result = stream => result, result = datagrams => result }
}

struct HostedDesktopGuard {
    shared: Arc<Shared>,
    id: u64,
    generation: u64,
}
impl Drop for HostedDesktopGuard {
    fn drop(&mut self) {
        let mut state = lock(&self.shared.state);
        if let Some(mut record) = state.hosted_services.remove(&self.id) {
            if record.info.generation == self.generation {
                record.change(self.generation, ServicePhase::Closed);
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info,
                });
            }
        }
    }
}
struct HeldGuard<'a> {
    shared: &'a Shared,
    connection_id: u64,
    held: HeldInput,
}
impl Drop for HeldGuard<'_> {
    fn drop(&mut self) {
        super::keys::release_held(self.shared, self.connection_id, &self.held);
    }
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
    // Keep teardown and held-input release ahead of replacement admission.
    let gate = shared.desktop_gate.try_lock();
    let outcome = {
        let mut state = lock(&shared.state);
        let session = live_session(&state, true, connection_id)?;
        if !session.services_only || session.peer_key != peer || session.wire_stamp != parent {
            Err(3)
        } else if !state.trust.service_granted(&peer, ServiceKind::Desktop) {
            Err(6)
        } else if gate.is_err()
            || state.hosted_services.len() >= 16
            || state.hosted_services.values().any(|r| {
                r.info.connection_id == connection_id && r.info.kind == ServiceKind::Desktop
            })
        {
            Err(4)
        } else if !decode
            || !shared
                .h264_encode
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            Err(1)
        } else {
            let monitor = monitor_size(&shared);
            if monitor.0 == 0 || monitor.1 == 0 {
                Err(1)
            } else if let Some(codec) = crate::session::negotiate_codec(
                codec,
                shared
                    .h264_high_encode
                    .load(std::sync::atomic::Ordering::Relaxed),
            ) {
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
                        quality: crate::session::agree_quality(&quality, monitor),
                        separate_cursor: separate,
                    },
                    SessionStamp::new(id, parent.generation),
                );
                let stop = tokio::sync::watch::channel(false).0;
                let receiver = stop.subscribe();
                state.hosted_services.insert(
                    id,
                    super::services::ServiceRecord {
                        info: info.clone(),
                        remote_id: Some(remote_id),
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
    let _gate = gate.map_err(|_| Error::Busy)?;
    let _guard = HostedDesktopGuard {
        shared: shared.clone(),
        id: desktop.info.service.id,
        generation: parent.generation,
    };
    let mut response = vec![VERSION, 0];
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
    shared: &Shared,
    desktop: &DesktopRuntime,
    recv: &mut RecvStream,
) -> Result<(), Error> {
    let mut held = HeldGuard {
        shared,
        connection_id: desktop.info.service.connection_id,
        held: HeldInput::default(),
    };
    loop {
        let body = read(recv).await?;
        if body.as_slice() == [RELEASE] {
            super::keys::release_held(shared, held.connection_id, &held.held);
            held.held = HeldInput::default();
            continue;
        }
        if body[0] != COMMAND {
            return Err(Error::Malformed);
        }
        let command = ControlMessage::decode(&body[1..])?;
        let state = lock(&shared.state);
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
        match command {
            ControlMessage::Input { stamp, event } if stamp == desktop.stamp => {
                let mut adapter = lock(&shared.desktop);
                adapter
                    .as_mut()
                    .ok_or(Error::Unsupported("desktop input adapter"))?
                    .apply(event)?;
                held.held.observe(event);
            }
            ControlMessage::Text { stamp, text } if stamp == desktop.stamp => {
                super::keys::apply_key_text(shared, &text.0, false)?;
            }
            ControlMessage::Chord { stamp, text } if stamp == desktop.stamp => {
                super::keys::apply_key_text(shared, &text.0, true)?;
            }
            ControlMessage::Keyframe { stamp } if stamp == desktop.stamp => {
                desktop.keyframe.request()
            }
            ControlMessage::Quality { stamp, quality } if stamp == desktop.stamp => {
                *lock(&desktop.quality) = crate::session::agree_quality(
                    &quality,
                    (desktop.info.monitor_width, desktop.info.monitor_height),
                );
                desktop.keyframe.request();
            }
            _ => return Err(Error::StaleSession),
        }
    }
}
async fn host_send(
    shared: &Shared,
    desktop: &DesktopRuntime,
    send: &mut SendStream,
) -> Result<(), Error> {
    let link = lock(&shared.state)
        .incoming
        .as_ref()
        .and_then(|s| s.link.clone())
        .ok_or(Error::NoSession)?;
    let mut seq = 0u32;
    let mut cursor = desktop.cursor.subscribe();
    let mut previous = None;
    loop {
        let notified = desktop.video.notified();
        tokio::pin!(notified);
        for frame in desktop.video.drain() {
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
                let datagrams = crate::video::fragment(&body, seq, max - PREFIX)
                    .into_iter()
                    .map(|fragment| {
                        let mut bytes = Vec::with_capacity(PREFIX + fragment.len());
                        bytes.extend_from_slice(&epoch(desktop.stamp));
                        bytes.extend_from_slice(&fragment);
                        bytes::Bytes::from(bytes)
                    })
                    .collect::<Vec<_>>();
                link.send_many_datagrams(&datagrams)
                    .map_err(Error::network_debug)?;
                seq = seq.wrapping_add(1);
            } else {
                let mut bytes = vec![VIDEO];
                bytes.extend_from_slice(&body);
                write(send, &bytes).await?;
            }
        }
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
        tokio::select! { _ = &mut notified => {}, _ = cursor.changed() => {} }
    }
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
    fn failed_input_batch_does_not_commit_keyboard_plan() {
        let desktop = desktop();
        for _ in 0..crate::input::INPUT_QUEUE {
            desktop
                .input
                .push(crate::InputEvent::key_down(crate::input::usage::A))
                .unwrap();
        }
        let key = crate::LocalKey {
            usage: crate::input::usage::LEFT_CONTROL,
            down: true,
            text: "",
        };
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
            .plan_key(crate::LocalKey {
                usage: crate::input::usage::LEFT_CONTROL,
                down: true,
                text: "",
            })
            .unwrap();
        desktop.release_input();
        assert!(lock(&desktop.keyboard).idle());
        desktop
            .plan_key(crate::LocalKey {
                usage: crate::input::usage::B,
                down: true,
                text: "",
            })
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
        assert_eq!(packet(&bytes, current), Some(&[1, 2, 3][..]));
        assert!(packet(&bytes, SessionStamp { id: 6, ..current }).is_none());
        assert!(packet(
            &bytes,
            SessionStamp {
                generation: 2,
                ..current
            }
        )
        .is_none());
        for end in 0..=PREFIX {
            assert!(packet(&bytes[..end], current).is_none());
        }
        bytes[0] ^= 1;
        assert!(packet(&bytes, current).is_none());
    }
}
