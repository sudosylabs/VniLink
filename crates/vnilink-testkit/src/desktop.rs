//! What the desktop suites share: a host serving a scripted screen through
//! its real picture pipeline (`vnilink_media::serve_shared_capture`: reads,
//! changes, stills, the send gate) with a stand-in encoder, a desktop that
//! records the input it applies, and a viewer connected as the application
//! connects one.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use vnilink::{DisplayInfo, EncodedFrame, HostCapture, HostDesktop, InputEvent, ServiceKind};
use vnilink_media::{EncodeParams, EncodedSample, Error, FrameSource, RawFrame, VideoEncoder};

use crate::Pair;

/// The screen's size: small, so the pipeline's own work is what is measured.
pub const SCREEN: (u16, u16) = (64, 64);

/// The display the host serves.
pub const DISPLAY: u64 = 7;

/// What the scripted screen does over time.
#[derive(Clone, Debug)]
pub enum Script {
    /// Never changes.
    Still,
    /// Changes every `every`, `share` of it each time, where `regions` say.
    Ticking {
        every: Duration,
        share: f32,
        regions: Option<Vec<vnilink::Region>>,
    },
}

/// A screen that does what its [`Script`] says, read as a capture is: each
/// read returns the current picture and says what changed since the last.
pub struct ScriptedScreen {
    script: Script,
    started: Instant,
    page: RawFrame,
    /// The tick the last read saw: a read reports a change only after a new one.
    read: Mutex<Option<u128>>,
    /// Pictures read that carried a change.
    pub changes: Arc<AtomicUsize>,
}

impl ScriptedScreen {
    /// When it started: a ticking screen changes at whole multiples of its
    /// interval after this.
    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn new(script: Script) -> Self {
        Self {
            script,
            started: Instant::now(),
            page: vnilink_media::measure::colour_text_page(SCREEN.0, SCREEN.1, 0),
            read: Mutex::new(None),
            changes: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn tick(&self) -> u128 {
        match &self.script {
            Script::Still => 0,
            Script::Ticking { every, .. } => {
                self.started.elapsed().as_millis() / every.as_millis().max(1)
            }
        }
    }
}

impl FrameSource for ScriptedScreen {
    fn size(&self) -> (u16, u16) {
        SCREEN
    }

    fn poll_frame(&self, timestamp: u64) -> Result<Option<RawFrame>, Error> {
        let tick = self.tick();
        let mut read = self
            .read
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let changed = match (&self.script, *read) {
            // The first read is the whole screen.
            (_, None) => None,
            (Script::Ticking { share, regions, .. }, Some(last)) if last != tick => {
                Some(vnilink_media::Changes {
                    share: *share,
                    regions: regions.clone(),
                })
            }
            _ => Some(vnilink_media::Changes::none()),
        };
        *read = Some(tick);
        if changed.as_ref().is_none_or(|changes| changes.share > 0.0) {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Some(
            self.page
                .clone()
                .with_timestamp(timestamp)
                .with_changed(changed),
        ))
    }

    fn attested_at(&self) -> Option<Instant> {
        match &self.script {
            Script::Still => Some(self.started),
            Script::Ticking { every, .. } => {
                Some(self.started + *every * u32::try_from(self.tick()).unwrap_or(u32::MAX))
            }
        }
    }
}

/// Stands in for a hardware or software encoder: a valid H.264 access unit
/// per picture, a few bytes long, at once. What it measures is the
/// pipeline around the encoder, not encoding.
pub struct StandInEncoder {
    pub encoded: Arc<AtomicUsize>,
}

impl VideoEncoder for StandInEncoder {
    fn encode(&mut self, _: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        self.encoded.fetch_add(1, Ordering::Relaxed);
        stand_in_sample(params.force_keyframe)
    }
}

fn stand_in_sample(keyframe: bool) -> Result<EncodedSample, Error> {
    use vnilink_media::{to_annex_b, AvccAccessUnit};
    let picture: &[u8] = if keyframe {
        &[0x65, 0x10]
    } else {
        &[0x41, 0x10]
    };
    let mut avcc = (picture.len() as u32).to_be_bytes().to_vec();
    avcc.extend_from_slice(picture);
    // A real SPS: hosts give every SPS the stream's colour description.
    let sets = [
        vec![0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99],
        vec![0x68, 0xce],
    ];
    let annex = to_annex_b(&AvccAccessUnit {
        nal_length_size: 4,
        avcc: &avcc,
        parameter_sets: &sets,
        keyframe,
    })?;
    EncodedSample::annex_b(keyframe, annex, vnilink::YuvMatrix::Bt709)
}

/// The host's capture as the engine sees it: one display, sending stills.
struct ScreenCapture;

impl HostCapture for ScreenCapture {
    fn monitor(&self) -> (u16, u16) {
        SCREEN
    }
    fn stills(&self) -> bool {
        true
    }
}

/// A desktop that applies input by recording it, with when it arrived.
pub struct RecordingDesktop {
    pub applied: Arc<Mutex<Vec<(Instant, InputEvent)>>>,
}

impl HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), vnilink::Error> {
        self.applied
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push((Instant::now(), event));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        SCREEN
    }
}

