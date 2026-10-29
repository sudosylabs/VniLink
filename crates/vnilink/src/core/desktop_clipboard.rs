//! A bounded, acknowledged clipboard pipe owned by one Desktop instance.
// trace: records every ignored failure
use super::desktop_stream::{runtime, DesktopRuntime};
use super::*;
use crate::clipboard::{self, Frame};
use crate::session::Grant;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const TAG: u8 = 11;
const TIMEOUT: Duration = Duration::from_secs(5);
struct Request {
    text: String,
    deadline: tokio::time::Instant,
    reply: std::sync::mpsc::SyncSender<Result<(), Error>>,
}
pub(super) struct ClipboardState {
    tx: tokio::sync::mpsc::Sender<Request>,
    rx: Mutex<Option<tokio::sync::mpsc::Receiver<Request>>>,
    latest: Mutex<Option<String>>,
    closed: AtomicBool,
    stop: tokio::sync::watch::Sender<bool>,
}
impl ClipboardState {
    pub(super) fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            latest: Mutex::new(None),
            closed: AtomicBool::new(false),
            stop: tokio::sync::watch::channel(false).0,
        }
    }
    pub(super) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.stop.send_replace(true);
        lock(&self.rx).take();
        *lock(&self.latest) = None;
    }
    /// Sends `text` and waits for the other side to apply it; records the
    /// outcome, its size and how long it took, never the text.
    fn submit(&self, service: u64, text: &str) -> Result<(), Error> {
        let started = std::time::Instant::now();
        let result = self.offer(text);
        tracing::debug!(
            target: crate::trace::CLIPBOARD,
            service,
            bytes = text.len(),
            ended = result.as_ref().err().map(Error::name),
            took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "clipboard sent"
        );
        result
    }
    fn offer(&self, text: &str) -> Result<(), Error> {
        clipboard::check_text(text)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Unsupported("Desktop clipboard pipe ended"));
        }
        let (reply, answer) = std::sync::mpsc::sync_channel(1);
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        match self.tx.try_send(Request {
            text: text.into(),
            deadline,
            reply,
        }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return Err(Error::Busy),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Err(Error::Unsupported("Desktop clipboard pipe ended"))
            }
        }
        match answer.recv_timeout(deadline.saturating_duration_since(tokio::time::Instant::now())) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(Error::NoSession),
        }
    }
}
struct PipeGuard(Arc<DesktopRuntime>);
impl Drop for PipeGuard {
    fn drop(&mut self) {
        self.0.clipboard.close();
    }
}

