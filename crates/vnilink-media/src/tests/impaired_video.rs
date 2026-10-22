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
//!
//! `impaired_desktop_stream_during_upload` streams the same desktop while the
//! viewer's core uploads one large file to the host's over the Files
//! connection, both cores' only other traffic. In relay mode the upload and
//! the stream share each core's one relay connection.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::{
    Core, CoreConfig, CustomRelay, EncodedFrame, Grants, JobState, Listen, RelaySettings,
    RemoteFilesService, ServiceKind, SessionInfo, SessionPhase,
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
    /// The product as it is: the harness viewer asks for nothing itself,
    /// and the core asks for a keyframe when it loses a frame, within its
    /// own limits.
    Passive,
    /// Also ask for a keyframe on every corrupt picture, with no
    /// outstanding-request limit: what unbounded requests would do to the
    /// host.
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
    /// Bytes of the one file the viewer uploads to the host while the
    /// stream runs, if any.
    upload: Option<u64>,
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
            upload: None,
        }
    }
}

/// Boots one loopback core, inside `namespace` when one is named.
///
/// A custom relay keeps the endpoint off the public relays. Without
/// `VNILINK_IMPAIRED_RELAY_URL` nothing listens on it, so the cores talk
/// directly; with it, the harness serves that relay and blocks direct UDP.
fn boot_in(namespace: Option<String>) -> Core {
    let relay =
        std::env::var("VNILINK_IMPAIRED_RELAY_URL").unwrap_or_else(|_| "http://127.0.0.1:9".into());
    std::thread::spawn(move || {
        if let Some(name) = namespace {
            enter_namespace(&name);
        }
        Core::start(CoreConfig {
            listen: Listen::Loopback,
            relay: RelaySettings::custom(vec![CustomRelay::new(relay).unwrap()]).unwrap(),
            ..CoreConfig::default()
        })
        .expect("loopback core")
    })
    .join()
    .expect("boot thread")
}

