//! One compositor-authorized RemoteDesktop session owns capture and input.
//! D-Bus Notify methods work without an EIS connection. The worker serializes
//! them on its own runtime: core input callbacks can run inside Tokio already.
use std::os::fd::OwnedFd;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

use ashpd::desktop::{
    remote_desktop::{Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions},
    screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
    PersistMode, Session,
};
use vnilink::{HostDesktop, InputEvent, PointerButton};
use vnilink_media::{Error, FrameSource, RawFrame};

use crate::{PortalCapture, ScreenGrant};
use futures_util::StreamExt;

const INPUT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
enum Notify {
    Key(i32, bool),
    Symbol(i32, bool),
    Pointer(f64, f64),
    Button(i32, bool),
    Wheel(Axis, i32),
    PreciseWheel(f64, f64),
    Motion(f64, f64),
}
enum Command {
    Input(Vec<Notify>, mpsc::Sender<Result<(), vnilink::Error>>),
    Close,
}
struct Connection {
    tx: mpsc::SyncSender<Command>,
    closed: AtomicBool,
}
impl Connection {
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // Wake the worker if idle. If busy/full, it observes closed itself.
            let _ = self.tx.try_send(Command::Close);
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

/// Applies input only through an already-granted portal session.
#[derive(Clone)]
pub struct WaylandInput {
    connection: Arc<Connection>,
    pixels: (u16, u16),
    logical: (i32, i32),
    /// Desktop origin of this granted surface. The portal does not enumerate
    /// other monitors, so this is normally `(0, 0)`.
    origin: (i32, i32),
}
impl WaylandInput {
    fn send(&self, events: Vec<Notify>) -> Result<(), vnilink::Error> {
        if self.connection.closed.load(Ordering::Acquire) {
            return Err(vnilink::Error::InputFailed(
                "the Wayland permission session ended",
            ));
        }
        let (tx, rx) = mpsc::channel();
        self.connection
            .tx
            .try_send(Command::Input(events, tx))
            .map_err(|_| vnilink::Error::InputFailed("the Wayland input worker is unavailable"))?;
        rx.recv_timeout(INPUT_TIMEOUT + Duration::from_millis(500))
            .unwrap_or(Err(vnilink::Error::InputFailed(
                "the Wayland input request timed out",
            )))
    }
    fn close(&self) {
        self.connection.close();
    }
}
impl HostDesktop for WaylandInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), vnilink::Error> {
        self.send(notifications(
            event,
            self.pixels,
            self.logical,
            self.origin,
        )?)
    }
    fn type_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        if text.len() > vnilink::MAX_KEY_TEXT || text.contains('\0') {
            return Err(vnilink::Error::InputFailed("invalid Wayland text input"));
        }
        let events = text
            .chars()
            .flat_map(|ch| {
                let key = keysym(ch);
                [Notify::Symbol(key, true), Notify::Symbol(key, false)]
            })
            .collect();
        self.send(events)
    }
    fn monitor(&self) -> (u16, u16) {
        self.pixels
    }
}

/// Capture and input share the same consent and are revoked together.
/// Someone may allow the screen without remote interaction: the grant is
/// then view-only, and its session still ends the capture when revoked.
/// With a [`ScreenGrant`], the portal keeps the consent until it is revoked
/// in the system's settings: only the first start asks.
pub struct WaylandCapture {
    capture: PortalCapture,
    input: WaylandInput,
    cursor_metadata: bool,
    control: bool,
}
impl WaylandCapture {
    pub fn open(
        startup: vnilink::BackendStartup,
        grant: Option<ScreenGrant>,
    ) -> Result<Self, Error> {
        startup.check().map_err(startup_error)?;
        let (tx, rx) = mpsc::sync_channel(64);
        let connection = Arc::new(Connection {
            tx,
            closed: AtomicBool::new(false),
        });
        let worker = connection.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("vnilink-wayland-input".into())
            .spawn({
                let startup = startup.clone();
                move || worker_main(worker, rx, ready_tx, startup, grant)
            })
            .map_err(|_| Error::CaptureRejected("the Wayland input worker could not start"))?;
        // Consent may need human interaction. Cancellation in the GTK layer
        // fences installation; a lost receiver makes the worker close its grant.
        let (fd, node, logical, cursor_metadata, control) = match startup.receive(&ready_rx) {
            Ok(result) => result?,
            Err(error) => {
                connection.close();
                return Err(startup_error(error));
            }
        };
        startup.check().map_err(|error| {
            connection.close();
            startup_error(error)
        })?;
        let capture = match PortalCapture::open(fd, node, startup.clone()) {
            Ok(capture) => capture,
            Err(error) => {
                connection.close();
                return Err(error);
            }
        };
        let input = WaylandInput {
            connection,
            pixels: capture.size(),
            logical,
            origin: (0, 0),
        };
        let source = Self {
            capture,
            input,
            cursor_metadata,
            control,
        };
        startup.check().map_err(startup_error)?;
        Ok(source)
    }
    pub fn separate_cursor(&self) -> bool {
        self.cursor_metadata
    }
    pub fn cursor(&self) -> Option<vnilink::CursorState> {
        if self.cursor_metadata {
            self.capture.cursor()
        } else {
            None
        }
    }
    /// Input through the grant, when it allowed remote interaction.
    pub fn desktop(&self) -> Option<WaylandInput> {
        self.control.then(|| self.input.clone())
    }
    /// Whether the grant has ended: revoked from the compositor's sharing
    /// controls, refused input, or closed here. It is never granted again.
    pub fn revoked(&self) -> bool {
        self.input.connection.closed.load(Ordering::Acquire)
    }
}
impl FrameSource for WaylandCapture {
    fn size(&self) -> (u16, u16) {
        self.capture.size()
    }
    fn attested_at(&self) -> Option<std::time::Instant> {
        self.capture.attested_at()
    }

