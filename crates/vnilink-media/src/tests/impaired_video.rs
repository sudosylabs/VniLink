//! Desktop video for the impaired-network harness.
//!
//! `npm run measure:impaired` runs `impaired_desktop_stream` inside a Linux
//! container after joining two network namespaces with a `tc netem` veth
//! pair. Each core boots on a thread that first enters its namespace, so its
//! runtime and endpoint socket live there; the namespaces route locally sent
//! loopback traffic over the veth, so the ordinary loopback fixtures cross
//! the impaired link unchanged.
//!
//! The host runs the real shared-capture fanout with a synthetic source and
//! encoder. Encoded sizes follow the encoder's bitrate and frame rate, with
//! keyframes about ten predicted frames large and a periodic keyframe every
//! 120 frames like VideoToolbox. A recovery keyframe, one that answers a
//! loss, is modelled at [`RECOVERY_KEYFRAME_FRAMES`] frames, the budget the
//! platform encoders hold it to; this is a model of that cap, not a
//! measurement of a real encoder, which reaches it only after its first
//! recovery keyframe. Each picture carries its sequence number, so
//! the viewer can tell a clean picture from one decoded against a missing
//! reference, which a real decoder shows as corruption rather than an error.
//!
//! Optional executables in `VNILINK_IMPAIRED_BURST_START` and
//! `VNILINK_IMPAIRED_BURST_END` black out the link for each loss burst; the
//! harness measures how long the viewer takes to show a clean picture again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::{
    Core, CoreConfig, CustomRelay, EncodedFrame, Grants, Listen, RelaySettings, ServiceKind,
    SessionPhase,
};

use crate::error::Error;
use crate::frame::RawFrame;
use crate::pump::{EncodeParams, EncodedSample, VideoEncoder};
use crate::FrameSource;
use crate::RECOVERY_KEYFRAME_FRAMES;

const WIDTH: u16 = 640;
const HEIGHT: u16 = 360;
const GOP: u64 = 120;
const KEYFRAME_RATIO: u64 = 10;
const MARKER: &[u8; 4] = b"VNLH";
const PRODUCED_SLOTS: usize = 8_192;

/// What the viewer does when it notices a reference gap.
#[derive(Clone, Copy, PartialEq)]
enum ViewerPolicy {
    /// The current product: nothing on the receive path requests a keyframe.
    Passive,
    /// Ask for a keyframe on every corrupt picture, with no outstanding-request
    /// limit. This previews what loss-triggered requests do to the host
    /// before the keyframe-discipline change bounds them.
    RequestWhileCorrupt,
}

impl ViewerPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::RequestWhileCorrupt => "request-while-corrupt",
        }
    }
}

struct Stream {
    duration: Duration,
    warmup: Duration,
    bursts: u32,
    burst: Duration,
    burst_start: Option<PathBuf>,
    burst_end: Option<PathBuf>,
    policy: ViewerPolicy,
}

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(text) => text
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a whole number, got {text:?}")),
        Err(_) => default,
    }
}

impl Stream {
    fn from_env() -> Self {
        let hook = |name| std::env::var_os(name).map(PathBuf::from);
        let burst_start = hook("VNILINK_IMPAIRED_BURST_START");
        let burst_end = hook("VNILINK_IMPAIRED_BURST_END");
        assert_eq!(
            burst_start.is_some(),
            burst_end.is_some(),
            "burst hooks come in pairs"
        );
        Self {
            duration: Duration::from_secs(env_u64("VNILINK_IMPAIRED_VIDEO_SECONDS", 30)),
            warmup: Duration::from_secs(env_u64("VNILINK_IMPAIRED_VIDEO_WARMUP_S", 4)),
            bursts: if burst_start.is_some() {
                env_u64("VNILINK_IMPAIRED_BURSTS", 5) as u32
            } else {
                0
            },
            burst: Duration::from_millis(env_u64("VNILINK_IMPAIRED_BURST_MS", 250)),
            burst_start,
            burst_end,
            policy: match std::env::var("VNILINK_IMPAIRED_VIEWER").as_deref() {
                Ok("passive") | Err(_) => ViewerPolicy::Passive,
                Ok("request-while-corrupt") => ViewerPolicy::RequestWhileCorrupt,
                Ok(other) => panic!(
                    "VNILINK_IMPAIRED_VIEWER must be passive or request-while-corrupt, got {other:?}"
                ),
            },
        }
    }
}

