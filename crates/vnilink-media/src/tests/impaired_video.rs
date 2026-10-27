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
//! connection, both cores' only other traffic, and
//! `impaired_desktop_stream_during_download` while it downloads one from the
//! host's, so the file goes the video's way. In relay mode the transfer and
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
    /// The one file moved between the cores while the stream runs, if any.
    transfer: Option<Transfer>,
}

/// Which way the file goes beside the stream.
#[derive(Clone, Copy, PartialEq)]
enum Direction {
    /// From the viewer to the host: against the video.
    Upload,
    /// From the host to the viewer: the video's way.
    Download,
}

impl Direction {
    fn label(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }
}

#[derive(Clone, Copy)]
struct Transfer {
    direction: Direction,
    bytes: u64,
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
            transfer: None,
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
        // A real SPS: hosts give every SPS the stream's VUI.
        out.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e, 0xda, 0x10, 0x99]);
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
        EncodedSample::annex_b(keyframe, payload, vnilink::YuvMatrix::Bt709)
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
                "conditions": viewer["conditions"]
                    .as_array()
                    .map(|held| held.iter().map(|entry| entry["condition"].clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            }));
        }
        std::thread::sleep(SAMPLE_EVERY);
    }
    samples
}

/// Conditions that need no attention by themselves.
const INFORMATIONAL: [&str; 1] = ["relay.path"];