/// The selected transport path of each of `core`'s connections, as its
/// diagnostics report it: `direct`, `relay` or `none`.
fn selected_paths(core: &Core) -> Value {
    let snapshot = core.diagnostic_snapshot().expect("diagnostic snapshot");
    snapshot
        .connections
        .iter()
        .map(|connection| json!({ "role": connection.role, "path": connection.path, "rtt_ms": connection.rtt_ms }))
        .collect()
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

/// How often the timeline samples the engines.
const SAMPLE_EVERY: Duration = Duration::from_millis(100);

/// The engines' running counters every [`SAMPLE_EVERY`] until `done`: when
/// the host dropped frames at its send gate or sent keyframes, against the
/// round trip, the encoder's rate, what the viewer lost and the phase of
/// the viewer's session `session`.
fn sample_timeline(
    host: &Core,
    viewer: &Core,
    session: u64,
    record: &Host,
    started: Instant,
    done: &AtomicBool,
) -> Vec<Value> {
    let read = |core: &Core| {
        let snapshot = core.diagnostic_snapshot().ok()?;
        let pipeline = serde_json::to_value(snapshot.pipelines.first()?).ok()?;
        let rtt = snapshot
            .connections
            .iter()
            .filter_map(|connection| connection.rtt_ms)
            .max();
        Some((pipeline, rtt))
    };
    let mut samples = Vec::new();
    while !done.load(Ordering::Acquire) {
        let at = millis(started.elapsed());
        let phase = viewer
            .controller_session(session)
            .map_or_else(|_| "gone".to_owned(), |info| phase_label(&info.phase));
        if let (Some((host, rtt)), Some((viewer, _))) = (read(host), read(viewer)) {
            let lost = ["lost_expired", "lost_evicted", "lost_gap", "lost_stale"]
                .iter()
                .filter_map(|key| viewer[key].as_u64())
                .sum::<u64>();
            let bps = record
                .counters
                .lock()
                .unwrap()
                .opens
                .last()
                .map(|open| open.1);
            samples.push(json!({
                "ms": at,
                "session": phase,
                "rtt_ms": rtt,
                "bps": bps,
                "send_dropped": host["send_dropped"],
                "keyframes_sent": host["keyframes_sent"],
                "keyframe_bytes": host["keyframe_bytes"],
                "fragment_loss_permille": host["viewer_fragment_loss_permille"],
                "viewer_lost": lost,
                "viewer_in_dropped": viewer["in_dropped"],
            }));
        }
        std::thread::sleep(SAMPLE_EVERY);
    }
    samples
}

fn phase_label(phase: &SessionPhase) -> String {
    match phase {
        SessionPhase::Failed(_) => "failed".to_owned(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// How often the viewer's session left the active phase in `timeline`.
fn session_interruptions(timeline: &[Value]) -> u64 {
    let active: Vec<bool> = timeline
        .iter()
        .map(|sample| sample["session"] == "active")
        .collect();
    active.windows(2).filter(|pair| pair[0] && !pair[1]).count() as u64
}

/// The longest time the viewer showed no new clean picture, from its first
/// clean picture until `end`: a freeze, whatever its cause.
fn longest_freeze(arrivals: &[Arrival], end: Instant) -> Option<Duration> {
    let clean: Vec<Instant> = arrivals
        .iter()
        .filter(|arrival| arrival.clean)
        .map(|arrival| arrival.at)
        .chain(std::iter::once(end))
        .collect();
    if clean.len() < 2 {
        return None;
    }
    clean.windows(2).map(|pair| pair[1] - pair[0]).max()
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

/// A fixture directory below `VNILINK_IMPAIRED_SCRATCH`, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("VNILINK_IMPAIRED_SCRATCH")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        // Tests in one process run concurrently: each stream gets its own tree.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = base.join(format!(
            "vnilink-impaired-video-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Incompressible, repeatable bytes.
fn write_file(path: &Path, bytes: u64) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).expect("fixture file"));
    let mut state = 0x1A26E_u64.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut left = bytes;
    let mut block = vec![0u8; 1 << 20];
    while left > 0 {
        let take = left.min(block.len() as u64) as usize;
        for chunk in block[..take].chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        file.write_all(&block[..take]).expect("fixture write");
        left -= take as u64;
    }
    file.flush().expect("fixture flush");
}

/// `path` as one of `core`'s file roots and a path relative to it.
fn file_location(core: &Core, path: &Path) -> (u64, String) {
    let path = std::fs::canonicalize(path).expect("fixture exists");
    core.file_roots()
        .unwrap()
        .into_iter()
        .find_map(|(id, root)| {
            let relative = path.strip_prefix(root).ok()?;
            let parts: Vec<_> = relative
                .components()
                .map(|part| part.as_os_str().to_str().expect("UTF-8 fixture path"))
                .collect();
            Some((id, parts.join("/")))
        })
        .expect("fixture belongs to an advertised filesystem root")
}

/// The upload the viewer runs beside the stream.
struct Upload {
    job: u64,
    bytes: u64,
    _service: RemoteFilesService,
    _fixtures: [Scratch; 2],
}

/// Gives both cores a file area, which they accept only before they
/// connect, and writes `bytes` of fixture on the viewer.
fn prepare_upload(host: &Core, viewer: &Core, bytes: u64) -> [Scratch; 2] {
    let fixtures = [Scratch::new("host"), Scratch::new("viewer")];
    for (core, scratch) in [host, viewer].into_iter().zip(&fixtures) {
        core.set_transfer_directory(scratch.0.join("transfer"))
            .unwrap();
        core.enable_filesystem_access().unwrap();
        std::fs::create_dir_all(scratch.0.join("root")).unwrap();
    }
    write_file(&fixtures[1].0.join("root/upload.bin"), bytes);
    fixtures
}

/// Opens a Files service beside the desktop on the viewer's session
/// `parent`, as the app does, and submits the upload of the prepared
/// fixture to the host through it. Submission hashes the file before it
/// returns, so this runs before the stream starts.
fn submit_upload(
    host: &Core,
    viewer: &Core,
    parent: &SessionInfo,
    fixtures: [Scratch; 2],
    bytes: u64,
) -> Upload {
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let service = viewer
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    let (from_root, from) = file_location(viewer, &fixtures[1].0.join("root"));
    let (to_root, to) = file_location(host, &fixtures[0].0.join("root"));
    let job = viewer
        .files_service_upload(
            owner.id,
            owner.generation,
            from_root,
            &from,
            &["upload.bin".to_owned()],
            to_root,
            &format!("{to}/arrived"),
        )
        .expect("upload submitted");
    Upload {
        job,
        bytes,
        _service: service,
        _fixtures: fixtures,
    }
}

/// Drives `upload` until `done`, cancelling it then if it still runs, and
/// reports its progress with what QUIC saw on each core's Files connections
/// once a second. The receiving host advances back to back; the sending
/// viewer only asks for state, every 100 ms.
fn drive_upload(
    host: &Core,
    viewer: &Core,
    upload: &Upload,
    started: Instant,
    done: &AtomicBool,
) -> Value {
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for (core, pause) in [(host, Duration::ZERO), (viewer, Duration::from_millis(100))] {
            let stop = &stop;
            scope.spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let idle = core.drive_jobs().map_or(true, |jobs| jobs.is_empty());
                    std::thread::sleep(if idle {
                        pause.max(Duration::from_millis(1))
                    } else {
                        pause
                    });
                }
            });
        }
        let mut files_paths = Vec::new();
        let mut next_sample = started;
        let mut ended = None;
        let mut snapshot = host.job_snapshot(upload.job).expect("receiver record");
        while !done.load(Ordering::Acquire) {
            snapshot = host.job_snapshot(upload.job).expect("receiver record");
            let now = Instant::now();
            if snapshot.state.is_terminal() {
                ended.get_or_insert(now);
            }
            if now >= next_sample {
                files_paths.push(json!({
                    "ms": millis(now - started),
                    "done_bytes": snapshot.done_bytes,
                    "host": host.files_path_stats(),
                    "viewer": viewer.files_path_stats(),
                }));
                next_sample += Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let ended = ended.unwrap_or_else(Instant::now);
        if !snapshot.state.is_terminal() {
            for core in [host, viewer] {
                let _ = core.job_cancel(upload.job);
            }
        }
        stop.store(true, Ordering::Release);
        let elapsed = (ended - started).as_secs_f64().max(1e-9);
        json!({
            "bytes": upload.bytes,
            "outcome": match snapshot.state {
                JobState::Complete => "complete",
                state if state.is_terminal() => "failed",
                _ => "timed-out",
            },
            "state": format!("{:?}", snapshot.state),
            "error": snapshot.error,
            "elapsed_ms": millis(ended - started),
            "done_bytes": snapshot.done_bytes,
            "mb_per_s": snapshot.done_bytes as f64 / 1e6 / elapsed,
            "files_paths": files_paths,
        })
    })
}