/// The host's clipboard: it records the text it is given.
pub struct RecordingClipboard {
    pub texts: Arc<Mutex<Vec<String>>>,
}

impl vnilink::HostClipboard for RecordingClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        self.texts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(text.into());
        Ok(())
    }
}

/// The input a host's recording desktop applied, with when.
pub type AppliedInput = Arc<Mutex<Vec<(Instant, InputEvent)>>>;

/// Sets `host` up to serve the scripted screen: a capture of one
/// [`SCREEN`]-sized display ([`DISPLAY`]) sending stills, and a desktop
/// that records the input it applies, which it returns. Done before a
/// viewer connects, as the engine refuses to swap adapters during a
/// session.
pub fn install_screen(host: &vnilink::Core) -> Result<AppliedInput, vnilink::Error> {
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_capture(Box::new(ScreenCapture))?;
    host.set_desktop(Box::new(RecordingDesktop {
        applied: Arc::clone(&applied),
    }))?;
    host.set_topology(
        1,
        &[DisplayInfo {
            id: DISPLAY,
            origin_x: 0,
            origin_y: 0,
            width: SCREEN.0,
            height: SCREEN.1,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 1,
        }],
    )?;
    Ok(applied)
}

/// The host's real picture pipeline (`vnilink_media::serve_shared_capture`)
/// reading a scripted screen with the stand-in encoder, on a thread of its
/// own, serving whichever viewers the host admits. Dropping it stops it.
pub struct ScreenPipeline {
    /// Pictures the stand-in encoder made.
    pub encoded: Arc<AtomicUsize>,
    /// When the scripted screen started; see [`ScriptedScreen::started`].
    pub screen_started: Instant,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<(), vnilink_media::PublishError>>>,
}

impl ScreenPipeline {
    /// Starts serving `screen` from `host`, set up by [`install_screen`].
    pub fn start(host: &Arc<vnilink::Core>, screen: ScriptedScreen) -> Self {
        let screen_started = screen.started();
        let encoded = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (host, stop, encoded) = (Arc::clone(host), Arc::clone(&stop), Arc::clone(&encoded));
            std::thread::spawn(move || {
                vnilink_media::serve_shared_capture(
                    &screen,
                    &host,
                    &stop,
                    false,
                    DISPLAY,
                    |_| {},
                    move |_, _, _, _, _| {
                        Ok(StandInEncoder {
                            encoded: Arc::clone(&encoded),
                        })
                    },
                )
            })
        };
        Self {
            encoded,
            screen_started,
            stop,
            thread: Some(thread),
        }
    }

    /// Stops the pipeline and says how it ended: `Err` with why when it
    /// failed or panicked.
    pub fn finish(mut self) -> Result<(), String> {
        self.halt().unwrap_or(Ok(()))
    }

    fn halt(&mut self) -> Option<Result<(), String>> {
        self.stop.store(true, Ordering::Release);
        let thread = self.thread.take()?;
        Some(match thread.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("the host's pipeline failed: {error}")),
            Err(_) => Err("the host's pipeline panicked".to_owned()),
        })
    }
}

