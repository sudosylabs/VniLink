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

/// A viewer watching the host's scripted screen through the Remote Desktop
/// service, as the application opens it. Dropping it stops the host's
/// pipeline.
pub struct DesktopSession {
    pub pair: Pair,
    pub service: vnilink::RemoteDesktopService,
    pub frames: vnilink::VideoSubscription,
    /// The input the host's desktop applied.
    pub applied: Arc<Mutex<Vec<(Instant, InputEvent)>>>,
    /// Pictures the stand-in encoder made.
    pub encoded: Arc<AtomicUsize>,
    /// When the scripted screen started; see [`ScriptedScreen::started`].
    pub screen_started: Instant,
    stop: Arc<AtomicBool>,
    pipeline: Option<std::thread::JoinHandle<Result<(), vnilink_media::PublishError>>>,
}

/// What the viewer's window can show; `stills` asks for still pictures,
/// which need a 4:4:4 decoder.
fn window_decoders(stills: bool) -> Vec<vnilink::WindowDecoder> {
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
        let applied = Arc::new(Mutex::new(Vec::new()));
        host.set_capture(Box::new(ScreenCapture))
            .expect("a capture");
        host.set_desktop(Box::new(RecordingDesktop {
            applied: Arc::clone(&applied),
        }))
        .expect("a desktop");
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
        )
        .expect("a topology");
        viewer
            .set_window_decoders(window_decoders(stills))
            .expect("the window's decoders");

        let connection = pair.connect_services();
        let owner = pair.attach(&connection, ServiceKind::Desktop);
        let service = viewer
            .open_desktop_service(owner.id, owner.generation)
            .expect("the desktop service");
        let frames = service.subscribe_video().expect("its pictures");

        let screen_started = screen.started();
        let encoded = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let pipeline = {
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
            pair,
            service,
            frames,
            applied,
            encoded,
            screen_started,
            stop,
            pipeline: Some(pipeline),
        }
    }

    /// The pictures that arrive within `watch`.
    pub fn watch(&self, watch: Duration) -> Vec<(Instant, EncodedFrame)> {
        let until = Instant::now() + watch;
        let mut frames = Vec::new();
        while Instant::now() < until {
            if let Some(frame) = self.frames.recv_timeout(Duration::from_millis(20)) {
                frames.push((Instant::now(), frame));
            }
        }
        frames
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
        self.stop.store(true, Ordering::Release);
        if let Some(pipeline) = self.pipeline.take() {
            let ended = pipeline.join();
            if !std::thread::panicking() {
                ended
                    .expect("the host's pipeline did not panic")
                    .expect("the host's pipeline ended cleanly");
            }
        }
    }
}