    fn attested_display(&self) -> &str {
        "portal"
    }

    fn cursor(&self) -> Option<vnilink::CursorState> {
        WaylandCapture::cursor(self)
    }

    fn poll_frame(&self, time: u64) -> Result<Option<RawFrame>, Error> {
        if self.revoked() {
            return Err(Error::CaptureRejected(
                "the Wayland control permission session ended",
            ));
        }
        self.capture.poll_frame(time)
    }
}
impl Drop for WaylandCapture {
    fn drop(&mut self) {
        self.input.close();
    }
}

type Ready = Result<(OwnedFd, u32, (i32, i32), bool, bool), Error>;
struct Grant {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
    fd: OwnedFd,
    node: u32,
    logical: (i32, i32),
    cursor_metadata: bool,
    /// Keyboard and pointer were granted with the screen.
    control: bool,
}
fn startup_error(_: vnilink::Error) -> Error {
    Error::CaptureRejected("desktop capture startup cancelled or expired")
}
async fn cancelled(startup: &vnilink::BackendStartup) {
    while startup.check().is_ok() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
async fn open_grant(
    startup: &vnilink::BackendStartup,
    grant: Option<&ScreenGrant>,
) -> Result<Grant, Error> {
    startup.check().map_err(startup_error)?;
    let bus = crate::dbus::session_bus()
        .await
        .map_err(|_| Error::Unsupported("the RemoteDesktop portal"))?;
    let proxy = RemoteDesktop::with_connection(bus.clone())
        .await
        .map_err(|_| Error::Unsupported("the RemoteDesktop portal"))?;
    let session = proxy
        .create_session(Default::default())
        .await
        .map_err(|_| {
            Error::CaptureRejected("the RemoteDesktop portal could not create a session")
        })?;
    let token = grant.and_then(ScreenGrant::token);
    // Once a session exists, cancellation must explicitly close it, including
    // while the compositor's permission request is waiting for the user.
    let work = async {
        startup
            .check()
            .map_err(|_| ashpd::Error::Response(ashpd::desktop::ResponseError::Other))?;
        proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default()
                    .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
                    .set_persist_mode(if grant.is_some() {
                        PersistMode::ExplicitlyRevoked
                    } else {
                        PersistMode::DoNot
                    })
                    .set_restore_token(token.as_deref()),
            )
            .await?
            .response()?;
        let cast = Screencast::with_connection(bus).await?;
        let cursor_metadata = cast
            .available_cursor_modes()
            .await
            .is_ok_and(|modes| modes.contains(CursorMode::Metadata));
        // Persistence for a combined session belongs to RemoteDesktop only.
        cast.select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_sources(ashpd::enumflags2::BitFlags::from(SourceType::Monitor))
                .set_cursor_mode(if cursor_metadata {
                    CursorMode::Metadata
                } else {
                    CursorMode::Embedded
                })
                .set_multiple(false),
        )
        .await?
        .response()?;
        let response = proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;
        let control = response
            .devices()
            .contains(DeviceType::Keyboard | DeviceType::Pointer);
        // Each start answers with the token for the next; the one offered
        // is spent. A grant that cannot be kept asks again next time.
        if let (Some(grant), Some(token)) = (grant, response.restore_token()) {
            let _ = grant.keep(token);
        }
        let streams = response.streams();
        let stream = streams
            .first()
            .filter(|stream| {
                streams.len() == 1
                    && stream
                        .source_type()
                        .is_none_or(|kind| kind == SourceType::Monitor)
            })
            .ok_or(ashpd::Error::Response(ashpd::desktop::ResponseError::Other))?;
        let logical = stream
            .size()
            .filter(|(w, h)| *w > 0 && *h > 0)
            .ok_or(ashpd::Error::Response(ashpd::desktop::ResponseError::Other))?;
        let node = stream.pipe_wire_node_id();
        let fd = cast
            .open_pipe_wire_remote(&session, Default::default())
            .await?;
        Ok::<_, ashpd::Error>((fd, node, logical, cursor_metadata, control))
    };
    let result = tokio::select! {
        biased;
        _ = cancelled(startup) => Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Other)),
        result = work => result,
    };
    match result {
        Ok((fd, node, logical, cursor_metadata, control)) => Ok(Grant {
            proxy,
            session,
            fd,
            node,
            logical,
            cursor_metadata,
            control,
        }),
        Err(error) => {
            let _ = tokio::time::timeout(INPUT_TIMEOUT, session.close()).await;
            Err(grant_error(&error))
        }
    }
}
/// Someone at this computer declining the dialog is a denial; anything
/// else, including a grant without exactly one screen, is not.
fn grant_error(error: &ashpd::Error) -> Error {
    match error {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            Error::CaptureDenied("screen sharing was declined on this computer")
        }
        _ => Error::CaptureRejected("choose one screen in the RemoteDesktop permission dialog"),
    }
}
fn worker_main(
    connection: Arc<Connection>,
    rx: mpsc::Receiver<Command>,
    ready: mpsc::Sender<Ready>,
    startup: vnilink::BackendStartup,
    grant: Option<ScreenGrant>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        let _ = ready.send(Err(Error::CaptureRejected(
            "the Wayland input runtime could not start",
        )));
        return;
    };
    // zbus drops proxies by spawning cleanup tasks. Keep a runtime context
    // entered until all portal objects/streams have been dropped.
    let _runtime_context = runtime.enter();
    let grant = match runtime.block_on(open_grant(&startup, grant.as_ref())) {
        Ok(grant) => grant,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let Grant {
        proxy,
        session,
        fd,
        node,
        logical,
        cursor_metadata,
        control,
    } = grant;
    let mut closed = match runtime.block_on(session.receive_closed()) {
        Ok(closed) => Box::pin(closed),
        Err(_) => {
            let _ = ready.send(Err(Error::CaptureRejected(
                "the Wayland permission session could not be monitored",
            )));
            let _ = runtime.block_on(session.close());
            return;
        }
    };
    if startup.check().is_ok()
        && ready
            .send(Ok((fd, node, logical, cursor_metadata, control)))
            .is_ok()
    {
        while !connection.closed.load(Ordering::Acquire) {
            // Pump D-Bus even with no input queued, and notice compositor-side
            // revocation instead of keeping a cached picture alive indefinitely.
            if runtime
                .block_on(async {
                    tokio::time::timeout(Duration::from_millis(1), closed.next()).await
                })
                .is_ok()
            {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Command::Close) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(Command::Input(events, reply)) => {
                    let result = runtime.block_on(async {
                        tokio::time::timeout(INPUT_TIMEOUT, deliver(&proxy, &session, node, events))
                            .await
                    });
                    let result = match result {
                        Ok(Ok(())) => Ok(()),
                        _ => {
                            connection.closed.store(true, Ordering::Release);
                            Err(vnilink::Error::InputFailed(
                                "the Wayland portal rejected input or ended the session",
                            ))
                        }
                    };
                    let _ = reply.send(result);
                }
            }
        }
    }
    connection.closed.store(true, Ordering::Release);
    let _ = runtime.block_on(async { tokio::time::timeout(INPUT_TIMEOUT, session.close()).await });
}
async fn deliver(
    proxy: &RemoteDesktop,
    session: &Session<RemoteDesktop>,
    node: u32,
    events: Vec<Notify>,
) -> ashpd::Result<()> {
    for event in events {
        let state = |down| {
            if down {
                KeyState::Pressed
            } else {
                KeyState::Released
            }
        };
        match event {
            Notify::Key(code, down) => {
                proxy
                    .notify_keyboard_keycode(session, code, state(down), Default::default())
                    .await?
            }
            Notify::Symbol(symbol, down) => {
                proxy
                    .notify_keyboard_keysym(session, symbol, state(down), Default::default())
                    .await?
            }
            Notify::Pointer(x, y) => {
                proxy
                    .notify_pointer_motion_absolute(session, node, x, y, Default::default())
                    .await?
            }
            Notify::Button(button, down) => {
                proxy
                    .notify_pointer_button(session, button, state(down), Default::default())
                    .await?
            }
            Notify::Motion(dx, dy) => {
                proxy
                    .notify_pointer_motion(session, dx, dy, Default::default())
                    .await?
            }
            Notify::PreciseWheel(dx, dy) => {
                proxy
                    .notify_pointer_axis(session, dx, dy, Default::default())
                    .await?
            }
            Notify::Wheel(axis, steps) => {
                proxy
                    .notify_pointer_axis_discrete(session, axis, steps, Default::default())
                    .await?
            }
        }
    }
    Ok(())
}
fn notifications(
    event: InputEvent,
    pixels: (u16, u16),
    logical: (i32, i32),
    origin: (i32, i32),
) -> Result<Vec<Notify>, vnilink::Error> {
    Ok(match event {
        InputEvent::Key { usage, down } => vec![Notify::Key(
            vnilink::linux_evdev_keycode(usage).ok_or(vnilink::Error::InputFailed(
                "this HID key is not mapped for Wayland",
            ))?,
            down,
        )],
        InputEvent::Button { button, down } => vec![Notify::Button(
            match button {
                PointerButton::Left => 0x110,
                PointerButton::Right => 0x111,
                PointerButton::Middle => 0x112,
                PointerButton::Back => 0x116,
                PointerButton::Forward => 0x115,
            },
            down,
        )],
        InputEvent::Pointer { x, y, .. } => {
            if pixels.0 == 0 || pixels.1 == 0 {
                return Err(vnilink::Error::InputFailed(
                    "the pointer is outside the desktop",
                ));
            }
            let local_x = x.checked_sub(origin.0).ok_or(vnilink::Error::InputFailed(
                "the pointer is outside the desktop",
            ))?;
            let local_y = y.checked_sub(origin.1).ok_or(vnilink::Error::InputFailed(
                "the pointer is outside the desktop",
            ))?;
            if local_x < 0
                || local_y < 0
                || u32::try_from(local_x).ok() >= Some(u32::from(pixels.0))
                || u32::try_from(local_y).ok() >= Some(u32::from(pixels.1))
            {
                return Err(vnilink::Error::InputFailed(
                    "the pointer is outside the desktop",
                ));
            }
            vec![Notify::Pointer(
                f64::from(local_x) * f64::from(logical.0) / f64::from(pixels.0),
                f64::from(local_y) * f64::from(logical.1) / f64::from(pixels.1),
            )]
        }
        InputEvent::Motion { dx, dy } => vec![Notify::Motion(f64::from(dx), f64::from(dy))],
        InputEvent::PreciseWheel { dx, dy } => {
            if dx.unsigned_abs() > vnilink::MAX_PRECISE_WHEEL as u32
                || dy.unsigned_abs() > vnilink::MAX_PRECISE_WHEEL as u32
            {
                return Err(vnilink::Error::InputFailed(
                    "scroll delta is outside the supported range",
                ));
            }
            // Portal continuous axes use logical displacement. One detent is
            // ten logical units, matching the Core Graphics line convention.
            vec![Notify::PreciseWheel(
                f64::from(dx) / 12.0,
                f64::from(dy) / 12.0,
            )]
        }
        InputEvent::Wheel { dx, dy } => {
            let mut out = Vec::new();
            if dx != 0 {
                out.push(Notify::Wheel(Axis::Horizontal, i32::from(dx)));
            }
            if dy != 0 {
                out.push(Notify::Wheel(Axis::Vertical, i32::from(dy)));
            }
            out
        }
    })
}
fn keysym(ch: char) -> i32 {
    match ch {
        '\n' | '\r' => 0xff0d,
        '\t' => 0xff09,
        ch if u32::from(ch) <= 0xff => ch as i32,
        ch => (0x0100_0000 | u32::from(ch)) as i32,
    }
}

#[cfg(test)]
#[path = "tests/wayland.rs"]
mod tests;