impl Drop for ScreenPipeline {
    fn drop(&mut self) {
        if let Some(Err(error)) = self.halt() {
            if !std::thread::panicking() {
                panic!("{error}");
            }
        }
    }
}

/// A viewer watching the host's scripted screen through the Remote Desktop
/// service, as the application opens it. Dropping it stops the host's
/// pipeline.
pub struct DesktopSession {
    /// Dropped first: the pipeline stops before the engines shut down.
    pipeline: Option<ScreenPipeline>,
    pub pair: Pair,
    pub service: vnilink::RemoteDesktopService,
    pub frames: vnilink::VideoSubscription,
    /// The input the host's desktop applied.
    pub applied: Arc<Mutex<Vec<(Instant, InputEvent)>>>,
    /// Pictures the stand-in encoder made.
    pub encoded: Arc<AtomicUsize>,
    /// When the scripted screen started; see [`ScriptedScreen::started`].
    pub screen_started: Instant,
}

/// What a viewer's window decodes: H.264, and AV1 at 4:4:4 when it shows
/// `stills`, which need a 4:4:4 decoder.
pub fn window_decoders(stills: bool) -> Vec<vnilink::WindowDecoder> {
    vec![
        vnilink::WindowDecoder {
            codec: vnilink::VideoCodec::H264,
            hardware: false,
            chroma444: false,
            bt601: false,
        },
        vnilink::WindowDecoder {
            codec: vnilink::VideoCodec::Av1,
            hardware: false,
            chroma444: stills,
            bt601: false,
        },
    ]
}

impl DesktopSession {
    /// Pairs two engines, starts the host's pipeline on `screen`, and
    /// connects a viewer whose window shows stills when `stills`.
    pub fn start(screen: ScriptedScreen, stills: bool) -> Self {
        Self::prepared(screen, stills, |_| {})
    }

    /// As [`Self::start`], with `prepare` setting the host up before the
    /// viewer connects, as adapters the engine refuses to swap during a
    /// session (a clipboard, say) must be.
    pub fn prepared(screen: ScriptedScreen, stills: bool, prepare: impl FnOnce(&Pair)) -> Self {
        let pair = Pair::for_desktop();
        prepare(&pair);
        let (host, viewer) = (&pair.host.core, &pair.controller.core);
        let applied = install_screen(host).expect("the host's scripted screen");
        viewer
            .set_window_decoders(window_decoders(stills))
            .expect("the window's decoders");

        let connection = pair.connect_services();
        let owner = pair.attach(&connection, ServiceKind::Desktop);
        let service = viewer
            .open_desktop_service(owner.id, owner.generation)
            .expect("the desktop service");
        let frames = service.subscribe_video().expect("its pictures");

        let pipeline = ScreenPipeline::start(host, screen);
        Self {
            encoded: Arc::clone(&pipeline.encoded),
            screen_started: pipeline.screen_started,
            pipeline: Some(pipeline),
            pair,
            service,
            frames,
            applied,
        }
    }

    /// The pictures that arrive within `watch`.
    pub fn watch(&self, watch: Duration) -> Vec<(Instant, EncodedFrame)> {
        watch_pictures(&self.frames, watch)
    }

    /// The host's side of this Desktop service: its id and generation.
    pub fn hosted(&self) -> (u64, u64) {
        self.pair
            .host
            .core
            .hosted_services()
            .expect("hosted services")
            .into_iter()
            .find(|service| service.kind == ServiceKind::Desktop)
            .map(|service| (service.id, service.generation))
            .expect("a hosted Desktop service")
    }

    /// Lets the controller share the host's clipboard, or not.
    pub fn share_clipboard(&self, clipboard: bool) {
        self.pair
            .host
            .core
            .set_desktop_sharing(vnilink::Grants {
                view: true,
                control: true,
                clipboard,
                transfer: false,
            })
            .expect("sharing");
    }