fn boot_in(namespace: Option<String>) -> Core {
    std::thread::spawn(move || {
        if let Some(name) = namespace {
            enter_namespace(&name);
        }
        // A custom relay keeps the endpoint off the public relays; nothing
        // listens on it, so the cores talk directly over loopback.
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new("http://127.0.0.1:9").unwrap()])
                .unwrap(),
            ..CoreConfig::default()
        })
        .expect("loopback core")
    })
    .join()
    .expect("boot thread")
}

#[cfg(target_os = "linux")]
fn enter_namespace(name: &str) {
    let path = Path::new("/run/netns").join(name);
    let file = std::fs::File::open(&path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
    nix::sched::setns(&file, nix::sched::CloneFlags::CLONE_NEWNET)
        .unwrap_or_else(|error| panic!("cannot enter network namespace {name}: {error}"));
}

#[cfg(not(target_os = "linux"))]
fn enter_namespace(name: &str) {
    panic!("network namespace {name} needs Linux");
}

/// Attests nothing, so the fanout treats every read as new pixels: a screen
/// in constant motion, never the one-a-second keepalive of a still one.
struct Source;

impl FrameSource for Source {
    fn size(&self) -> (u16, u16) {
        (WIDTH, HEIGHT)
    }

    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        RawFrame::bgra(
            WIDTH,
            HEIGHT,
            timestamp_us,
            vec![0; usize::from(WIDTH) * usize::from(HEIGHT) * 4],
        )
        .map(Some)
    }
}

impl vnilink::HostCapture for Source {
    fn monitor(&self) -> (u16, u16) {
        (WIDTH, HEIGHT)
    }
}

#[derive(Default)]
struct Counters {
    opens: Vec<(u8, u32)>,
    frames: u64,
    bytes: u64,
    keyframes_open: u64,
    keyframes_forced: u64,
    keyframes_periodic: u64,
    keyframe_bytes: u64,
}

/// Host-side record shared by every encoder the fanout opens.
struct Host {
    next: AtomicU64,
    counters: Mutex<Counters>,
    produced: Mutex<Vec<Option<(u64, Instant)>>>,
}

impl Host {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
            counters: Mutex::new(Counters::default()),
            produced: Mutex::new(vec![None; PRODUCED_SLOTS]),
        }
    }

    fn produced_at(&self, index: u64) -> Option<Instant> {
        self.produced.lock().unwrap()[index as usize % PRODUCED_SLOTS]
            .filter(|(slot, _)| *slot == index)
            .map(|(_, at)| at)
    }
}

struct Encoder {
    host: Arc<Host>,
    opened: bool,
    since_keyframe: u64,
}

/// Annex-B access unit of `bytes` bytes that carries `index`. The filler
/// never contains zero bytes, so it cannot imitate a start code.
fn picture(index: u64, keyframe: bool, bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.max(64));
    if keyframe {
        out.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1F]);
        out.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xCE, 0x3C, 0x80]);
        out.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    } else {
        out.extend_from_slice(&[0, 0, 0, 1, 0x41]);
    }
    out.extend_from_slice(MARKER);
    out.extend_from_slice(format!("{index:016x}").as_bytes());
    while out.len() < bytes {
        out.push(0x80 | (out.len() as u8 & 0x7F));
    }
    out
}

fn picture_index(payload: &[u8]) -> Option<u64> {
    let head = &payload[..payload.len().min(64)];
    let at = head.windows(MARKER.len()).position(|w| w == MARKER)? + MARKER.len();
    let digits = std::str::from_utf8(payload.get(at..at + 16)?).ok()?;
    u64::from_str_radix(digits, 16).ok()
}