/// Which condition the viewer held as its worst most often over the
/// timeline, and every condition it held at all. Dominant is `null` when
/// no condition was worst in at least a fifth of the samples.
fn condition_summary(timeline: &[Value]) -> Value {
    let mut worst: Vec<(String, u64)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for sample in timeline {
        let held: Vec<&str> = sample["conditions"]
            .as_array()
            .map(|held| held.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for label in &held {
            if !seen.iter().any(|known| known == label) {
                seen.push((*label).to_owned());
            }
        }
        if let Some(label) = held.iter().find(|label| !INFORMATIONAL.contains(label)) {
            match worst.iter_mut().find(|(known, _)| known == label) {
                Some((_, count)) => *count += 1,
                None => worst.push(((*label).to_owned(), 1)),
            }
        }
    }
    let dominant = worst
        .iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count * 5 >= timeline.len() as u64)
        .map(|(label, _)| label.clone());
    json!({
        "dominant": dominant,
        "worst_samples": worst
            .iter()
            .map(|(label, count)| (label.clone(), json!(count)))
            .collect::<serde_json::Map<_, _>>(),
        "seen": seen,
        "samples": timeline.len(),
    })
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

/// `path` on `core`'s shared disk, as the engine names it: its root and
/// the path below it, never a disk's root itself.
fn file_location(core: &Core, path: &Path) -> (u64, String) {
    let (root, parent, name) = core
        .locate(path)
        .expect("the engine is running")
        .expect("the fixture is below a root of the shared disk");
    if parent.is_empty() {
        (root, name)
    } else {
        (root, format!("{parent}/{name}"))
    }
}

/// The transfer the viewer runs beside the stream.
struct Running {
    job: u64,
    transfer: Transfer,
    _service: RemoteFilesService,
    _fixtures: [Scratch; 2],
}

/// Gives both cores a file area, which they accept only before they
/// connect, and writes the fixture on the side that sends it.
fn prepare_transfer(host: &Core, viewer: &Core, transfer: Transfer) -> [Scratch; 2] {
    let fixtures = [Scratch::new("host"), Scratch::new("viewer")];
    for (core, scratch) in [host, viewer].into_iter().zip(&fixtures) {
        core.set_transfer_directory(scratch.0.join("transfer"))
            .unwrap();
        core.enable_filesystem_access().unwrap();
        std::fs::create_dir_all(scratch.0.join("root")).unwrap();
    }
    let sender = match transfer.direction {
        Direction::Upload => &fixtures[1],
        Direction::Download => &fixtures[0],
    };
    write_file(&sender.0.join("root/large.bin"), transfer.bytes);
    fixtures
}

/// Opens a Files service beside the desktop on the viewer's session
/// `parent`, as the app does, and submits the transfer of the prepared
/// fixture through it. Submission hashes the file before it returns, so
/// this runs before the stream starts.
fn submit_transfer(
    host: &Core,
    viewer: &Core,
    parent: &SessionInfo,
    fixtures: [Scratch; 2],
    transfer: Transfer,
) -> Running {
    let owner = viewer
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let service = viewer
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    let (host_root, host_folder) = file_location(host, &fixtures[0].0.join("root"));
    let (viewer_root, viewer_folder) = file_location(viewer, &fixtures[1].0.join("root"));
    let names = ["large.bin".to_owned()];
    let job = match transfer.direction {
        Direction::Upload => viewer.files_service_upload(
            owner.id,
            owner.generation,
            viewer_root,
            &viewer_folder,
            &names,
            host_root,
            &format!("{host_folder}/arrived"),
        ),
        Direction::Download => viewer.files_service_download(
            owner.id,
            owner.generation,
            host_root,
            &host_folder,
            &names,
            viewer_root,
            &format!("{viewer_folder}/arrived"),
        ),
    }
    .expect("transfer submitted");
    Running {
        job,
        transfer,
        _service: service,
        _fixtures: fixtures,
    }
}

/// Drives `running` until `done`, cancelling it then if it still runs, and
/// reports its progress with what QUIC saw on each core's Files connections
/// once a second. The receiving core advances back to back; the sending one
/// only asks for state, every 100 ms.
fn drive_transfer(
    host: &Core,
    viewer: &Core,
    running: &Running,
    started: Instant,
    done: &AtomicBool,
) -> Value {
    let (receiver, sender) = match running.transfer.direction {
        Direction::Upload => (host, viewer),
        Direction::Download => (viewer, host),
    };
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        for (core, pause) in [
            (receiver, Duration::ZERO),
            (sender, Duration::from_millis(100)),
        ] {
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
        let mut snapshot = receiver.job_snapshot(running.job).expect("receiver record");
        while !done.load(Ordering::Acquire) {
            snapshot = receiver.job_snapshot(running.job).expect("receiver record");
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
                let _ = core.job_cancel(running.job);
            }
        }
        stop.store(true, Ordering::Release);
        let elapsed = (ended - started).as_secs_f64().max(1e-9);
        json!({
            "direction": running.transfer.direction.label(),
            "bytes": running.transfer.bytes,
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
        .transfer
        .map(|transfer| (prepare_transfer(&host, &viewer, transfer), transfer));
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
        transfer: stream.transfer.is_some(),
    })
    .unwrap();
    if stream.transfer.is_some() {
        // Both ends of a transfer hold the grant: the sender serves under
        // it and the receiver fetches and writes under its own.
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
    let running = fixtures
        .map(|(fixtures, transfer)| submit_transfer(&host, &viewer, &parent, fixtures, transfer));

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
        let transfer = running.as_ref().map(|running| {
            scope.spawn(|| drive_transfer(&host, &viewer, running, started, &sampling))
        });
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
        let transfer = transfer.map(|driver| driver.join().expect("transfer driver"));
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
        "scenario": match stream.transfer.map(|transfer| transfer.direction) {
            None => "video",
            Some(Direction::Upload) => "combined",
            Some(Direction::Download) => "combined-download",
        },
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
        "conditions": condition_summary(&timeline),
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
        "transfer": transfer,
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
    stream_beside(Direction::Upload);
}

#[test]
#[ignore = "impaired-network harness: run through `npm run measure:impaired`"]
fn impaired_desktop_stream_during_download() {
    stream_beside(Direction::Download);
}

/// Runs the stream beside a transfer of the large-file case `direction`.
fn stream_beside(direction: Direction) {
    let stream = Stream {
        transfer: Some(Transfer {
            direction,
            bytes: env_u64("VNILINK_IMPAIRED_LARGE_BYTES", 1 << 30),
        }),
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
fn desktop_stream_runs_beside_a_transfer_either_way_on_loopback() {
    for direction in [Direction::Upload, Direction::Download] {
        let report = run_stream(&Stream {
            duration: Duration::from_secs(4),
            warmup: Duration::ZERO,
            bursts: 0,
            burst: Duration::ZERO,
            burst_start: None,
            burst_end: None,
            policy: ViewerPolicy::Passive,
            transfer: Some(Transfer {
                direction,
                bytes: 8 << 20,
            }),
        });
        assert!(report["first_picture_ms"].as_f64().is_some(), "{report}");
        assert!(
            report["frames_received"].as_u64().unwrap() >= 30,
            "{report}"
        );
        let transfer = &report["transfer"];
        assert_eq!(transfer["direction"], direction.label());
        assert_eq!(transfer["outcome"], "complete", "{transfer}");
        assert_eq!(transfer["done_bytes"], 8 << 20);
        assert!(transfer["files_paths"]
            .as_array()
            .is_some_and(|samples| !samples.is_empty()));
        assert_eq!(report["session_interruptions"], 0, "{report}");
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
        transfer: None,
    });
    assert!(report["first_picture_ms"].as_f64().is_some(), "{report}");
    assert!(
        report["frames_received"].as_u64().unwrap() >= 30,
        "{report}"
    );
    assert!(report["transfer"].is_null());
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

#[test]
fn the_dominant_condition_is_the_worst_one_held_most_often() {
    let sample = |held: &[&str]| json!({ "conditions": held });
    let timeline = [
        sample(&["relay.path"]),
        sample(&["link.loss", "relay.path"]),
        sample(&["link.loss"]),
        sample(&["link.congestion", "link.loss"]),
        sample(&[]),
    ];
    let summary = condition_summary(&timeline);
    assert_eq!(summary["dominant"], "link.loss");
    assert_eq!(summary["worst_samples"]["link.congestion"], 1);
    assert_eq!(
        summary["seen"],
        json!(["relay.path", "link.loss", "link.congestion"])
    );
    // A relay alone, or a condition in under a fifth of the samples, dominates nothing.
    let mostly_clean: Vec<Value> = (0..10)
        .map(|index| {
            sample(if index == 0 {
                &["link.loss"]
            } else {
                &["relay.path"]
            })
        })
        .collect();
    assert!(condition_summary(&mostly_clean)["dominant"].is_null());
}

/// Pages of coloured text, so a still picture of them is as large as one of
/// a real screen of text.
const STILLS_WIDTH: u16 = 1920;
const STILLS_HEIGHT: u16 = 1080;

/// How long the scenario's screen stays the same, then moves. A still
/// picture is made `STILL_AFTER` into the still period and takes a moment
/// to encode and send: the screen moves again while it is on the wire, the
/// worst moment for a change.
const STILL_PERIOD: Duration = Duration::from_millis(2_250);
const MOVING_PERIOD: Duration = Duration::from_millis(750);

/// A screen that stays the same for `still`, then moves for `moving`, over
/// and over. Each still period shows the other of two pages, so each still
/// picture differs from the one before.
struct Alternating {
    started: Instant,
    still: Duration,
    moving: Duration,
    pages: [RawFrame; 2],
}

impl Alternating {
    fn new(started: Instant, still: Duration, moving: Duration) -> Self {
        let page = |index| crate::measure::colour_text_page(STILLS_WIDTH, STILLS_HEIGHT, index);
        Self {
            started,
            still,
            moving,
            pages: [page(0), page(40)],
        }
    }

    /// The period `at` falls in, and whether the screen is moving then.
    fn phase(&self, at: Instant) -> (u64, bool) {
        let cycle = (self.still + self.moving).as_micros() as u64;
        let since = at.saturating_duration_since(self.started).as_micros() as u64;
        (
            since / cycle,
            since % cycle >= self.still.as_micros() as u64,
        )
    }

    /// When the screen starts moving in each period before `end`.
    fn onsets(&self, end: Instant) -> Vec<Instant> {
        (0..)
            .map(|period| self.started + (self.still + self.moving) * period + self.still)
            .take_while(|onset| *onset < end)
            .collect()
    }
}

impl FrameSource for Alternating {
    fn size(&self) -> (u16, u16) {
        (STILLS_WIDTH, STILLS_HEIGHT)
    }

    fn poll_frame(&self, timestamp_us: u64) -> Result<Option<RawFrame>, Error> {
        let (period, _) = self.phase(Instant::now());
        Ok(Some(
            self.pages[period as usize % 2]
                .clone()
                .with_timestamp(timestamp_us),
        ))
    }

    /// A new attestation for every read while moving; the start of the
    /// still period while still.
    fn attested_at(&self) -> Option<Instant> {
        let now = Instant::now();
        let (period, moving) = self.phase(now);
        Some(if moving {
            now
        } else {
            self.started + (self.still + self.moving) * period as u32
        })
    }
}

/// A host whose capture sends still pictures or not.
struct StillsHost(bool);

impl vnilink::HostCapture for StillsHost {
    fn monitor(&self) -> (u16, u16) {
        (STILLS_WIDTH, STILLS_HEIGHT)
    }
    fn stills(&self) -> bool {
        self.0
    }
}

/// One stream of a screen that alternates still and moving, with still
/// pictures `on` or not: how soon each change after a still period shows,
/// and what the stills cost.
fn run_stills(stream: &Stream, on: bool) -> Value {
    let host = Arc::new(boot_in(std::env::var("VNILINK_IMPAIRED_HOST_NS").ok()));
    let viewer = boot_in(std::env::var("VNILINK_IMPAIRED_VIEW_NS").ok());
    host.set_capture(Box::new(StillsHost(on))).unwrap();
    for core in [&*host, &viewer] {
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
    }
    let decoder = |codec, chroma444| vnilink::WindowDecoder {
        codec,
        hardware: false,
        chroma444,
        bt601: false,
    };
    viewer
        .set_window_decoders(vec![
            decoder(vnilink::VideoCodec::H264, false),
            decoder(vnilink::VideoCodec::Av1, on),
        ])
        .unwrap();
    // The app's Smooth preset by default: the largest still pictures, so
    // the longest a change can wait behind one.
    let kbps = env_u64("VNILINK_IMPAIRED_STILLS_KBPS", 12_000) as u32;
    viewer
        .set_desktop_quality_defaults(
            vnilink::Quality::new(kbps, 60, STILLS_WIDTH, STILLS_HEIGHT).expect("a valid quality"),
        )
        .unwrap();
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
    assert_eq!(handle.desktop_info().unwrap().stills, on, "stills agreed");
    let frames = handle.subscribe_video().unwrap();
    let connected = json!({ "host": selected_paths(&host), "viewer": selected_paths(&viewer) });

    let started = Instant::now();
    let source = Arc::new(Alternating::new(started, STILL_PERIOD, MOVING_PERIOD));
    let record = Arc::new(Host::new());
    let stop = Arc::new(AtomicBool::new(false));
    let capture = {
        let (host, record, stop, source) = (
            Arc::clone(&host),
            Arc::clone(&record),
            Arc::clone(&stop),
            Arc::clone(&source),
        );
        std::thread::spawn(move || {
            crate::serve_shared_capture(
                &*source,
                &host,
                &stop,
                false,
                0,
                |_| {},
                move |_, _, _, _, _| {
                    Ok(Encoder {
                        host: Arc::clone(&record),
                        opened: false,
                        since_keyframe: 0,
                    })
                },
            )
        })
    };
    let schedule = std::thread::spawn({
        let stream = Stream {
            duration: stream.duration,
            warmup: stream.warmup,
            bursts: stream.bursts,
            burst: stream.burst,
            burst_start: stream.burst_start.clone(),
            burst_end: stream.burst_end.clone(),
            policy: stream.policy,
            transfer: None,
        };
        move || impair(&stream, started)
    });
    // Changed pictures as they arrived, and the stills.
    let mut changed: Vec<(Instant, Instant)> = Vec::new();
    let mut stills: Vec<(Instant, usize)> = Vec::new();
    while started.elapsed() < stream.duration {
        let Some(frame) = frames.recv_timeout(Duration::from_millis(20)) else {
            continue;
        };
        let at = Instant::now();
        match frame.role {
            vnilink::FrameRole::Still => stills.push((at, frame.payload.len())),
            vnilink::FrameRole::Changed => {
                let index = picture_index(&frame.payload).expect("a sequence marker");
                if let Some(produced) = record.produced_at(index) {
                    changed.push((at, produced));
                }
            }
            vnilink::FrameRole::Unchanged => {}
        }
    }
    let end = started + stream.duration;
    let bursts = schedule.join().expect("impairment schedule");
    stop.store(true, Ordering::Release);
    capture
        .join()
        .expect("capture thread")
        .expect("shared capture");
    let hosted = host
        .diagnostic_snapshot()
        .expect("diagnostic snapshot")
        .pipelines
        .into_iter()
        .find(|pipeline| pipeline.hosted);
    let finished = json!({ "host": selected_paths(&host), "viewer": selected_paths(&viewer) });
    handle.close();

    // Each change after the warm-up: when its first picture showed, from
    // the change and from when that picture was made.
    let onsets: Vec<Value> = source
        .onsets(end)
        .into_iter()
        .filter(|onset| *onset >= started + stream.warmup)
        .map(|onset| {
            let first = changed.iter().find(|(_, produced)| *produced >= onset);
            // The still of this still period, if one arrived: when, against
            // the change, and how large. A change whose first picture
            // arrives just after it waited behind it.
            let still = stills.iter().rev().find(|(at, _)| {
                *at >= onset - STILL_PERIOD && first.is_none_or(|(shown, _)| at <= shown)
            });
            json!({
                "at_ms": millis(onset - started),
                "shown_after_ms": first.map(|(at, _)| millis(*at - onset)),
                "latency_ms": first.map(|(at, produced)| millis(*at - *produced)),
                "in_burst": bursts.iter().any(|(began, ended)| onset >= *began && onset <= *ended),
                "still_arrived_ms": still.map(|(at, _)| millis(*at - onset)),
                "still_bytes": still.map(|(_, bytes)| bytes),
            })
        })
        .collect();
    let sorted = |values: Vec<f64>| {
        let mut values = values;
        values.sort_by(f64::total_cmp);
        json!({
            "p50": percentile(&values, 0.50),
            "p95": percentile(&values, 0.95),
            "max": values.last().copied(),
        })
    };
    let first_latency: Vec<f64> = onsets
        .iter()
        .filter_map(|onset| onset["latency_ms"].as_f64())
        .collect();
    let latency: Vec<f64> = changed
        .iter()
        .map(|(at, produced)| millis(*at - *produced))
        .collect();
    json!({
        "stills": on,
        "changes": onsets.len(),
        "changes_unshown": onsets.iter().filter(|onset| onset["shown_after_ms"].is_null()).count(),
        "first_picture_latency_ms": sorted(first_latency),
        "latency_ms": sorted(latency),
        "stills_received": stills.len(),
        "still_bytes_received": stills.iter().map(|(_, bytes)| bytes).sum::<usize>(),
        // From the screen going still to its still picture arriving.
        "still_after_ms": sorted(
            stills
                .iter()
                .map(|(at, _)| {
                    let (period, _) = source.phase(*at);
                    millis(*at - (started + (STILL_PERIOD + MOVING_PERIOD) * period as u32))
                })
                .collect()
        ),
        "paths": { "connected": connected, "finished": finished },
        "host": hosted.map(|pipeline| json!({
            "stills_sent": pipeline.stills_sent,
            "still_bytes": pipeline.still_bytes,
            "stills_dropped": pipeline.stills_dropped,
            "send_dropped": pipeline.send_dropped,
            "keyframes_sent": pipeline.keyframes_sent,
        })),
        "onsets": onsets,
    })
}

/// A screen that alternates still and moving, streamed without still
/// pictures and then with them over the same link.
#[test]
#[ignore = "impaired-network harness: run through `npm run measure:impaired`"]
fn impaired_desktop_stills() {
    let stream = Stream::from_env();
    let off = run_stills(&stream, false);
    let on = run_stills(&stream, true);
    let report = json!({
        "scenario": "stills",
        "duration_s": stream.duration.as_secs_f64(),
        // The harness checks the paths of the run with still pictures.
        "paths": on["paths"].clone(),
        "off": off,
        "on": on,
    });
    let text = serde_json::to_string_pretty(&report).unwrap();
    match std::env::var_os("VNILINK_IMPAIRED_OUT") {
        Some(path) => std::fs::write(path, text).expect("result file"),
        None => eprintln!("{text}"),
    }
}

/// The stills scenario on loopback, briefly: every change shown, and every
/// still sent received. Whether a still of the dense page fits depends on
/// what the link is estimated to carry, which a lightly loaded loopback
/// sender leaves low; `an_unchanged_screen_brings_one_still_picture` shows a
/// still going end to end.
#[test]
fn stills_run_on_loopback() {
    let stream = Stream {
        duration: Duration::from_secs(9),
        warmup: Duration::from_secs(1),
        bursts: 0,
        burst: Duration::ZERO,
        burst_start: None,
        burst_end: None,
        policy: ViewerPolicy::Passive,
        transfer: None,
    };
    let on = run_stills(&stream, true);
    assert_eq!(on["changes_unshown"], 0, "{on}");
    assert_eq!(on["stills_received"], on["host"]["stills_sent"], "{on}");
    let off = run_stills(&stream, false);
    assert_eq!(off["stills_received"], 0, "{off}");
}