impl RemoteDesktopService {
    /// Returns only after the host acknowledges native clipboard application.
    pub fn submit_clipboard(&self, text: &str) -> Result<(), Error> {
        let span = self.shared.upgrade().map(|shared| shared.span.clone());
        let _engine = span.as_ref().map(tracing::Span::enter);
        let desktop = self.active_desktop()?;
        desktop.clipboard.submit(desktop.info.service.id, text)
    }
    /// Latest host text for this Desktop; never reads the local OS clipboard.
    pub fn clipboard_text(&self) -> Result<Option<String>, Error> {
        let desktop = self.active_desktop()?;
        if desktop.clipboard.closed.load(Ordering::Acquire) {
            return Err(Error::Unsupported("Desktop clipboard pipe ended"));
        }
        let text = lock(&desktop.clipboard.latest).clone();
        Ok(text)
    }
}
impl Core {
    /// Non-owning native clipboard routing to an exact controller Desktop.
    /// This never adopts a replacement owner or keeps its connection alive.
    pub fn submit_controller_desktop_clipboard(
        &self,
        id: u64,
        generation: u64,
        text: &str,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let desktop = runtime(&*self.alive_state()?, id, generation, false)?;
        desktop.clipboard.submit(id, text)
    }
    pub fn controller_desktop_clipboard_text(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Option<String>, Error> {
        let _engine = self.engine();
        let desktop = runtime(&*self.alive_state()?, id, generation, false)?;
        if desktop.clipboard.closed.load(Ordering::Acquire) {
            return Err(Error::Unsupported("Desktop clipboard pipe ended"));
        }
        let text = lock(&desktop.clipboard.latest).clone();
        Ok(text)
    }
    /// Publishes host text for one admitted Desktop, with controller receipt ACK.
    pub fn publish_desktop_clipboard(
        &self,
        id: u64,
        generation: u64,
        text: &str,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let desktop = {
            let state = self.alive_state()?;
            let desktop = runtime(&state, id, generation, true)?;
            allowed(&state, &desktop)?;
            desktop
        };
        desktop.clipboard.submit(id, text)
    }
}
fn allowed(state: &State, desktop: &DesktopRuntime) -> Result<(), Error> {
    if !state.trust.granted(
        &parse_endpoint_id(&desktop.info.service.peer_id)?,
        Grant::Clipboard,
    ) {
        return Err(Error::ClipboardDenied);
    }
    Ok(())
}
fn validate(shared: &Shared, desktop: &DesktopRuntime, hosted: bool) -> Result<(), Error> {
    let state = lock(&shared.state);
    runtime(
        &state,
        desktop.info.service.id,
        desktop.info.service.generation,
        hosted,
    )?;
    if hosted {
        allowed(&state, desktop)?;
    }
    Ok(())
}
fn receive(
    shared: &Shared,
    desktop: &DesktopRuntime,
    hosted: bool,
    text: &str,
) -> Result<(), Error> {
    if hosted {
        // Adapter acquisition precedes validation so waiting old requests cannot
        // enter a replacement. Do not hold core state across native callbacks.
        let mut slot = lock(&shared.clipboard);
        validate(shared, desktop, true)?;
        slot.as_mut()
            .ok_or(Error::Unsupported("no clipboard adapter is installed"))?
            .set_text(text)
    } else {
        let state = lock(&shared.state);
        runtime(
            &state,
            desktop.info.service.id,
            desktop.info.service.generation,
            false,
        )?;
        if desktop.clipboard.closed.load(Ordering::Acquire) {
            return Err(Error::NoSession);
        }
        *lock(&desktop.clipboard.latest) = Some(text.into());
        shared.events.publish(CoreEvent::DesktopClipboardReceived {
            service: desktop.info.service.clone(),
        });
        Ok(())
    }
}

pub(super) async fn client(
    shared: Arc<Shared>,
    link: Connection,
    desktop: Arc<DesktopRuntime>,
    stamp: crate::protocol::SessionStamp,
) {
    let _guard = PipeGuard(desktop.clone());
    let mut stop = desktop.clipboard.stop.subscribe();
    if *stop.borrow() {
        return;
    }
    let work = async {
        let (send, recv) = tokio::time::timeout(TIMEOUT, async {
            let (mut send, mut recv) = link.open_bi().await.map_err(Error::network_debug)?;
            let mut request = vec![TAG];
            request.extend_from_slice(&stamp.id.to_be_bytes());
            request.extend_from_slice(&stamp.generation.to_be_bytes());
            request.extend_from_slice(&desktop.info.service.id.to_be_bytes());
            super::desktop_stream::write(&mut send, &request).await?;
            if super::desktop_stream::read(&mut recv).await?.as_slice() != [0] {
                return Err(Error::NoSession);
            }
            Ok((send, recv))
        })
        .await
        .map_err(|_| Error::Timeout)??;
        let rx = lock(&desktop.clipboard.rx).take().ok_or(Error::Busy)?;
        exchange(shared, desktop, false, send, recv, rx).await
    };
    tokio::select! { _ = stop.changed() => {}, _ = work => {} }
}
pub(super) async fn serve(
    shared: Arc<Shared>,
    connection_id: u64,
    peer: [u8; 32],
    mut send: SendStream,
    recv: RecvStream,
    body: &[u8],
) -> Result<(), Error> {
    if body.len() != 24 {
        return Err(Error::Malformed);
    }
    let id = u64::from_be_bytes(body[..8].try_into().unwrap());
    let generation = u64::from_be_bytes(body[8..16].try_into().unwrap());
    let remote_id = u64::from_be_bytes(body[16..].try_into().unwrap());
    let desktop = {
        let state = lock(&shared.state);
        let desktop = runtime(&state, id, generation, true)?;
        let owner = state.hosted_services.get(&id).ok_or(Error::NoSession)?;
        if owner.remote_id != Some(remote_id)
            || desktop.info.service.connection_id != connection_id
            || parse_endpoint_id(&desktop.info.service.peer_id)? != peer
        {
            return Err(Error::StaleSession);
        }
        desktop
    };
    let rx = lock(&desktop.clipboard.rx).take().ok_or(Error::Busy)?;
    let _guard = PipeGuard(desktop.clone());
    let mut stop = desktop.clipboard.stop.subscribe();
    if *stop.borrow() {
        return Err(Error::NoSession);
    }
    let work = async {
        super::desktop_stream::write(&mut send, &[0]).await?;
        exchange(shared, desktop, true, send, recv, rx).await
    };
    tokio::select! { _ = stop.changed() => Err(Error::NoSession), result = work => result }
}

async fn read(recv: &mut RecvStream) -> Result<Frame, Error> {
    let mut prefix = [0; 4];
    recv.read_exact(&mut prefix)
        .await
        .map_err(Error::network_debug)?;
    let size = u32::from_be_bytes(prefix) as usize;
    clipboard::check_len(size)?;
    let mut body = vec![0; size];
    tokio::time::timeout(TIMEOUT, recv.read_exact(&mut body))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    clipboard::decode(&body)
}
async fn write(send: &mut SendStream, frame: Frame) -> Result<(), Error> {
    let bytes = clipboard::encode(&frame)?;
    send.write_all(&bytes).await.map_err(Error::network_debug)
}
async fn exchange(
    shared: Arc<Shared>,
    desktop: Arc<DesktopRuntime>,
    hosted: bool,
    mut send: SendStream,
    mut recv: RecvStream,
    mut requests: tokio::sync::mpsc::Receiver<Request>,
) -> Result<(), Error> {
    let (frames, mut incoming) = tokio::sync::mpsc::channel(1);
    let mut readers = tokio::task::JoinSet::new();
    readers.spawn(async move {
        loop {
            let frame = read(&mut recv).await;
            let ended = frame.is_err();
            if frames.send(frame).await.is_err() || ended {
                break;
            }
        }
    });
    let mut pending: Option<Request> = None;
    loop {
        let deadline = pending
            .as_ref()
            .map(|r| r.deadline)
            .unwrap_or_else(|| tokio::time::Instant::now() + TIMEOUT);
        tokio::select! {
            _ = tokio::time::sleep_until(deadline), if pending.is_some() => {
                if let Some(request) = pending.take() { crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(Err(Error::Timeout))); }
                return Err(Error::Timeout);
            }
            request = requests.recv(), if pending.is_none() => {
                let Some(request) = request else { return Err(Error::NoSession); };
                if request.deadline <= tokio::time::Instant::now() { crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(Err(Error::Timeout))); continue; }
                if let Err(error) = validate(&shared, &desktop, hosted) { crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(Err(error))); continue; }
                let result = tokio::time::timeout_at(request.deadline, write(&mut send, Frame::Text(request.text.clone()))).await.map_err(|_| Error::Timeout).and_then(|r| r);
                if let Err(error) = result { crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(Err(error))); return Err(Error::NoSession); }
                pending = Some(request);
            }
            frame = incoming.recv() => match frame.ok_or(Error::NoSession)?? {
                Frame::Text(text) => {
                    let result = receive(&shared, &desktop, hosted, &text);
                    tracing::debug!(
                        target: crate::trace::CLIPBOARD,
                        service = desktop.info.service.id,
                        hosted,
                        bytes = text.len(),
                        refused = result.as_ref().err().map(Error::name),
                        reason = result.as_ref().err().and_then(Error::reason),
                        "clipboard received"
                    );
                    tokio::time::timeout(TIMEOUT, write(&mut send, Frame::Status(clipboard::status_code(&result)))).await.map_err(|_| Error::Timeout)??;
                }
                Frame::Status(status) => {
                    let request = pending.take().ok_or(Error::Malformed)?;
                    if request.deadline <= tokio::time::Instant::now() {
                        crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(Err(Error::Timeout)));
                        return Err(Error::Timeout);
                    }
                    crate::ignored!(crate::trace::CLIPBOARD, "the caller stopped waiting for its answer", request.reply.send(clipboard::status_result(status)));
                }
            }
        }
    }
}