impl VideoEncoder for Encoder {
    fn encode(&mut self, _frame: &RawFrame, params: &EncodeParams) -> Result<EncodedSample, Error> {
        let opening = !self.opened;
        let keyframe = opening || params.force_keyframe || self.since_keyframe + 1 >= GOP;
        let per_frame =
            (u64::from(params.bitrate_bps) / 8 / u64::from(params.frame_rate.max(1))).max(256);
        let bytes = if keyframe && params.recovery {
            per_frame * u64::from(RECOVERY_KEYFRAME_FRAMES)
        } else if keyframe {
            per_frame * KEYFRAME_RATIO
        } else {
            per_frame
        }
        .min(vnilink::MAX_VIDEO_FRAME as u64 - 64) as usize;
        let index = self.host.next.fetch_add(1, Ordering::Relaxed);
        let payload = picture(index, keyframe, bytes);
        {
            let mut counters = self.host.counters.lock().unwrap();
            counters.frames += 1;
            counters.bytes += payload.len() as u64;
            if keyframe {
                counters.keyframe_bytes += payload.len() as u64;
                if opening {
                    counters.keyframes_open += 1;
                } else if params.force_keyframe {
                    counters.keyframes_forced += 1;
                } else {
                    counters.keyframes_periodic += 1;
                }
            }
        }
        self.host.produced.lock().unwrap()[index as usize % PRODUCED_SLOTS] =
            Some((index, Instant::now()));
        self.opened = true;
        self.since_keyframe = if keyframe { 0 } else { self.since_keyframe + 1 };
        EncodedSample::annex_b(keyframe, payload)
    }
}

/// One received picture.
struct Arrival {
    at: Instant,
    produced: Option<Instant>,
    clean: bool,
    /// Capture to send on the host and time queued for the window, as the
    /// frame header and the viewer inbox report them.
    host_us: u32,
    queued_us: u32,
}

/// Shows a picture only while every frame since the last keyframe arrived.
#[derive(Default)]
struct Decoder {
    last: Option<u64>,
    clean: bool,
    gaps: u64,
}

impl Decoder {
    fn decode(&mut self, frame: &EncodedFrame, index: u64) -> bool {
        if frame.keyframe {
            self.clean = true;
        } else if self.last.map(|last| last + 1) != Some(index) {
            if self.clean {
                self.gaps += 1;
            }
            self.clean = false;
        }
        self.last = Some(index);
        self.clean
    }
}

fn run_hook(path: &Path) {
    let status = std::process::Command::new(path)
        .status()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", path.display()));
    assert!(status.success(), "{} failed: {status}", path.display());
}

/// Blacks out the link on schedule and returns each burst's start and end.
fn impair(stream: &Stream, started: Instant) -> Vec<(Instant, Instant)> {
    let (Some(start), Some(end)) = (&stream.burst_start, &stream.burst_end) else {
        return Vec::new();
    };
    let spacing = stream.duration.saturating_sub(stream.warmup) / stream.bursts.max(1);
    (0..stream.bursts)
        .map(|burst| {
            let due = started + stream.warmup + spacing * burst;
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
            let began = Instant::now();
            run_hook(start);
            std::thread::sleep(stream.burst);
            run_hook(end);
            (began, Instant::now())
        })
        .collect()
}