    /// The input the host applied so far.
    pub fn applied(&self) -> Vec<InputEvent> {
        self.applied
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .iter()
            .map(|(_, event)| *event)
            .collect()
    }
}

impl Drop for DesktopSession {
    fn drop(&mut self) {
        self.pipeline.take();
    }
}

/// The pictures `frames` brings within `watch`, each with when it arrived.
pub fn watch_pictures(
    frames: &vnilink::VideoSubscription,
    watch: Duration,
) -> Vec<(Instant, EncodedFrame)> {
    let until = Instant::now() + watch;
    let mut pictures = Vec::new();
    while Instant::now() < until {
        if let Some(frame) = frames.recv_timeout(Duration::from_millis(20)) {
            pictures.push((Instant::now(), frame));
        }
    }
    pictures
}

/// What a host's desktop applied, in order and when: the log of a desktop
/// [`InputLog::install`] puts on an engine.
#[derive(Clone, Default)]
pub struct InputLog(Arc<Mutex<Vec<(Instant, InputEvent)>>>);

impl InputLog {
    /// Installs on `host` a desktop of one `monitor`-sized display that
    /// applies input by logging it, and gives the log.
    pub fn install(host: &crate::Engine, monitor: (u16, u16)) -> Result<Self, vnilink::Error> {
        let log = Self::default();
        host.core.set_desktop(Box::new(LoggingDesktop {
            log: log.clone(),
            monitor,
        }))?;
        Ok(log)
    }

    /// Every event applied so far.
    pub fn events(&self) -> Vec<InputEvent> {
        self.timed().into_iter().map(|(_, event)| event).collect()
    }

    /// Every event applied so far, with when.
    pub fn timed(&self) -> Vec<(Instant, InputEvent)> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Waits until at least `count` events were applied, and gives them.
    pub fn wait_for(&self, count: usize) -> Vec<InputEvent> {
        crate::wait_for(
            &format!("the host to apply {count} input events"),
            crate::DEFAULT_WAIT,
            || {
                let events = self.events();
                (events.len() >= count).then_some(events)
            },
        )
    }

    /// Waits until `count` events were applied, which must be all there
    /// are: a duplicated release fails here.
    pub fn exactly(&self, count: usize) -> Vec<InputEvent> {
        let events = self.wait_for(count);
        assert_eq!(
            events.len(),
            count,
            "the host applied exactly {count} input events: {events:?}"
        );
        events
    }
}

struct LoggingDesktop {
    log: InputLog,
    monitor: (u16, u16),
}

impl HostDesktop for LoggingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), vnilink::Error> {
        self.log
            .0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push((Instant::now(), event));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }
}

/// A host's capture of one display, set up as adapters are before a
/// session; it offers no still pictures.
pub struct MonitorCapture {
    monitor: (u16, u16),
    separate_cursor: bool,
}

impl MonitorCapture {
    /// A display of `monitor`'s size, its cursor drawn into its pictures.
    pub fn new(monitor: (u16, u16)) -> Self {
        Self {
            monitor,
            separate_cursor: false,
        }
    }

    /// Sends its cursor apart from its pictures when `separate`.
    pub fn separate_cursor(self, separate: bool) -> Self {
        Self {
            separate_cursor: separate,
            ..self
        }
    }
}

impl HostCapture for MonitorCapture {
    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }

    fn separate_cursor(&self) -> bool {
        self.separate_cursor
    }
}

/// A keyframe stamped `timestamp_us`, carrying `payload`, as a host's
/// encoder hands one over.
pub fn keyframe(timestamp_us: u64, payload: Vec<u8>) -> EncodedFrame {
    EncodedFrame::h264(true, timestamp_us, payload).expect("an H.264 keyframe")
}