fn run_stream(stream: &Stream) -> Value {
    let host = Arc::new(boot_in(std::env::var("VNILINK_IMPAIRED_HOST_NS").ok()));
    let viewer = boot_in(std::env::var("VNILINK_IMPAIRED_VIEW_NS").ok());
    let fixtures = stream
        .upload
        .map(|bytes| (prepare_upload(&host, &viewer, bytes), bytes));
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
        transfer: stream.upload.is_some(),
    })
    .unwrap();
    if stream.upload.is_some() {
        // The host fetches the upload from the viewer, which grants that.
        viewer
            .set_desktop_sharing(Grants {
                view: false,
                control: false,
                clipboard: false,
                transfer: true,
            })
            .unwrap();
    }
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
    let connected = json!({ "host": selected_paths(&host), "viewer": selected_paths(&viewer) });
    let upload =
        fixtures.map(|(fixtures, bytes)| submit_upload(&host, &viewer, &parent, fixtures, bytes));

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
    let sampling = AtomicBool::new(false);
    let bursts = std::thread::scope(|scope| {
        let schedule = scope.spawn(|| impair(stream, started));
        let timeline =
            scope.spawn(|| sample_timeline(&host, &viewer, parent.id, &record, started, &sampling));
        let transfer = upload
            .as_ref()
            .map(|upload| scope.spawn(|| drive_upload(&host, &viewer, upload, started, &sampling)));
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
        sampling.store(true, Ordering::Release);
        let timeline = timeline.join().expect("timeline sampler");
        let transfer = transfer.map(|driver| driver.join().expect("upload driver"));
        (
            (bursts, timeline, transfer),
            decoder,
            arrivals,
            keyframes,
            requests,
            first_picture,
        )
    });
    let (
        (bursts, timeline, transfer),
        decoder,
        arrivals,
        keyframes_received,
        requests,
        first_picture,
    ) = bursts;
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
    let finished = json!({ "host": selected_paths(&host), "viewer": selected_paths(&viewer) });
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
        "scenario": if upload.is_some() { "combined" } else { "video" },
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
        "longest_freeze_ms": longest_freeze(&arrivals, started + stream.duration).map(millis),
        "session_interruptions": session_interruptions(&timeline),
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
        "paths": { "connected": connected, "finished": finished },
        "bursts": burst_reports,
        "unrecovered_bursts": unrecovered,
        "upload": transfer,
        "timeline": timeline,
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
#[ignore = "impaired-network harness: run through `npm run measure:impaired`"]
fn impaired_desktop_stream_during_upload() {
    let stream = Stream {
        upload: Some(env_u64("VNILINK_IMPAIRED_LARGE_BYTES", 1 << 30)),
        ..Stream::from_env()
    };
    let report = run_stream(&stream);
    let text = serde_json::to_string_pretty(&report).unwrap();
    match std::env::var_os("VNILINK_IMPAIRED_OUT") {
        Some(path) => std::fs::write(path, text).expect("result file"),
        None => eprintln!("{text}"),
    }
}

#[test]
fn desktop_stream_runs_beside_an_upload_on_loopback() {
    let report = run_stream(&Stream {
        duration: Duration::from_secs(4),
        warmup: Duration::ZERO,
        bursts: 0,
        burst: Duration::ZERO,
        burst_start: None,
        burst_end: None,
        policy: ViewerPolicy::Passive,
        upload: Some(8 << 20),
    });
    assert!(report["first_picture_ms"].as_f64().is_some(), "{report}");
    assert!(
        report["frames_received"].as_u64().unwrap() >= 30,
        "{report}"
    );
    let upload = &report["upload"];
    assert_eq!(upload["outcome"], "complete", "{upload}");
    assert_eq!(upload["done_bytes"], 8 << 20);
    assert!(upload["files_paths"]
        .as_array()
        .is_some_and(|samples| !samples.is_empty()));
    assert_eq!(report["session_interruptions"], 0, "{report}");
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
        upload: None,
    });
    assert!(report["first_picture_ms"].as_f64().is_some(), "{report}");
    assert!(
        report["frames_received"].as_u64().unwrap() >= 30,
        "{report}"
    );
    assert!(report["upload"].is_null());
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