fn percentile(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    Some(sorted[rank])
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn wait_active(controller: &Core, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while controller.controller_session(id).unwrap().phase != SessionPhase::Active {
        assert!(Instant::now() < deadline, "connection was not admitted");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_stream(stream: &Stream) -> Value {
    let host = Arc::new(boot_in(std::env::var("VNILINK_IMPAIRED_HOST_NS").ok()));
    let viewer = boot_in(std::env::var("VNILINK_IMPAIRED_VIEW_NS").ok());
    host.set_capture(Box::new(Source)).unwrap();
    for core in [&*host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    viewer
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(&viewer.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    host.start_hosting().unwrap();
    let parent = viewer.begin_connect_services(&host.endpoint_id()).unwrap();
    wait_active(&viewer, parent.id);
    let parent = viewer.controller_session(parent.id).unwrap();
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = viewer
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let frames = handle.subscribe_video().unwrap();

    let record = Arc::new(Host::new());
    let stop = Arc::new(AtomicBool::new(false));
    let capture = {
        let host = Arc::clone(&host);
        let record = Arc::clone(&record);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            crate::serve_shared_capture(
                &Source,
                &host,
                &stop,
                false,
                0,
                |_| {},
                move |_, _, _, fps, bps| {
                    record.counters.lock().unwrap().opens.push((fps, bps));
                    Ok(Encoder {
                        host: Arc::clone(&record),
                        opened: false,
                        since_keyframe: 0,
                    })
                },
            )
        })
    };

    let started = Instant::now();
    let bursts = std::thread::scope(|scope| {
        let schedule = scope.spawn(|| impair(stream, started));
        let mut decoder = Decoder::default();
        let mut arrivals = Vec::new();
        let mut keyframes = 0u64;
        let mut requests = 0u64;
        let mut first_picture = None;
        while started.elapsed() < stream.duration {
            let Some(frame) = frames.recv_timeout(Duration::from_millis(20)) else {
                continue;
            };
            let Some(index) = picture_index(&frame.payload) else {
                panic!("a received picture has no sequence marker");
            };
            let at = Instant::now();
            keyframes += u64::from(frame.keyframe);
            let clean = decoder.decode(&frame, index);
            if clean && first_picture.is_none() {
                first_picture = Some(at - started);
            }
            if !clean && stream.policy == ViewerPolicy::RequestWhileCorrupt {
                handle.request_keyframe().unwrap();
                requests += 1;
            }
            arrivals.push(Arrival {
                at,
                produced: record.produced_at(index),
                clean,
                host_us: frame.timing.host_us,
                queued_us: frame.timing.queued_us,
            });
        }
        let bursts = schedule.join().expect("impairment schedule");
        (
            bursts,
            decoder,
            arrivals,
            keyframes,
            requests,
            first_picture,
        )
    });
    let (bursts, decoder, arrivals, keyframes_received, requests, first_picture) = bursts;
    stop.store(true, Ordering::Release);
    capture
        .join()
        .expect("capture thread")
        .expect("shared capture");
    // The engines' own loss, keyframe and reopen counters, read before the
    // service closes and its pipeline leaves the snapshot.
    let pipelines = |core: &Core| {
        let snapshot = core.diagnostic_snapshot().expect("diagnostic snapshot");
        json!({
            "pipelines": serde_json::to_value(&snapshot.pipelines).unwrap(),
            "rtt_ms": snapshot.connections.iter().filter_map(|connection| connection.rtt_ms).max(),
        })
    };
    let engine = json!({ "host": pipelines(&host), "viewer": pipelines(&viewer) });
    handle.close();

    let counters = record.counters.lock().unwrap();
    let mut latency: Vec<f64> = arrivals
        .iter()
        .filter_map(|arrival| arrival.produced.map(|at| millis(arrival.at - at)))
        .collect();
    latency.sort_by(f64::total_cmp);
    let clean = arrivals.iter().filter(|arrival| arrival.clean).count() as u64;
    let stage = |read: fn(&Arrival) -> u32| {
        let mut values: Vec<f64> = arrivals
            .iter()
            .map(|arrival| f64::from(read(arrival)) / 1_000.0)
            .collect();
        values.sort_by(f64::total_cmp);
        json!({ "p50": percentile(&values, 0.50), "p95": percentile(&values, 0.95) })
    };
    let burst_reports: Vec<Value> = bursts
        .iter()
        .enumerate()
        .map(|(position, (began, ended))| {
            let horizon = bursts
                .get(position + 1)
                .map_or(started + stream.duration, |next| next.0);
            // A picture made after the burst ended and shown clean means the
            // stream recovered.
            let recovered = arrivals.iter().find(|arrival| {
                arrival.clean
                    && arrival.at < horizon
                    && arrival.produced.is_some_and(|made| made >= *ended)
            });
            json!({
                "start_ms": millis(*began - started),
                "blackout_ms": millis(*ended - *began),
                "time_to_picture_ms": recovered.map(|arrival| millis(arrival.at - *ended)),
            })
        })
        .collect();
    let unrecovered = burst_reports
        .iter()
        .filter(|burst| burst["time_to_picture_ms"].is_null())
        .count();
    json!({
        "scenario": "video",
        "viewer_policy": stream.policy.label(),
        "duration_s": stream.duration.as_secs_f64(),
        "gop_frames": GOP,
        "keyframe_ratio": KEYFRAME_RATIO,
        "encoder_opens": counters.opens.iter().map(|(fps, bps)| json!({"fps": fps, "bps": bps})).collect::<Vec<_>>(),
        "frames_produced": counters.frames,
        "bytes_produced": counters.bytes,
        "keyframes_produced": {
            "open": counters.keyframes_open,
            "forced": counters.keyframes_forced,
            "periodic": counters.keyframes_periodic,
            "bytes": counters.keyframe_bytes,
        },
        "frames_received": arrivals.len(),
        "keyframes_received": keyframes_received,
        "delivery_ratio": arrivals.len() as f64 / counters.frames.max(1) as f64,
        "clean_ratio": clean as f64 / counters.frames.max(1) as f64,
        "reference_gaps": decoder.gaps,
        "viewer_keyframe_requests": requests,
        "viewer_lagged": frames.lagged(),
        "first_picture_ms": first_picture.map(millis),
        "latency_ms": {
            "p50": percentile(&latency, 0.50),
            "p95": percentile(&latency, 0.95),
            "p99": percentile(&latency, 0.99),
        },
        "stage_ms": {
            "host": stage(|arrival| arrival.host_us),
            "viewer_queue": stage(|arrival| arrival.queued_us),
        },
        "engine": engine,
        "bursts": burst_reports,
        "unrecovered_bursts": unrecovered,
    })
}

#[test]
#[ignore = "impaired-network harness: run through `npm run measure:impaired`"]
fn impaired_desktop_stream() {
    let report = run_stream(&Stream::from_env());
    let text = serde_json::to_string_pretty(&report).unwrap();
    match std::env::var_os("VNILINK_IMPAIRED_OUT") {
        Some(path) => std::fs::write(path, text).expect("result file"),
        None => eprintln!("{text}"),
    }
}

#[test]
fn desktop_stream_delivers_clean_pictures_on_loopback() {
    let report = run_stream(&Stream {
        duration: Duration::from_secs(3),
        warmup: Duration::ZERO,
        bursts: 0,
        burst: Duration::ZERO,
        burst_start: None,
        burst_end: None,
        policy: ViewerPolicy::Passive,
    });
    assert!(report["first_picture_ms"].as_f64().is_some(), "{report}");
    assert!(
        report["frames_received"].as_u64().unwrap() >= 30,
        "{report}"
    );
    assert_eq!(report["keyframes_produced"]["open"], 1, "{report}");
    assert_eq!(report["reference_gaps"], 0, "{report}");
    assert!(report["latency_ms"]["p50"].as_f64().is_some());
}

#[test]
fn pictures_carry_a_sequence_number_without_start_codes() {
    for (index, keyframe, bytes) in [(0, true, 4_000), (u64::MAX, false, 300)] {
        let payload = picture(index, keyframe, bytes);
        assert_eq!(payload.len(), bytes);
        assert_eq!(picture_index(&payload), Some(index));
        let parsed = crate::parse_annex_b(&payload).unwrap();
        assert_eq!(parsed.has_idr(), keyframe);
        assert_eq!(parsed.has_sps() && parsed.has_pps(), keyframe);
    }
}