/// A picture that is not a keyframe, stamped `timestamp_us`, carrying
/// `payload`.
pub fn delta(timestamp_us: u64, payload: Vec<u8>) -> EncodedFrame {
    EncodedFrame::h264(false, timestamp_us, payload).expect("an H.264 frame")
}

/// The next input failure the host reports in `events`: the session it
/// names and why.
pub fn input_failed(events: &vnilink::EventSubscription) -> (u64, &'static str) {
    crate::events::next_picked(
        events,
        "the host to report an input failure",
        crate::DEFAULT_WAIT,
        |event| match event {
            vnilink::CoreEvent::InputFailed { id, message } => Some((id, message)),
            _ => None,
        },
    )
}

/// What a host's desktop was asked to do: an input event, text typed as
/// characters, or text typed as the host layout's keys.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    Event(InputEvent),
    Text(String),
    Chord(String),
}

/// What a host's desktop was asked to type, in order: the log of a desktop
/// [`Typing::install`] puts on an engine.
#[derive(Clone, Default)]
pub struct Typing(Arc<Mutex<Vec<Typed>>>);

impl Typing {
    /// Installs on `host` a desktop of one `monitor`-sized display that
    /// records what it is asked to type and the input it applies, refusing
    /// text when `fail_text` and translated keys when `fail_chord`, as an
    /// operating system can.
    pub fn install(
        host: &crate::Engine,
        monitor: (u16, u16),
        fail_text: bool,
        fail_chord: bool,
    ) -> Result<Self, vnilink::Error> {
        let typing = Self::default();
        host.core.set_desktop(Box::new(TypingDesktop {
            typing: typing.clone(),
            monitor,
            fail_text,
            fail_chord,
        }))?;
        Ok(typing)
    }

    /// Everything asked so far.
    pub fn seen(&self) -> Vec<Typed> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Waits until the host was asked as many things as `expected`, then
    /// states that they are `expected`, in order, for the contract `what`:
    /// one more, or one fewer, fails.
    pub fn wait(&self, expected: &[Typed], what: &str) {
        let got = crate::wait_for(
            &format!("the host to be asked {} things", expected.len()),
            crate::DEFAULT_WAIT,
            || {
                let got = self.seen();
                (got.len() >= expected.len()).then_some(got)
            },
        );
        assert_eq!(got, expected, "{what}");
    }

    fn record(&self, typed: Typed) {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(typed);
    }
}

struct TypingDesktop {
    typing: Typing,
    monitor: (u16, u16),
    fail_text: bool,
    fail_chord: bool,
}

