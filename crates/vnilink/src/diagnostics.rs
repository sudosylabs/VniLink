//! Bounded diagnostic snapshots without sensitive payload.
//!
//! A snapshot names stages, phases, counts and queue depths so a later agent
//! can identify a failure's stage. It never carries screen pixels, clipboard
//! text, terminal content, relay tokens, identity secrets or absolute
//! personal paths. Peers appear as short fingerprints: enough to correlate
//! entries, useless for impersonation. Export is size-capped; oversized
//! lists truncate with an explicit marker instead of growing.

use serde::Serialize;

/// Snapshot format version. Bump when fields change meaning.
pub const SNAPSHOT_VERSION: u32 = 4;
/// Hard cap on exported JSON bytes.
pub const MAX_EXPORT_BYTES: usize = 64 * 1024;
/// Per-list cap before `truncated` is set.
pub const MAX_SNAPSHOT_LISTED: usize = 64;
/// Stage durations older than this read as unknown (`None`).
pub(crate) const PIPELINE_FRESH_SECS: u64 = 3;

/// First 4 bytes of a peer key as hex. Correlates entries without
/// publishing an identity that could be redialed from the export.
pub fn fingerprint(key: &[u8; 32]) -> String {
    key[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Milliseconds since process start on a monotonic clock. Snapshots carry
/// no wall time or dates.
pub fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// Final path component only. Absolute personal paths never enter a
/// snapshot; callers that only have a full path must pass it through here.
pub fn file_name_only(path: &str) -> String {
    const MAX: usize = 128;
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    let name = if name.is_empty() { "?" } else { name };
    if name.len() <= MAX {
        return name.into();
    }
    let mut end = MAX;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ConnectionSummary {
    pub id: u64,
    pub role: String,
    pub phase: String,
    pub generation: u64,
    pub peer: String,
    pub services: u32,
    /// Selected transport path: `direct`, `relay` or `none`. No addresses.
    pub path: String,
    /// Round-trip estimate of the selected path, when there is one.
    pub rtt_ms: Option<u64>,
    /// Connection totals across all open paths.
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// The selected path's congestion window, and the congestion events,
    /// lost packets and black holes this side saw on it: its sending
    /// direction only.
    pub cwnd: Option<u64>,
    pub congestion_events: Option<u64>,
    pub lost_packets: Option<u64>,
    pub black_holes_detected: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ServiceSummary {
    pub id: u64,
    pub connection_id: u64,
    pub kind: String,
    pub phase: String,
    pub hosted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ResourceCounts {
    pub incoming_connections: u32,
    pub outgoing_connections: u32,
    pub controller_services: u32,
    pub hosted_services: u32,
    pub terminal_sessions: u32,
    pub retained_operations: u32,
    pub privacy_held: bool,
    pub active_jobs: u32,
    pub paired_devices: u32,
    /// Connection id holding the input lease, if any live control exists.
    pub control_connection: Option<u64>,
}

/// One media pipeline: codec, geometry, stage durations, queue depths and
/// drops. Labels and counts only; no frame bytes or pixel data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PipelineSummary {
    pub service_id: u64,
    pub hosted: bool,
    /// Agreed codec: `h264`, `h264-high` or `unknown`.
    pub codec: String,
    pub width: u16,
    pub height: u16,
    /// Host-confirmed input state: `viewing`, `controlling`, `contended`.
    pub control: String,
    /// Last measured stage durations, fresh within [`PIPELINE_FRESH_SECS`].
    pub capture_us: Option<u64>,
    pub encode_us: Option<u64>,
    pub decode_us: Option<u64>,
    /// Produce-side (host encode) queue.
    pub out_queued: u16,
    pub out_dropped: u64,
    pub out_oldest_ms: u64,
    /// Consume-side (controller decode) queue.
    pub in_queued: u16,
    pub in_dropped: u64,
    pub in_oldest_ms: u64,
    pub keyframe_pending: bool,
    /// Frames the viewer lost on the way, by how: unfinished too long,
    /// pushed out by newer frames, skipped, or completed too late.
    pub lost_expired: u64,
    pub lost_evicted: u64,
    pub lost_gap: u64,
    pub lost_stale: u64,
    /// Keyframes the viewer asked for; keyframes the host sent, and their bytes.
    pub keyframes_requested: u64,
    pub keyframes_sent: u64,
    pub keyframe_bytes: u64,
    /// Times the host opened its encoder again for new settings.
    pub encoder_reopens: u64,
    /// Frames the host dropped rather than queue behind a slow link.
    pub send_dropped: u64,
    /// Host: still pictures sent and their bytes, and those left out while
    /// the link or the stream was busy.
    pub stills_sent: u64,
    pub still_bytes: u64,
    pub stills_dropped: u64,
    /// Host: what its viewer last reported losing, smoothed, in thousandths
    /// of the fragments sent and of the frames after any repair. None
    /// before a fresh report.
    pub viewer_fragment_loss_permille: Option<u32>,
    pub viewer_frame_loss_permille: Option<u32>,
    pub selected_display: Option<u64>,
    /// Fixed display-fault label, if the selected source failed.
    pub fault: Option<String>,
    /// Viewer: what is wrong with the live picture, from the most severe.
    pub conditions: Vec<ConditionSummary>,
    /// Why the picture stopped (`DesktopEnd::label`), once it did.
    pub end: Option<String>,
    /// Host: the encoder open for this viewer and whether it is hardware.
    pub encoder: Option<String>,
    pub encoder_hardware: Option<bool>,
    /// Viewer: whether the window decodes in hardware, once it said, and how
    /// often its decoder restarted from a keyframe.
    pub decoder_hardware: Option<bool>,
    pub decoder_recoveries: u64,
}

/// One held condition: its label, since when on [`monotonic_ms`], and the
/// signal that set it with its value and the threshold it crossed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ConditionSummary {
    pub condition: String,
    pub since_ms: u64,
    pub signal: String,
    pub value: u64,
    pub threshold: u64,
}

impl From<crate::ConditionEntry> for ConditionSummary {
    fn from(entry: crate::ConditionEntry) -> Self {
        Self {
            condition: entry.condition.label().into(),
            since_ms: entry.since_ms,
            signal: entry.evidence.signal.into(),
            value: entry.evidence.value,
            threshold: entry.evidence.threshold,
        }
    }
}

/// The last desktop picture that stopped on this computer, kept after its
/// pipeline left the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct EndSummary {
    pub service_id: u64,
    pub hosted: bool,
    /// `DesktopEnd::label`.
    pub reason: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct QueueDepths {
    /// Bounded depths only; payload bytes are never included.
    pub input_queued: u32,
    pub terminal_startups_pending: u32,
    /// Peers whose files connections this computer serves.
    pub files_serving: u32,
}

/// File transfer since this core started: bytes and files each way,
/// segment checks, and how often transfers resumed, retried, stalled or
/// were refused, by refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct TransferSummary {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub files_served: u64,
    pub files_fetched: u64,
    pub files_resumed: u64,
    pub segments_checked: u64,
    pub segments_mismatched: u64,
    pub retries: u64,
    pub stalls: u64,
    pub refused_denied: u64,
    pub refused_changed: u64,
    pub refused_busy: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct DiagnosticSnapshot {
    pub version: u32,
    /// Milliseconds on the caller's monotonic clock; no wall time, no dates.
    pub monotonic_ms: u64,
    pub hosting: bool,
    pub connections: Vec<ConnectionSummary>,
    pub services: Vec<ServiceSummary>,
    pub pipelines: Vec<PipelineSummary>,
    pub resources: ResourceCounts,
    pub queues: QueueDepths,
    pub transfers: TransferSummary,
    pub last_end: Option<EndSummary>,
    pub truncated: bool,
}

/// Facts the snapshot builder needs. The [`Core`](crate::Core) query maps
/// live state onto this; tests construct it directly.
#[derive(Debug, Clone, Default)]
pub struct SnapshotInputs {
    pub monotonic_ms: u64,
    pub hosting: bool,
    pub connections: Vec<ConnectionSummary>,
    pub services: Vec<ServiceSummary>,
    pub pipelines: Vec<PipelineSummary>,
    pub resources: ResourceCounts,
    pub queues: QueueDepths,
    pub transfers: TransferSummary,
    pub last_end: Option<EndSummary>,
}

pub fn build_snapshot(mut inputs: SnapshotInputs) -> DiagnosticSnapshot {
    let mut truncated = false;
    if inputs.connections.len() > MAX_SNAPSHOT_LISTED {
        inputs.connections.truncate(MAX_SNAPSHOT_LISTED);
        truncated = true;
    }
    if inputs.services.len() > MAX_SNAPSHOT_LISTED {
        inputs.services.truncate(MAX_SNAPSHOT_LISTED);
        truncated = true;
    }
    if inputs.pipelines.len() > MAX_SNAPSHOT_LISTED {
        inputs.pipelines.truncate(MAX_SNAPSHOT_LISTED);
        truncated = true;
    }
    DiagnosticSnapshot {
        version: SNAPSHOT_VERSION,
        monotonic_ms: inputs.monotonic_ms,
        hosting: inputs.hosting,
        connections: inputs.connections,
        services: inputs.services,
        pipelines: inputs.pipelines,
        resources: inputs.resources,
        queues: inputs.queues,
        transfers: inputs.transfers,
        last_end: inputs.last_end,
        truncated,
    }
}

/// Serializes a snapshot to compact JSON, enforcing [`MAX_EXPORT_BYTES`].
/// Oversized snapshots shed list entries (connections first, then
/// services, then pipelines) and set `truncated`; counts always describe
/// the live state, never the truncated lists.
pub fn export_json(snapshot: &DiagnosticSnapshot) -> Result<String, crate::Error> {
    let mut snapshot = snapshot.clone();
    let mut encoded = serde_json::to_string(&snapshot)
        .map_err(|_| crate::Error::InvalidSetting("snapshot does not serialize"))?;
    while encoded.len() > MAX_EXPORT_BYTES
        && (!snapshot.connections.is_empty()
            || !snapshot.services.is_empty()
            || !snapshot.pipelines.is_empty())
    {
        if !snapshot.connections.is_empty() {
            let keep = snapshot.connections.len() / 2;
            snapshot.connections.truncate(keep);
        } else if !snapshot.services.is_empty() {
            let keep = snapshot.services.len() / 2;
            snapshot.services.truncate(keep);
        } else {
            let keep = snapshot.pipelines.len() / 2;
            snapshot.pipelines.truncate(keep);
        }
        snapshot.truncated = true;
        encoded = serde_json::to_string(&snapshot)
            .map_err(|_| crate::Error::InvalidSetting("snapshot does not serialize"))?;
    }
    if encoded.len() > MAX_EXPORT_BYTES {
        return Err(crate::Error::InvalidSetting(
            "snapshot exceeds the export cap",
        ));
    }
    Ok(encoded)
}