impl HostDesktop for TypingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), vnilink::Error> {
        self.typing.record(Typed::Event(event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        if self.fail_text {
            return Err(vnilink::Error::InputFailed(
                "the operating system rejected the event",
            ));
        }
        self.typing.record(Typed::Text(text.to_owned()));
        Ok(())
    }

    fn type_chord(&mut self, text: &str) -> Result<(), vnilink::Error> {
        if self.fail_chord {
            return Err(vnilink::Error::InputFailed(
                "translated keys are not available",
            ));
        }
        self.typing.record(Typed::Chord(text.to_owned()));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }
}

/// What a [`Scripted`] desktop does on one call, in place of applying it
/// at once.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// The operating system refuses it (`Error::InputFailed`).
    Fail(&'static str),
    /// The desktop cannot apply it (`Error::Unsupported`).
    Unavailable(&'static str),
    /// It waits, holding the backend, until [`Scripted::unpark`]; then
    /// applies: a slow operating system, as slow as the test says.
    Park,
}

/// A desktop that records what it is asked, as [`Typing`] does, and on
/// the calls its script names (counting every event, text and chord from
/// 1) fails or parks instead: a backend a suite drives.
#[derive(Clone)]
pub struct Scripted {
    typing: Typing,
    script: Arc<Mutex<Vec<(usize, Step)>>>,
    calls: Arc<AtomicUsize>,
    /// Calls parked now, and whether they may go on.
    park: Arc<(Mutex<(usize, bool)>, std::sync::Condvar)>,
}

impl Scripted {
    /// Installs on `host` a desktop of one `monitor`-sized display that
    /// follows `script`.
    pub fn install(
        host: &crate::Engine,
        monitor: (u16, u16),
        script: &[(usize, Step)],
    ) -> Result<Self, vnilink::Error> {
        let scripted = Self {
            typing: Typing::default(),
            script: Arc::new(Mutex::new(script.to_vec())),
            calls: Arc::new(AtomicUsize::new(0)),
            park: Arc::new((Mutex::new((0, false)), std::sync::Condvar::new())),
        };
        host.core.set_desktop(Box::new(ScriptedDesktop {
            scripted: scripted.clone(),
            monitor,
        }))?;
        Ok(scripted)
    }

    /// Everything it was asked and applied, in order.
    pub fn seen(&self) -> Vec<Typed> {
        self.typing.seen()
    }

    /// As [`Typing::wait`].
    pub fn wait(&self, expected: &[Typed], what: &str) {
        self.typing.wait(expected, what);
    }

    /// The calls made so far, refused ones too.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }

    /// Waits until a call is parked, holding the backend.
    pub fn wait_parked(&self) {
        crate::wait_for("a call to park", crate::DEFAULT_WAIT, || {
            (crate::lock(&self.park.0).0 > 0).then_some(())
        });
    }

    /// Lets every parked call, and every later one, go on.
    pub fn unpark(&self) {
        crate::lock(&self.park.0).1 = true;
        self.park.1.notify_all();
    }

    /// What the script says of the call `call`, if anything.
    fn step(&self, call: usize) -> Option<Step> {
        crate::lock(&self.script)
            .iter()
            .find(|(at, _)| *at == call)
            .map(|(_, step)| *step)
    }

    /// Follows the script for the next call; `Ok` when it applies.
    fn follow(&self) -> Result<(), vnilink::Error> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel) + 1;
        match self.step(call) {
            None => Ok(()),
            Some(Step::Fail(reason)) => Err(vnilink::Error::InputFailed(reason)),
            Some(Step::Unavailable(reason)) => Err(vnilink::Error::Unsupported(reason)),
            Some(Step::Park) => {
                let (gate, wake) = &*self.park;
                let mut parked = crate::lock(gate);
                parked.0 += 1;
                // Bounded, so a test that never unparks fails rather than
                // hanging its engine; a call that waited too long applies
                // nothing.
                let (mut parked, waited) = wake
                    .wait_timeout_while(parked, crate::DEFAULT_WAIT, |(_, go)| !*go)
                    .unwrap_or_else(|poison| poison.into_inner());
                parked.0 -= 1;
                assert!(
                    !waited.timed_out(),
                    "call {call} stayed parked for {:?}; the test never unparked it",
                    crate::DEFAULT_WAIT
                );
                Ok(())
            }
        }
    }
}

struct ScriptedDesktop {
    scripted: Scripted,
    monitor: (u16, u16),
}

impl HostDesktop for ScriptedDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), vnilink::Error> {
        self.scripted.follow()?;
        self.scripted.typing.record(Typed::Event(event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        self.scripted.follow()?;
        self.scripted.typing.record(Typed::Text(text.to_owned()));
        Ok(())
    }

    fn type_chord(&mut self, text: &str) -> Result<(), vnilink::Error> {
        self.scripted.follow()?;
        self.scripted.typing.record(Typed::Chord(text.to_owned()));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }
}

/// Waits until `host` has no release of held input still queued, so what
/// its desktop applied is all it will apply.
pub fn releases_settled(host: &crate::Engine) {
    crate::wait_for("the host's releases to settle", crate::DEFAULT_WAIT, || {
        (host.core.input_releases_pending() == 0).then_some(())
    });
}

/// The next picture `video` brings, within the kit's wait, failing naming
/// `what`.
pub fn next_picture(video: &vnilink::VideoSubscription, what: &str) -> EncodedFrame {
    crate::wait_for(what, crate::DEFAULT_WAIT, || {
        video.recv_timeout(Duration::from_millis(20))
    })
}
