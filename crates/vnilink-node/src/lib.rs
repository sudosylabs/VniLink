//! Node-API control bridge. Input, session, and video-frame polling cross
//! here as JSON; callers transfer frame bytes as base64. Poll stateful video
//! subscriptions instead of holding pollers in JavaScript.
//! Management calls run on libuv workers. The separate data module uses
//! bounded, nonblocking native drains and input queues outside this mutex.
mod native;
mod storage;

use native::{
    TransferCapability, TransferNativeBackend, TransferNativeDevice, TransferPowerPolicy,
    TransferPowerStatus, TransferRecordingRetention, TransferRecordingStatus,
    TransferSharingStatus,
};
#[cfg(feature = "reset-installation")]
pub use storage::reset_installation_cli;
mod data;

use napi::{bindgen_prelude::AsyncTask, Env, Error, Result, Status, Task};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use vnilink_app::{EventPoller, FfiError, FilesService, VideoPoller, VnilinkCore};

/// Largest accepted command body. Clipboard text reaches 256 KiB and encoded
/// frames cross as base64, so this must clear a ~1 MiB frame plus JSON shape.
const COMMAND_BYTES_MAX: usize = 2 * 1024 * 1024;

/// Bridge-local transfer shapes. The application `Ffi*` records behind these
/// are not serde types, so the bridge owns the exact JSON field names.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TransferPointerSettings {
    sensitivity_percent: u16,
    reverse_wheel: bool,
    swap_buttons: bool,
}

#[derive(Deserialize, Serialize)]
struct TransferDisplay {
    id: String,
    origin_x: i32,
    origin_y: i32,
    width: u16,
    height: u16,
    rotation_deg: u16,
    scale_percent: u16,
    revision: String,
}

#[derive(Deserialize, Serialize)]
struct TransferQuality {
    max_bitrate_kbps: u32,
    max_frame_rate: u8,
    max_width: u16,
    max_height: u16,
}

#[derive(Serialize)]
struct TransferPath {
    relay: bool,
    selected: bool,
    rtt_ms: String,
    tx_bytes: String,
    rx_bytes: String,
}

#[derive(Serialize)]
struct TransferFrame {
    codec: String,
    keyframe: bool,
    timestamp_us: String,
    data: String,
    display_id: String,
    revision: String,
}

#[derive(Serialize)]
struct TransferRelay {
    url: String,
    authenticated: bool,
}

use vnilink_app::FfiVideoCodecCaps as TransferVideoCodecCaps;

use vnilink_app::FfiSharingPolicy as TransferSharingPolicy;

use vnilink_app::FfiSystemPolicy as TransferSystemPolicy;

#[derive(Serialize)]
struct TransferSystemSupport {
    lock: bool,
    restart: bool,
    shutdown: bool,
    privacy: bool,
}

#[derive(Deserialize, Serialize, Clone)]
struct TransferTerminalPolicy {
    allow_persistence: bool,
    max_sessions: u16,
    max_sessions_per_peer: u16,
    history_bytes: u32,
    replay_bytes: u32,
    detached_timeout_secs: u32,
}

#[derive(Serialize)]
struct TransferTerminalSession {
    id: String,
    process_id: u32,
    shell: String,
    rows: u16,
    columns: u16,
    persistent: bool,
    attached: bool,
}

#[derive(Serialize)]
struct TransferJob {
    id: String,
    peer: String,
    label: String,
    direction: String,
    sender: bool,
    state: String,
    total_bytes: String,
    done_bytes: String,
    files_total: String,
    files_done: String,
    resumed_files: String,
    skipped_files: String,
    skipped_symlinks: String,
    waiting_file: Option<String>,
    error: Option<String>,
}

fn transfer_job(snapshot: &vnilink_app::FfiJobSnapshot) -> TransferJob {
    fn direction_name(direction: &vnilink_app::FfiJobDirection) -> &'static str {
        match direction {
            vnilink_app::FfiJobDirection::Upload => "Upload",
            vnilink_app::FfiJobDirection::Download => "Download",
        }
    }
    fn state_name(state: &vnilink_app::FfiJobState) -> &'static str {
        match state {
            vnilink_app::FfiJobState::Preparing => "Preparing",
            vnilink_app::FfiJobState::Serving => "Serving",
            vnilink_app::FfiJobState::Transferring => "Transferring",
            vnilink_app::FfiJobState::WaitingDecision => "WaitingDecision",
            vnilink_app::FfiJobState::Interrupted => "Interrupted",
            vnilink_app::FfiJobState::Cancelled => "Cancelled",
            vnilink_app::FfiJobState::Failed => "Failed",
            vnilink_app::FfiJobState::Complete => "Complete",
        }
    }
    TransferJob {
        id: snapshot.id.to_string(),
        peer: snapshot.peer.clone(),
        label: snapshot.label.clone(),
        direction: direction_name(&snapshot.direction).into(),
        sender: snapshot.sender,
        state: state_name(&snapshot.state).into(),
        total_bytes: snapshot.total_bytes.to_string(),
        done_bytes: snapshot.done_bytes.to_string(),
        files_total: snapshot.files_total.to_string(),
        files_done: snapshot.files_done.to_string(),
        resumed_files: snapshot.resumed_files.to_string(),
        skipped_files: snapshot.skipped_files.to_string(),
        skipped_symlinks: snapshot.skipped_symlinks.to_string(),
        waiting_file: snapshot.waiting_file.clone(),
        error: snapshot.error.clone(),
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct TransferRelayInput {
    url: String,
    authenticated: bool,
    #[serde(rename = "replacementToken")]
    replacement_token: String,
}

impl From<vnilink_app::FfiTerminalPolicy> for TransferTerminalPolicy {
    fn from(policy: vnilink_app::FfiTerminalPolicy) -> Self {
        Self {
            allow_persistence: policy.allow_persistence,
            max_sessions: policy.max_sessions,
            max_sessions_per_peer: policy.max_sessions_per_peer,
            history_bytes: policy.history_bytes,
            replay_bytes: policy.replay_bytes,
            detached_timeout_secs: policy.detached_timeout_secs,
        }
    }
}

impl From<TransferTerminalPolicy> for vnilink_app::FfiTerminalPolicy {
    fn from(policy: TransferTerminalPolicy) -> Self {
        Self {
            allow_persistence: policy.allow_persistence,
            max_sessions: policy.max_sessions,
            max_sessions_per_peer: policy.max_sessions_per_peer,
            history_bytes: policy.history_bytes,
            replay_bytes: policy.replay_bytes,
            detached_timeout_secs: policy.detached_timeout_secs,
        }
    }
}

/// Transfer shapes for core events. Integers cross as decimal strings like
/// every other bridge record; enum variants keep their core names.
struct EventTransfer;

impl EventTransfer {
    fn service_kind(kind: &vnilink_app::FfiServiceKind) -> &'static str {
        match kind {
            vnilink_app::FfiServiceKind::Desktop => "Desktop",
            vnilink_app::FfiServiceKind::Files => "Files",
            vnilink_app::FfiServiceKind::Terminal => "Terminal",
            vnilink_app::FfiServiceKind::Camera => "Camera",
            vnilink_app::FfiServiceKind::SystemAudio => "SystemAudio",
            vnilink_app::FfiServiceKind::Microphone => "Microphone",
            vnilink_app::FfiServiceKind::System => "System",
        }
    }

    fn close_cause(cause: &vnilink_app::FfiCloseCause) -> &'static str {
        match cause {
            vnilink_app::FfiCloseCause::OwnerClose => "OwnerClose",
            vnilink_app::FfiCloseCause::ParentLoss => "ParentLoss",
            vnilink_app::FfiCloseCause::Revoked => "Revoked",
            vnilink_app::FfiCloseCause::Replaced => "Replaced",
            vnilink_app::FfiCloseCause::DeviceLoss => "DeviceLoss",
            vnilink_app::FfiCloseCause::Shutdown => "Shutdown",
            vnilink_app::FfiCloseCause::PeerClose => "PeerClose",
            vnilink_app::FfiCloseCause::IdleExpired => "IdleExpired",
        }
    }

    fn service_phase(phase: &vnilink_app::FfiServicePhase) -> serde_json::Value {
        match phase {
            vnilink_app::FfiServicePhase::Waiting => serde_json::Value::String("Waiting".into()),
            vnilink_app::FfiServicePhase::Active => serde_json::Value::String("Active".into()),
            vnilink_app::FfiServicePhase::Suspended => {
                serde_json::Value::String("Suspended".into())
            }
            vnilink_app::FfiServicePhase::Closed { cause } => {
                serde_json::json!({ "Closed": { "cause": Self::close_cause(cause) } })
            }
            vnilink_app::FfiServicePhase::Failed { message } => {
                serde_json::json!({ "Failed": { "message": message } })
            }
        }
    }

    fn service(service: &vnilink_app::FfiServiceInfo) -> serde_json::Value {
        serde_json::json!({
            "id": service.id.to_string(),
            "connectionId": service.connection_id.to_string(),
            "generation": service.generation.to_string(),
            "peer_id": service.peer_id,
            "kind": Self::service_kind(&service.kind),
            "phase": Self::service_phase(&service.phase),
        })
    }

    fn session_phase(phase: &vnilink_app::FfiSessionPhase) -> serde_json::Value {
        match phase {
            vnilink_app::FfiSessionPhase::Connecting => {
                serde_json::Value::String("Connecting".into())
            }
            vnilink_app::FfiSessionPhase::Authenticated => {
                serde_json::Value::String("Authenticated".into())
            }
            vnilink_app::FfiSessionPhase::Authorized => {
                serde_json::Value::String("Authorized".into())
            }
            vnilink_app::FfiSessionPhase::Negotiating => {
                serde_json::Value::String("Negotiating".into())
            }
            vnilink_app::FfiSessionPhase::Active => serde_json::Value::String("Active".into()),
            vnilink_app::FfiSessionPhase::Reconnecting => {
                serde_json::Value::String("Reconnecting".into())
            }
            vnilink_app::FfiSessionPhase::Stopping => serde_json::Value::String("Stopping".into()),
            vnilink_app::FfiSessionPhase::Closed => serde_json::Value::String("Closed".into()),
            vnilink_app::FfiSessionPhase::Failed { message } => {
                serde_json::json!({ "Failed": { "message": message } })
            }
        }
    }

    fn event(event: &vnilink_app::FfiEvent) -> serde_json::Value {
        match event {
            vnilink_app::FfiEvent::HostingChanged { hosting } => {
                serde_json::json!({ "type": "hostingChanged", "hosting": hosting })
            }
            vnilink_app::FfiEvent::DevicePaired { endpoint_id } => {
                serde_json::json!({ "type": "devicePaired", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::DeviceAuthorized { endpoint_id } => {
                serde_json::json!({ "type": "deviceAuthorized", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::DeviceRevoked { endpoint_id } => {
                serde_json::json!({ "type": "deviceRevoked", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::DeviceUnpaired { endpoint_id } => {
                serde_json::json!({ "type": "deviceUnpaired", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::ControlRequested { endpoint_id, check } => {
                serde_json::json!({ "type": "controlRequested", "endpoint_id": endpoint_id, "check": check })
            }
            vnilink_app::FfiEvent::AddressesUpdated { endpoint_id } => {
                serde_json::json!({ "type": "addressesUpdated", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::PeerRenamed {
                endpoint_id,
                hostname,
            } => {
                serde_json::json!({ "type": "peerRenamed", "endpoint_id": endpoint_id, "hostname": hostname })
            }
            vnilink_app::FfiEvent::SessionChanged {
                id,
                generation,
                role,
                phase,
                peer_id,
            } => {
                let role = match role {
                    vnilink_app::FfiSessionRole::Controller => "Controller",
                    vnilink_app::FfiSessionRole::Host => "Host",
                };
                serde_json::json!({
                    "type": "sessionChanged",
                    "id": id.to_string(),
                    "generation": generation.to_string(),
                    "role": role,
                    "phase": Self::session_phase(phase),
                    "peer_id": peer_id,
                })
            }
            vnilink_app::FfiEvent::ServiceChanged { service } => {
                serde_json::json!({ "type": "serviceChanged", "service": Self::service(service) })
            }
            vnilink_app::FfiEvent::HostedServiceChanged { service } => {
                serde_json::json!({ "type": "hostedServiceChanged", "service": Self::service(service) })
            }
            vnilink_app::FfiEvent::QualityUpdated { id, quality } => {
                serde_json::json!({
                    "type": "qualityUpdated",
                    "id": id.to_string(),
                    "quality": {
                        "max_bitrate_kbps": quality.max_bitrate_kbps,
                        "max_frame_rate": quality.max_frame_rate,
                        "max_width": quality.max_width,
                        "max_height": quality.max_height,
                    },
                })
            }
            vnilink_app::FfiEvent::InputFailed { id, message } => {
                serde_json::json!({ "type": "inputFailed", "id": id.to_string(), "message": message })
            }
            vnilink_app::FfiEvent::KeyframeRequested { id } => {
                serde_json::json!({ "type": "keyframeRequested", "id": id.to_string() })
            }
            vnilink_app::FfiEvent::ClipboardReceived { id } => {
                serde_json::json!({ "type": "clipboardReceived", "id": id.to_string() })
            }
            vnilink_app::FfiEvent::ClipboardFilesReceived { id } => {
                serde_json::json!({ "type": "clipboardFilesReceived", "id": id.to_string() })
            }
            vnilink_app::FfiEvent::DesktopClipboardReceived { service } => {
                serde_json::json!({ "type": "desktopClipboardReceived", "service": Self::service(service) })
            }
            vnilink_app::FfiEvent::TopologyChanged { revision } => {
                serde_json::json!({ "type": "topologyChanged", "revision": revision.to_string() })
            }
            vnilink_app::FfiEvent::HostLocked { locked } => {
                serde_json::json!({ "type": "hostLocked", "locked": locked })
            }
        }
    }
}

#[derive(Serialize)]
struct TransferControlRequest {
    endpoint_id: String,
    check: String,
}

fn pointer_button(name: &str) -> Result<vnilink_app::FfiPointerButton> {
    match name {
        "Left" => Ok(vnilink_app::FfiPointerButton::Left),
        "Right" => Ok(vnilink_app::FfiPointerButton::Right),
        "Middle" => Ok(vnilink_app::FfiPointerButton::Middle),
        "Back" => Ok(vnilink_app::FfiPointerButton::Back),
        "Forward" => Ok(vnilink_app::FfiPointerButton::Forward),
        _ => Err(Error::new(Status::InvalidArg, "Invalid pointer button")),
    }
}

fn keyboard_mode(name: &str) -> Result<vnilink_app::FfiKeyboardMode> {
    match name {
        "Character" => Ok(vnilink_app::FfiKeyboardMode::Character),
        "Map" => Ok(vnilink_app::FfiKeyboardMode::Map),
        "Translate" => Ok(vnilink_app::FfiKeyboardMode::Translate),
        _ => Err(Error::new(Status::InvalidArg, "Invalid keyboard mode")),
    }
}

fn keyboard_mode_name(mode: vnilink_app::FfiKeyboardMode) -> &'static str {
    match mode {
        vnilink_app::FfiKeyboardMode::Character => "Character",
        vnilink_app::FfiKeyboardMode::Map => "Map",
        vnilink_app::FfiKeyboardMode::Translate => "Translate",
    }
}

fn session_role(name: &str) -> Result<vnilink_app::FfiSessionRole> {
    match name {
        "Controller" => Ok(vnilink_app::FfiSessionRole::Controller),
        "Host" => Ok(vnilink_app::FfiSessionRole::Host),
        _ => Err(Error::new(Status::InvalidArg, "Invalid session role")),
    }
}

fn parse_id(value: &str, what: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| Error::new(Status::InvalidArg, what))
}

fn session_target(session_id: &str, generation: &str) -> Result<(u64, u64)> {
    Ok((
        parse_id(session_id, "Invalid session ID")?,
        parse_id(generation, "Invalid session generation")?,
    ))
}

fn transfer_display(display: &vnilink_app::FfiDisplay) -> TransferDisplay {
    TransferDisplay {
        id: display.id.to_string(),
        origin_x: display.origin_x,
        origin_y: display.origin_y,
        width: display.width,
        height: display.height,
        rotation_deg: display.rotation_deg,
        scale_percent: display.scale_percent,
        revision: display.revision.to_string(),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Command {
    BeginConnectServices {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Snapshot,
    Endpoint,
    Capabilities,
    VideoCodecs,
    SetVideoCodecs {
        codecs: TransferVideoCodecCaps,
    },
    SaveCustomRelays {
        relays: Vec<TransferRelayInput>,
    },
    PeerHostname {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    ClipboardOwnerSession,
    SessionKeyboardMode {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    SetSessionKeyboardMode {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        mode: String,
    },
    SessionSwapControlAndCommand {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    SetSessionSwapControlAndCommand {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        enabled: bool,
    },
    ControllerSessionPaths {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SubscribeEvents,
    PollEvents {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
        #[serde(rename = "maxEvents")]
        max_events: u32,
    },
    ReleaseEvents {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    SubscribeVideo,
    SharingPolicy,
    SetSharingPolicy {
        policy: vnilink_app::FfiSharingPolicyUpdate,
    },
    SystemPolicy,
    SetSystemPolicy {
        policy: TransferSystemPolicy,
    },
    TerminalPolicy,
    SetTerminalPolicy {
        policy: TransferTerminalPolicy,
    },
    EnableNativeTerminal {
        program: String,
        directory: String,
    },
    EnableDefaultNativeTerminal,
    DisableTerminal,
    RemoteTerminalSessions {
        #[serde(rename = "connectionId")]
        connection_id: String,
        generation: String,
    },
    CloseRemoteTerminalSession {
        #[serde(rename = "connectionId")]
        connection_id: String,
        generation: String,
        id: String,
    },
    HostedTerminalSessions,
    CloseTerminalSession {
        id: String,
    },
    SubscribeFilesJobs {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    PollFilesJobs {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    ReleaseFilesJobs {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    FilesJobCancel {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
        #[serde(rename = "jobId")]
        job_id: String,
    },
    FilesJobResume {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
        #[serde(rename = "jobId")]
        job_id: String,
    },
    CreateInvitation {
        #[serde(rename = "ttlSeconds")]
        ttl_seconds: u32,
    },
    AcceptInvitation {
        token: String,
    },
    ExportPairingFile {
        #[serde(rename = "ttlSeconds")]
        ttl_seconds: u32,
        #[serde(rename = "allowControl")]
        allow_control: bool,
    },
    OpenPairingFile {
        bytes: Vec<u8>,
    },
    #[serde(rename = "openPairingFileBase64")]
    OpenPairingFileBase64 {
        data: String,
    },
    ConfirmPairing {
        #[serde(rename = "previewId")]
        preview_id: String,
        digits: String,
    },
    Revoke {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Unpair {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    SetPeerName {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
        name: String,
    },
    Connect {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    ConnectFiles {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    BeginConnectFiles {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    BeginConnect {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Disconnect,
    DisconnectSession {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SubmitKey {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        usage: u16,
        down: bool,
    },
    SubmitPointer {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        x: i32,
        y: i32,
        display: String,
        revision: String,
    },
    SubmitButton {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        button: String,
        down: bool,
    },
    SubmitWheel {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        dx: i16,
        dy: i16,
    },
    SubmitMotion {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        dx: i16,
        dy: i16,
    },
    SubmitPreciseWheel {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        dx: i32,
        dy: i32,
    },
    SubmitText {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        text: String,
    },
    SubmitClipboard {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        text: String,
    },
    SubmitLocalKey {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        usage: u16,
        down: bool,
        text: String,
    },
    ReleaseSessionInput {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    StartNativeScreenSharing {
        #[serde(rename = "displayId")]
        display_id: u32,
        width: u16,
        height: u16,
        #[serde(rename = "pixelWidth")]
        pixel_width: u16,
        #[serde(rename = "pixelHeight")]
        pixel_height: u16,
    },
    StopNativeScreenSharing,
    NativeSharingStatus,
    StartRecording {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        destination: String,
        #[serde(rename = "maxBytes")]
        max_bytes: String,
        #[serde(rename = "maxDurationMs")]
        max_duration_ms: String,
        retention: Option<TransferRecordingRetention>,
    },
    RecordingStatus {
        id: String,
    },
    StopRecording {
        id: String,
    },
    ReleaseRecording {
        id: String,
    },
    RemoteCameraDevices {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    RemoteAudioDevices {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        kind: String,
    },
    NativeCameraDevices,
    NativeAudioDevices,
    NativeAudioOutputs,
    SetNativeCameraSources {
        ids: Vec<String>,
    },
    SetNativeAudioSources {
        microphones: Vec<String>,
        system: Vec<String>,
    },
    PowerPolicy,
    SetPowerPolicy {
        policy: TransferPowerPolicy,
    },
    PowerStatus,
    NativeRuntime,
    EnableNativeInput,
    ReportNativePermissions {
        screen: bool,
        input: bool,
        camera: bool,
        microphone: bool,
    },
    SetInputDefaults {
        mode: String,
        swap: bool,
        pointer: TransferPointerSettings,
    },
    EnableFilesystemAccess,
    SetFileRoots {
        paths: Vec<String>,
    },
    KeyboardMode,
    SetKeyboardMode {
        mode: String,
    },
    SwapControlAndCommand,
    SetSwapControlAndCommand {
        enabled: bool,
    },
    SessionPointerSettings {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    SetSessionPointerSettings {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        settings: TransferPointerSettings,
    },
    SessionClipboardText {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    ClipboardText,
    StartHosting,
    StopHosting,
    StopHostedSession,
    ResetIdentity,
    SetRelayPublic,
    SetRelayCustom {
        urls: Vec<String>,
    },
    SavedRelays,
    RelayRestartRequired,
    ControlCheck,
    ControlRequest,
    ConfirmControlRequest {
        digits: String,
    },
    RejectControlRequest,
    ControllerSessions,
    HostedSessions,
    ControllerSession {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    ForgetControllerSession {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    SessionPaths {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    Topology,
    SessionTopology {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    SelectDisplays {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        ids: Vec<String>,
    },
    CaptureDisplays,
    SetTopology {
        revision: String,
        displays: Vec<TransferDisplay>,
    },
    SetDesktopCodecDefault {
        codec: String,
    },
    SetDesktopQualityDefaults {
        quality: TransferQuality,
    },
    UpdateSessionQuality {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        role: String,
        quality: TransferQuality,
    },
    SubscribeSessionVideo {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    PollVideo {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    ReleaseVideo {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    PublishVideo {
        #[serde(deserialize_with = "frame_codec")]
        codec: vnilink_app::FfiVideoCodec,
        #[serde(rename = "sessionId")]
        session_id: String,
        keyframe: bool,
        #[serde(rename = "timestampUs")]
        timestamp_us: String,
        data: String,
    },
    Shutdown,
}

fn frame_codec<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<vnilink_app::FfiVideoCodec, D::Error> {
    use serde::de::Error;
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "h264" => Ok(vnilink_app::FfiVideoCodec::H264),
        "h265" => Ok(vnilink_app::FfiVideoCodec::H265),
        "vp8" => Ok(vnilink_app::FfiVideoCodec::Vp8),
        "vp9" => Ok(vnilink_app::FfiVideoCodec::Vp9),
        "av1" => Ok(vnilink_app::FfiVideoCodec::Av1),
        _ => Err(D::Error::custom("unsupported video codec")),
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Reply {
    RestartRequired {
        required: bool,
    },
    Snapshot {
        snapshot: vnilink_app::FfiSnapshot,
    },
    Invitation {
        token: String,
    },
    PairingFile {
        file: vnilink_app::FfiPairingFile,
    },
    PairingPreview {
        preview: vnilink_app::FfiPairingPreview,
    },
    Device {
        device: vnilink_app::FfiDeviceInfo,
    },
    Session {
        session: vnilink_app::FfiSessionInfo,
    },
    Sessions {
        sessions: Vec<vnilink_app::FfiSessionInfo>,
    },
    Text {
        text: Option<String>,
    },
    Check {
        check: String,
    },
    ControlRequest {
        request: Option<TransferControlRequest>,
    },
    KeyboardMode {
        mode: String,
    },
    Swap {
        enabled: bool,
    },
    PointerSettings {
        settings: TransferPointerSettings,
    },
    Displays {
        displays: Vec<TransferDisplay>,
    },
    DisplayIds {
        displays: Vec<String>,
    },
    Paths {
        paths: Vec<TransferPath>,
    },
    Relays {
        relays: Vec<TransferRelay>,
    },
    VideoSubscription {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    VideoFrame {
        frame: Option<TransferFrame>,
    },
    Endpoint {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Capabilities {
        capabilities: vnilink_app::FfiCapabilities,
    },
    VideoCodecs {
        codecs: TransferVideoCodecCaps,
    },
    Hostname {
        hostname: Option<String>,
    },
    OwnerSession {
        session: Option<vnilink_app::FfiSessionInfo>,
    },
    EventSubscription {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    Events {
        events: Vec<serde_json::Value>,
        lagged: String,
    },
    SharingPolicy {
        policy: TransferSharingPolicy,
    },
    SystemPolicy {
        policy: TransferSystemPolicy,
        support: TransferSystemSupport,
    },
    TerminalPolicy {
        policy: TransferTerminalPolicy,
    },
    TerminalSessions {
        sessions: Vec<TransferTerminalSession>,
    },
    Jobs {
        jobs: Vec<TransferJob>,
    },
    Job {
        job: TransferJob,
    },
    FileJobsSubscription {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    NativeSharingStatus {
        displays: Vec<TransferSharingStatus>,
    },
    RecordingStatus {
        status: TransferRecordingStatus,
    },
    NativeDevices {
        devices: Vec<TransferNativeDevice>,
    },
    PowerPolicy {
        policy: TransferPowerPolicy,
    },
    PowerStatus {
        statuses: Vec<TransferPowerStatus>,
    },
    NativeRuntime {
        backends: Vec<TransferNativeBackend>,
        capabilities: Vec<TransferCapability>,
    },
    Done,
}

// Commands serialize state mutations and installation saves. Shutdown marks the
// instance closed; no later task can restart or mutate the old installation.
// Video pollers stay here behind subscription ids because pollers cannot
// cross the JSON bridge; callers poll and release them by id.
struct State {
    core: Arc<VnilinkCore>,
    closed: bool,
    native: native::NativeRuntimeState,
    video: HashMap<u64, VideoPoller>,
    events: HashMap<u64, EventPoller>,
    files: HashMap<u64, Arc<FilesService>>,
    next_subscription: u64,
}
#[napi]
pub struct Engine {
    core: Arc<VnilinkCore>,
    desktops: Arc<data::DesktopChannels>,
    state: Arc<Mutex<State>>,
}

pub struct OpenedEngine {
    core: VnilinkCore,
    backends: Vec<TransferNativeBackend>,
}
pub struct OpenTask {
    temporary: bool,
}
impl Task for OpenTask {
    type Output = OpenedEngine;
    type JsValue = Engine;
    fn compute(&mut self) -> Result<Self::Output> {
        let result = if self.temporary {
            VnilinkCore::create(true, vec![])
        } else {
            VnilinkCore::open_persistent(Box::new(
                storage::EncryptedInstallationStore::open_production().map_err(core_error)?,
            ))
        };
        let core = result.map_err(core_error)?;
        let backends = if self.temporary {
            vec![]
        } else {
            core.initialize_native_backends()
                .into_iter()
                .map(Into::into)
                .collect()
        };
        core.start_file_job_driver();
        Ok(OpenedEngine { core, backends })
    }
    fn resolve(&mut self, _: Env, opened: Self::Output) -> Result<Engine> {
        let core = Arc::new(opened.core);
        let desktops = Arc::new(data::DesktopChannels::default());
        Ok(Engine {
            core: core.clone(),
            desktops: desktops.clone(),
            state: Arc::new(Mutex::new(State {
                core,
                closed: false,
                native: native::NativeRuntimeState::new(opened.backends, desktops),
                video: HashMap::new(),
                events: HashMap::new(),
                files: HashMap::new(),
                next_subscription: 1,
            })),
        })
    }
}
#[napi]
pub fn open_engine() -> AsyncTask<OpenTask> {
    AsyncTask::new(OpenTask { temporary: false })
}
// Explicit isolated test/preview mode; never a fallback for unavailable storage.
#[napi]
pub fn open_temporary_engine() -> AsyncTask<OpenTask> {
    AsyncTask::new(OpenTask { temporary: true })
}

impl EngineBridge {
    /// Commands that describe displays, tune quality, and move video frames.
    fn media(state: &mut State, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::Topology => Reply::Displays {
                displays: state
                    .core
                    .topology()
                    .map_err(core_error)?
                    .iter()
                    .map(transfer_display)
                    .collect(),
            },
            Command::SessionTopology {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                Reply::Displays {
                    displays: state
                        .core
                        .session_topology(id, gen)
                        .map_err(core_error)?
                        .iter()
                        .map(transfer_display)
                        .collect(),
                }
            }
            Command::SelectDisplays {
                session_id,
                generation,
                ids,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                let selected = ids
                    .iter()
                    .map(|value| parse_id(value, "Invalid display ID"))
                    .collect::<Result<Vec<_>>>()?;
                state
                    .core
                    .select_displays(id, gen, selected)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::CaptureDisplays => Reply::DisplayIds {
                displays: state
                    .core
                    .capture_displays()
                    .map_err(core_error)?
                    .into_iter()
                    .map(|display| display.to_string())
                    .collect(),
            },
            Command::SetTopology { revision, displays } => {
                let revision = parse_id(revision, "Invalid topology revision")?;
                let displays = displays
                    .iter()
                    .map(|display| {
                        Ok::<_, Error>(vnilink_app::FfiDisplay {
                            id: parse_id(&display.id, "Invalid display ID")?,
                            origin_x: display.origin_x,
                            origin_y: display.origin_y,
                            width: display.width,
                            height: display.height,
                            rotation_deg: display.rotation_deg,
                            scale_percent: display.scale_percent,
                            revision: parse_id(&display.revision, "Invalid topology revision")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                state
                    .core
                    .set_topology(revision, displays)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SetDesktopCodecDefault { codec } => {
                use vnilink_app::FfiCodecPreference as C;
                let preference = match codec.as_str() {
                    "auto" => C::Auto,
                    "h264" => C::H264,
                    "h265" => C::H265,
                    "vp8" => C::Vp8,
                    "vp9" => C::Vp9,
                    "av1" => C::Av1,
                    _ => {
                        return Err(core_error(FfiError::InvalidSetting {
                            message: "invalid codec preference".into(),
                        }))
                    }
                };
                state
                    .core
                    .set_desktop_codec_default(preference)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SetDesktopQualityDefaults { quality } => {
                let quality = vnilink_app::new_quality(
                    quality.max_bitrate_kbps,
                    quality.max_frame_rate,
                    quality.max_width,
                    quality.max_height,
                )
                .map_err(core_error)?;
                state
                    .core
                    .set_desktop_quality_defaults(quality)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::UpdateSessionQuality {
                session_id,
                generation,
                role,
                quality,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                let quality = vnilink_app::new_quality(
                    quality.max_bitrate_kbps,
                    quality.max_frame_rate,
                    quality.max_width,
                    quality.max_height,
                )
                .map_err(core_error)?;
                state
                    .core
                    .update_session_quality_for_generation(id, gen, session_role(role)?, quality)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubscribeSessionVideo {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                let poller = state
                    .core
                    .subscribe_session_video(id, gen)
                    .map_err(core_error)?;
                let subscription = state.next_subscription;
                state.next_subscription = subscription.saturating_add(1);
                state.video.insert(subscription, poller);
                Reply::VideoSubscription {
                    subscription_id: subscription.to_string(),
                }
            }
            Command::SubscribeVideo => {
                let poller = state.core.subscribe_video().map_err(core_error)?;
                let subscription = state.next_subscription;
                state.next_subscription = subscription.saturating_add(1);
                state.video.insert(subscription, poller);
                Reply::VideoSubscription {
                    subscription_id: subscription.to_string(),
                }
            }
            Command::PollVideo { subscription_id } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                let poller = state
                    .video
                    .get(&subscription)
                    .ok_or_else(|| Error::new(Status::InvalidArg, "Unknown video subscription"))?;
                Reply::VideoFrame {
                    frame: poller.try_recv().map(|frame| {
                        use base64::Engine as _;
                        TransferFrame {
                            codec: frame.codec.label().into(),
                            keyframe: frame.keyframe,
                            timestamp_us: frame.timestamp_us.to_string(),
                            data: base64::engine::general_purpose::STANDARD.encode(&frame.payload),
                            display_id: frame.display_id.to_string(),
                            revision: frame.revision.to_string(),
                        }
                    }),
                }
            }
            Command::ReleaseVideo { subscription_id } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                state.video.remove(&subscription);
                Reply::Done
            }
            Command::PublishVideo {
                codec,
                session_id,
                keyframe,
                timestamp_us,
                data,
            } => {
                use base64::Engine as _;
                let id = parse_id(session_id, "Invalid session ID")?;
                let timestamp = parse_id(timestamp_us, "Invalid timestamp")?;
                let payload = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid video data"))?;
                state
                    .core
                    .publish_video_frame(
                        id,
                        vnilink_app::FfiEncodedFrame::new(*codec, *keyframe, timestamp, payload)
                            .map_err(|_| Error::new(Status::InvalidArg, "Invalid video frame"))?,
                    )
                    .map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that host, advertise relays, and reset the installation.
    fn hosting(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::StartHosting => {
                core.start_hosting().map_err(core_error)?;
                Reply::Done
            }
            Command::StopHosting => {
                core.stop_hosting().map_err(core_error)?;
                Reply::Done
            }
            Command::StopHostedSession => {
                core.stop_hosted_session().map_err(core_error)?;
                Reply::Done
            }
            Command::ResetIdentity => {
                core.reset_identity().map_err(core_error)?;
                Reply::Done
            }
            Command::SetRelayPublic => {
                core.set_relay_public().map_err(core_error)?;
                Reply::Done
            }
            Command::SetRelayCustom { urls } => {
                core.set_relay_custom(urls.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::SaveCustomRelays { relays } => {
                let inputs = relays
                    .iter()
                    .map(|relay| vnilink_app::FfiRelayInput {
                        url: relay.url.clone(),
                        authenticated: relay.authenticated,
                        replacement_token: relay.replacement_token.clone(),
                    })
                    .collect();
                core.save_custom_relays(inputs).map_err(core_error)?;
                Reply::Done
            }
            Command::RelayRestartRequired => Reply::RestartRequired {
                required: core.relay_restart_required().map_err(core_error)?,
            },
            Command::SavedRelays => Reply::Relays {
                relays: core
                    .saved_relays()
                    .map_err(core_error)?
                    .into_iter()
                    .map(|relay| TransferRelay {
                        url: relay.url,
                        authenticated: relay.authenticated,
                    })
                    .collect(),
            },
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that inject input and read per-session input state.
    fn input(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::SessionKeyboardMode {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                Reply::KeyboardMode {
                    mode: keyboard_mode_name(
                        core.session_keyboard_mode(id, gen).map_err(core_error)?,
                    )
                    .to_string(),
                }
            }
            Command::SetSessionKeyboardMode {
                session_id,
                generation,
                mode,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.set_session_keyboard_mode(id, gen, keyboard_mode(mode)?)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SessionSwapControlAndCommand {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                Reply::Swap {
                    enabled: core
                        .session_swap_control_and_command(id, gen)
                        .map_err(core_error)?,
                }
            }
            Command::SetSessionSwapControlAndCommand {
                session_id,
                generation,
                enabled,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.set_session_swap_control_and_command(id, gen, *enabled)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitKey {
                session_id,
                generation,
                usage,
                down,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_key(id, gen, *usage, *down)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitPointer {
                session_id,
                generation,
                x,
                y,
                display,
                revision,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_pointer(
                    id,
                    gen,
                    *x,
                    *y,
                    parse_id(display, "Invalid display ID")?,
                    parse_id(revision, "Invalid topology revision")?,
                )
                .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitButton {
                session_id,
                generation,
                button,
                down,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_button(id, gen, pointer_button(button)?, *down)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitWheel {
                session_id,
                generation,
                dx,
                dy,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_wheel(id, gen, *dx, *dy).map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitMotion {
                session_id,
                generation,
                dx,
                dy,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_motion(id, gen, *dx, *dy).map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitPreciseWheel {
                session_id,
                generation,
                dx,
                dy,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_precise_wheel(id, gen, *dx, *dy)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitText {
                session_id,
                generation,
                text,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_text(id, gen, text.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitClipboard {
                session_id,
                generation,
                text,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_clipboard(id, gen, text.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SubmitLocalKey {
                session_id,
                generation,
                usage,
                down,
                text,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.submit_local_key(id, gen, *usage, *down, text.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::ReleaseSessionInput {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.release_session_input(id, gen).map_err(core_error)?;
                Reply::Done
            }
            Command::SetInputDefaults {
                mode,
                swap,
                pointer,
            } => {
                core.set_input_defaults(
                    keyboard_mode(mode)?,
                    *swap,
                    vnilink_app::FfiPointerSettings {
                        sensitivity_percent: pointer.sensitivity_percent,
                        reverse_wheel: pointer.reverse_wheel,
                        swap_buttons: pointer.swap_buttons,
                    },
                )
                .map_err(core_error)?;
                Reply::Done
            }
            Command::EnableFilesystemAccess => {
                core.enable_filesystem_access().map_err(core_error)?;
                Reply::Done
            }
            Command::SetFileRoots { paths } => {
                core.set_file_roots(paths.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::KeyboardMode => Reply::KeyboardMode {
                mode: keyboard_mode_name(core.keyboard_mode().map_err(core_error)?).to_string(),
            },
            Command::SetKeyboardMode { mode } => {
                core.set_keyboard_mode(keyboard_mode(mode)?)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SwapControlAndCommand => Reply::Swap {
                enabled: core.swap_control_and_command().map_err(core_error)?,
            },
            Command::SetSwapControlAndCommand { enabled } => {
                core.set_swap_control_and_command(*enabled)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SessionPointerSettings {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                let settings = core.session_pointer_settings(id, gen).map_err(core_error)?;
                Reply::PointerSettings {
                    settings: TransferPointerSettings {
                        sensitivity_percent: settings.sensitivity_percent,
                        reverse_wheel: settings.reverse_wheel,
                        swap_buttons: settings.swap_buttons,
                    },
                }
            }
            Command::SetSessionPointerSettings {
                session_id,
                generation,
                settings,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                core.set_session_pointer_settings(
                    id,
                    gen,
                    vnilink_app::FfiPointerSettings {
                        sensitivity_percent: settings.sensitivity_percent,
                        reverse_wheel: settings.reverse_wheel,
                        swap_buttons: settings.swap_buttons,
                    },
                )
                .map_err(core_error)?;
                Reply::Done
            }
            Command::SessionClipboardText {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                Reply::Text {
                    text: core.session_clipboard_text(id, gen).map_err(core_error)?,
                }
            }
            Command::ClipboardText => Reply::Text {
                text: core.clipboard_text().map_err(core_error)?,
            },
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that dial, list, and tear down sessions.
    fn sessions(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::BeginConnectServices { endpoint_id } => Reply::Session {
                session: core
                    .begin_connect_services(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            Command::Connect { endpoint_id } => Reply::Session {
                session: core.connect(endpoint_id.clone()).map_err(core_error)?,
            },
            Command::ConnectFiles { endpoint_id } => Reply::Session {
                session: core
                    .connect_files(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            Command::BeginConnectFiles { endpoint_id } => Reply::Session {
                session: core
                    .begin_connect_files(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            Command::BeginConnect { endpoint_id } => Reply::Session {
                session: core
                    .begin_connect(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            Command::Disconnect => {
                core.disconnect().map_err(core_error)?;
                Reply::Done
            }
            Command::DisconnectSession { session_id } => {
                let id = parse_id(session_id, "Invalid session ID")?;
                core.disconnect_session(id).map_err(core_error)?;
                Reply::Done
            }
            Command::ControllerSessions => Reply::Sessions {
                sessions: core.controller_sessions().map_err(core_error)?,
            },
            Command::HostedSessions => Reply::Sessions {
                sessions: core.hosted_sessions().map_err(core_error)?,
            },
            Command::ControllerSession { session_id } => Reply::Session {
                session: core
                    .controller_session(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?,
            },
            Command::ForgetControllerSession { session_id } => {
                core.forget_controller_session(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::SessionPaths { session_id } => Reply::Paths {
                paths: core
                    .session_paths(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?
                    .into_iter()
                    .map(|path| TransferPath {
                        relay: path.relay,
                        selected: path.selected,
                        rtt_ms: path.rtt_ms.to_string(),
                        tx_bytes: path.tx_bytes.to_string(),
                        rx_bytes: path.rx_bytes.to_string(),
                    })
                    .collect(),
            },
            Command::ControllerSessionPaths { session_id } => Reply::Paths {
                paths: core
                    .controller_session_paths(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?
                    .into_iter()
                    .map(|path| TransferPath {
                        relay: path.relay,
                        selected: path.selected,
                        rtt_ms: path.rtt_ms.to_string(),
                        tx_bytes: path.tx_bytes.to_string(),
                        rx_bytes: path.rx_bytes.to_string(),
                    })
                    .collect(),
            },
            Command::ClipboardOwnerSession => Reply::OwnerSession {
                session: core.clipboard_owner_session().map_err(core_error)?,
            },
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that pair devices and manage the device list.
    fn pairing(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::CreateInvitation { ttl_seconds } => Reply::Invitation {
                token: core
                    .create_invitation(u64::from(*ttl_seconds))
                    .map_err(core_error)?,
            },
            Command::AcceptInvitation { token } => Reply::Device {
                device: core.accept_invitation(token.clone()).map_err(core_error)?,
            },
            Command::ExportPairingFile {
                ttl_seconds,
                allow_control,
            } => Reply::PairingFile {
                file: core
                    .export_pairing_file(u64::from(*ttl_seconds), *allow_control)
                    .map_err(core_error)?,
            },
            Command::OpenPairingFile { bytes } => Reply::PairingPreview {
                preview: core.open_pairing_file(bytes.clone()).map_err(core_error)?,
            },
            Command::OpenPairingFileBase64 { data } => {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid pairing file data"))?;
                Reply::PairingPreview {
                    preview: core.open_pairing_file(bytes).map_err(core_error)?,
                }
            }
            Command::ConfirmPairing { preview_id, digits } => {
                let id = preview_id
                    .parse::<u64>()
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid preview ID"))?;
                Reply::Device {
                    device: core
                        .confirm_pairing(id, digits.clone())
                        .map_err(core_error)?,
                }
            }
            Command::Revoke { endpoint_id } => {
                core.revoke(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::Unpair { endpoint_id } => {
                core.unpair(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Command::SetPeerName { endpoint_id, name } => {
                core.set_peer_pet_name(endpoint_id.clone(), Some(name.clone()))
                    .map_err(core_error)?;
                Reply::Done
            }
            Command::PeerHostname { endpoint_id } => Reply::Hostname {
                hostname: core
                    .peer_hostname(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }

    /// Commands that answer incoming control requests.
    fn access(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::ControlCheck => Reply::Check {
                check: core.control_check(),
            },
            Command::ControlRequest => Reply::ControlRequest {
                request: core.control_request().map_err(core_error)?.map(|request| {
                    TransferControlRequest {
                        endpoint_id: request.endpoint_id,
                        check: request.check,
                    }
                }),
            },
            Command::ConfirmControlRequest { digits } => Reply::Device {
                device: core
                    .confirm_control_request(digits.clone())
                    .map_err(core_error)?,
            },
            Command::RejectControlRequest => {
                core.reject_control_request().map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that report installation state without side effects.
    fn identity(core: &VnilinkCore, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::Snapshot => Reply::Snapshot {
                snapshot: core.snapshot().map_err(core_error)?,
            },
            Command::Endpoint => Reply::Endpoint {
                endpoint_id: core.endpoint_id(),
            },
            Command::Capabilities => Reply::Capabilities {
                capabilities: core.capabilities(),
            },
            Command::VideoCodecs => Reply::VideoCodecs {
                codecs: TransferVideoCodecCaps::from(core.video_codecs().map_err(core_error)?),
            },
            Command::SetVideoCodecs { codecs } => {
                core.set_video_codecs(codecs.clone().into())
                    .map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }

    /// Commands that grant service permissions and inspect terminal and file jobs.
    fn services(state: &mut State, command: &Command) -> Result<Reply> {
        let reply: Reply =
            match command {
                Command::SharingPolicy => Reply::SharingPolicy {
                    policy: state.core.sharing_policy().map_err(core_error)?,
                },
                Command::SetSharingPolicy { policy } => {
                    state.core.set_sharing_policy(*policy).map_err(core_error)?;
                    Reply::Done
                }
                Command::SystemPolicy => {
                    let policy = state.core.system_policy().map_err(core_error)?;
                    let support = state.core.system_support().map_err(core_error)?.support;
                    Reply::SystemPolicy {
                        policy,
                        support: TransferSystemSupport {
                            lock: support.lock,
                            restart: support.restart,
                            shutdown: support.shutdown,
                            privacy: support.privacy,
                        },
                    }
                }
                Command::SetSystemPolicy { policy } => {
                    state.core.set_system_policy(*policy).map_err(core_error)?;
                    Reply::Done
                }
                Command::TerminalPolicy => Reply::TerminalPolicy {
                    policy: TransferTerminalPolicy::from(
                        state.core.terminal_policy().map_err(core_error)?,
                    ),
                },
                Command::SetTerminalPolicy { policy } => {
                    state
                        .core
                        .set_terminal_policy(policy.clone().into())
                        .map_err(core_error)?;
                    Reply::Done
                }
                Command::EnableNativeTerminal { program, directory } => {
                    state
                        .core
                        .enable_native_terminal(program.clone(), directory.clone())
                        .map_err(core_error)?;
                    Reply::Done
                }
                Command::EnableDefaultNativeTerminal => {
                    state
                        .core
                        .enable_default_native_terminal()
                        .map_err(core_error)?;
                    Reply::Done
                }
                Command::DisableTerminal => {
                    state.core.disable_terminal().map_err(core_error)?;
                    Reply::Done
                }
                Command::RemoteTerminalSessions {
                    connection_id,
                    generation,
                } => {
                    let connection = parse_id(connection_id, "Invalid connection ID")?;
                    let generation = parse_id(generation, "Invalid generation")?;
                    Reply::TerminalSessions {
                        sessions: state
                            .core
                            .remote_terminal_sessions(connection, generation)
                            .map_err(core_error)?
                            .into_iter()
                            .map(|session| TransferTerminalSession {
                                id: session.id,
                                process_id: session.process_id,
                                shell: session.shell,
                                rows: session.rows,
                                columns: session.columns,
                                persistent: session.persistent,
                                attached: session.attached,
                            })
                            .collect(),
                    }
                }
                Command::CloseRemoteTerminalSession {
                    connection_id,
                    generation,
                    id,
                } => {
                    let connection = parse_id(connection_id, "Invalid connection ID")?;
                    let generation = parse_id(generation, "Invalid generation")?;
                    state
                        .core
                        .close_remote_terminal_session(connection, generation, id.clone())
                        .map_err(core_error)?;
                    Reply::Done
                }
                Command::HostedTerminalSessions => Reply::TerminalSessions {
                    sessions: state
                        .core
                        .hosted_terminal_sessions()
                        .map_err(core_error)?
                        .into_iter()
                        .map(|session| TransferTerminalSession {
                            id: session.id,
                            process_id: session.process_id,
                            shell: session.shell,
                            rows: session.rows,
                            columns: session.columns,
                            persistent: session.persistent,
                            attached: session.attached,
                        })
                        .collect(),
                },
                Command::CloseTerminalSession { id } => {
                    state
                        .core
                        .close_terminal_session(id.clone())
                        .map_err(core_error)?;
                    Reply::Done
                }
                Command::SubscribeFilesJobs {
                    session_id,
                    generation,
                } => {
                    let (id, gen) = session_target(session_id, generation)?;
                    let handle = state.core.open_files_service(id, gen).map_err(core_error)?;
                    let subscription = state.next_subscription;
                    state.next_subscription = subscription.saturating_add(1);
                    state.files.insert(subscription, handle);
                    Reply::FileJobsSubscription {
                        subscription_id: subscription.to_string(),
                    }
                }
                Command::PollFilesJobs { subscription_id } => {
                    let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                    let handle = state.files.get(&subscription).ok_or_else(|| {
                        Error::new(Status::InvalidArg, "Unknown files subscription")
                    })?;
                    Reply::Jobs {
                        jobs: handle
                            .jobs()
                            .map_err(core_error)?
                            .iter()
                            .map(transfer_job)
                            .collect(),
                    }
                }
                Command::ReleaseFilesJobs { subscription_id } => {
                    let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                    state.files.remove(&subscription);
                    Reply::Done
                }
                Command::FilesJobCancel {
                    subscription_id,
                    job_id,
                } => {
                    let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                    let handle = state.files.get(&subscription).ok_or_else(|| {
                        Error::new(Status::InvalidArg, "Unknown files subscription")
                    })?;
                    Reply::Job {
                        job: transfer_job(
                            &handle
                                .job_cancel(parse_id(job_id, "Invalid job ID")?)
                                .map_err(core_error)?,
                        ),
                    }
                }
                Command::FilesJobResume {
                    subscription_id,
                    job_id,
                } => {
                    let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                    let handle = state.files.get(&subscription).ok_or_else(|| {
                        Error::new(Status::InvalidArg, "Unknown files subscription")
                    })?;
                    Reply::Job {
                        job: transfer_job(
                            &handle
                                .job_resume(parse_id(job_id, "Invalid job ID")?)
                                .map_err(core_error)?,
                        ),
                    }
                }
                _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
            };
        Ok(reply)
    }

    /// Commands that drain the core event queue into transfer records.
    fn events(state: &mut State, command: &Command) -> Result<Reply> {
        let reply: Reply = match command {
            Command::SubscribeEvents => {
                let poller = state.core.subscribe_events().map_err(core_error)?;
                let subscription = state.next_subscription;
                state.next_subscription = subscription.saturating_add(1);
                state.events.insert(subscription, poller);
                Reply::EventSubscription {
                    subscription_id: subscription.to_string(),
                }
            }
            Command::PollEvents {
                subscription_id,
                max_events,
            } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                let poller = state
                    .events
                    .get(&subscription)
                    .ok_or_else(|| Error::new(Status::InvalidArg, "Unknown event subscription"))?;
                let events = poller
                    .poll(*max_events)
                    .iter()
                    .map(EventTransfer::event)
                    .collect();
                Reply::Events {
                    events,
                    lagged: poller.lagged().to_string(),
                }
            }
            Command::ReleaseEvents { subscription_id } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                state.events.remove(&subscription);
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

/// Owns command dispatch for one engine instance. The interface is a single
/// entry point; each command family lives in its own method so new
/// functionality extends the bridge by adding a method, never by editing a
/// shared procedure.
struct EngineBridge {
    state: Arc<Mutex<State>>,
}

impl EngineBridge {
    fn run(&self, command: &Command) -> Result<String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::from_reason("Engine unavailable"))?;
        if state.closed {
            return Err(core_error(FfiError::ShutDown));
        }
        let reply = Self::dispatch(&mut state, command)?;
        serde_json::to_string(&reply).map_err(|_| Error::from_reason("Cannot encode engine reply"))
    }

    /// Routes every command to its family. There is deliberately no fallback
    /// arm: adding a variant to `Command` fails to compile here until it is
    /// placed with its family.
    fn dispatch(state: &mut State, command: &Command) -> Result<Reply> {
        match command {
            Command::Snapshot
            | Command::Endpoint
            | Command::Capabilities
            | Command::VideoCodecs
            | Command::SetVideoCodecs { .. } => {
                let core = &state.core;
                Self::identity(core, command)
            }
            Command::CreateInvitation { .. }
            | Command::AcceptInvitation { .. }
            | Command::ExportPairingFile { .. }
            | Command::OpenPairingFile { .. }
            | Command::OpenPairingFileBase64 { .. }
            | Command::ConfirmPairing { .. }
            | Command::Revoke { .. }
            | Command::Unpair { .. }
            | Command::SetPeerName { .. }
            | Command::PeerHostname { .. } => {
                let core = &state.core;
                Self::pairing(core, command)
            }
            Command::ControlCheck
            | Command::ControlRequest
            | Command::ConfirmControlRequest { .. }
            | Command::RejectControlRequest => {
                let core = &state.core;
                Self::access(core, command)
            }
            Command::BeginConnectServices { .. }
            | Command::Connect { .. }
            | Command::ConnectFiles { .. }
            | Command::BeginConnect { .. }
            | Command::BeginConnectFiles { .. }
            | Command::Disconnect
            | Command::DisconnectSession { .. }
            | Command::ControllerSessions
            | Command::HostedSessions
            | Command::ControllerSession { .. }
            | Command::ForgetControllerSession { .. }
            | Command::SessionPaths { .. }
            | Command::ControllerSessionPaths { .. }
            | Command::ClipboardOwnerSession => {
                let core = &state.core;
                Self::sessions(core, command)
            }
            Command::SessionKeyboardMode { .. }
            | Command::SetSessionKeyboardMode { .. }
            | Command::SessionSwapControlAndCommand { .. }
            | Command::SetSessionSwapControlAndCommand { .. }
            | Command::SubmitKey { .. }
            | Command::SubmitPointer { .. }
            | Command::SubmitButton { .. }
            | Command::SubmitWheel { .. }
            | Command::SubmitMotion { .. }
            | Command::SubmitPreciseWheel { .. }
            | Command::SubmitText { .. }
            | Command::SubmitClipboard { .. }
            | Command::SubmitLocalKey { .. }
            | Command::ReleaseSessionInput { .. }
            | Command::SetInputDefaults { .. }
            | Command::EnableFilesystemAccess
            | Command::SetFileRoots { .. }
            | Command::KeyboardMode
            | Command::SetKeyboardMode { .. }
            | Command::SwapControlAndCommand
            | Command::SetSwapControlAndCommand { .. }
            | Command::SessionPointerSettings { .. }
            | Command::SetSessionPointerSettings { .. }
            | Command::SessionClipboardText { .. }
            | Command::ClipboardText => {
                let core = &state.core;
                Self::input(core, command)
            }
            Command::SaveCustomRelays { .. }
            | Command::StartHosting
            | Command::StopHosting
            | Command::StopHostedSession
            | Command::ResetIdentity
            | Command::SetRelayPublic
            | Command::SetRelayCustom { .. }
            | Command::SavedRelays
            | Command::RelayRestartRequired => {
                let core = &state.core;
                Self::hosting(core, command)
            }
            Command::StartNativeScreenSharing { .. }
            | Command::StopNativeScreenSharing
            | Command::NativeSharingStatus
            | Command::StartRecording { .. }
            | Command::RecordingStatus { .. }
            | Command::StopRecording { .. }
            | Command::ReleaseRecording { .. }
            | Command::RemoteCameraDevices { .. }
            | Command::RemoteAudioDevices { .. }
            | Command::NativeCameraDevices
            | Command::NativeAudioDevices
            | Command::NativeAudioOutputs
            | Command::SetNativeCameraSources { .. }
            | Command::SetNativeAudioSources { .. }
            | Command::PowerPolicy
            | Command::SetPowerPolicy { .. }
            | Command::PowerStatus
            | Command::NativeRuntime
            | Command::ReportNativePermissions { .. }
            | Command::EnableNativeInput => native::dispatch(state, command),
            Command::Topology
            | Command::SessionTopology { .. }
            | Command::SelectDisplays { .. }
            | Command::CaptureDisplays
            | Command::SetTopology { .. }
            | Command::SetDesktopCodecDefault { .. }
            | Command::SetDesktopQualityDefaults { .. }
            | Command::UpdateSessionQuality { .. }
            | Command::PublishVideo { .. }
            | Command::SubscribeSessionVideo { .. }
            | Command::PollVideo { .. }
            | Command::ReleaseVideo { .. }
            | Command::SubscribeVideo => Self::media(state, command),
            Command::SubscribeEvents
            | Command::PollEvents { .. }
            | Command::ReleaseEvents { .. } => Self::events(state, command),
            Command::SharingPolicy
            | Command::SetSharingPolicy { .. }
            | Command::SystemPolicy
            | Command::SetSystemPolicy { .. }
            | Command::TerminalPolicy
            | Command::SetTerminalPolicy { .. }
            | Command::EnableNativeTerminal { .. }
            | Command::EnableDefaultNativeTerminal
            | Command::DisableTerminal
            | Command::RemoteTerminalSessions { .. }
            | Command::CloseRemoteTerminalSession { .. }
            | Command::HostedTerminalSessions
            | Command::CloseTerminalSession { .. }
            | Command::SubscribeFilesJobs { .. }
            | Command::PollFilesJobs { .. }
            | Command::ReleaseFilesJobs { .. }
            | Command::FilesJobCancel { .. }
            | Command::FilesJobResume { .. } => Self::services(state, command),
            Command::Shutdown => {
                state.closed = true;
                native::shutdown(state)?;
                Ok(Reply::Done)
            }
        }
    }
}

pub struct CommandTask {
    bridge: EngineBridge,
    command: Command,
}
#[napi]
impl Engine {
    #[napi]
    pub fn execute(&self, command: String) -> Result<AsyncTask<CommandTask>> {
        if command.len() > COMMAND_BYTES_MAX {
            return Err(Error::new(Status::InvalidArg, "Command too large"));
        }
        // Single JSON parse, then an exact-shape check: serde unit variants
        // accept extra fields despite deny_unknown_fields, so the key set is
        // verified here before entering the core.
        let raw: serde_json::Value = serde_json::from_str(&command)
            .map_err(|_| Error::new(Status::InvalidArg, "Invalid engine command"))?;
        if !CommandShape::check(&raw) {
            return Err(Error::new(Status::InvalidArg, "Invalid engine command"));
        }
        let decoded: Command = serde_json::from_value(raw)
            .map_err(|_| Error::new(Status::InvalidArg, "Invalid engine command"))?;
        Ok(AsyncTask::new(CommandTask {
            bridge: EngineBridge {
                state: self.state.clone(),
            },
            command: decoded,
        }))
    }
}
impl Task for CommandTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<String> {
        self.bridge.run(&self.command)
    }
    fn resolve(&mut self, _: Env, output: String) -> Result<String> {
        Ok(output)
    }
}
/// Exact key sets per command type. Serde struct variants already reject
/// unknown fields, but unit variants silently accept extras, so every type is
/// checked here against one table.
struct CommandShape;

impl CommandShape {
    fn check(raw: &serde_json::Value) -> bool {
        let Some(object) = raw.as_object() else {
            return false;
        };
        let Some(kind) = object.get("type").and_then(|kind| kind.as_str()) else {
            return false;
        };
        if kind == "setSharingPolicy"
            && !object
                .get("policy")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|policy| {
                    !policy.is_empty() && policy.values().all(serde_json::Value::is_boolean)
                })
        {
            return false;
        }
        let expected: &[&str] = match kind {
            "snapshot" | "disconnect" | "shutdown" => &["type"],
            "endpoint"
            | "capabilities"
            | "videoCodecs"
            | "clipboardOwnerSession"
            | "subscribeEvents"
            | "subscribeVideo" => &["type"],
            "beginConnectServices" => &["type", "endpointId"],
            "setVideoCodecs" => &["type", "codecs"],
            "saveCustomRelays" => &["type", "relays"],
            "peerHostname" => &["type", "endpointId"],
            "createInvitation" => &["type", "ttlSeconds"],
            "exportPairingFile" => &["type", "ttlSeconds", "allowControl"],
            "acceptInvitation" => &["type", "token"],
            "openPairingFile" => &["type", "bytes"],
            "openPairingFileBase64" => &["type", "data"],
            "confirmPairing" => &["type", "previewId", "digits"],
            "revoke" | "unpair" => &["type", "endpointId"],
            "setPeerName" => &["type", "endpointId", "name"],
            "connect" | "beginConnect" | "connectFiles" | "beginConnectFiles" => {
                &["type", "endpointId"]
            }
            "disconnectSession" => &["type", "sessionId"],
            "submitKey" => &["type", "sessionId", "generation", "usage", "down"],
            "submitPointer" => &[
                "type",
                "sessionId",
                "generation",
                "x",
                "y",
                "display",
                "revision",
            ],
            "submitButton" => &["type", "sessionId", "generation", "button", "down"],
            "submitWheel" | "submitMotion" | "submitPreciseWheel" => {
                &["type", "sessionId", "generation", "dx", "dy"]
            }
            "submitText" | "submitClipboard" => &["type", "sessionId", "generation", "text"],
            "submitLocalKey" => &["type", "sessionId", "generation", "usage", "down", "text"],
            "releaseSessionInput"
            | "sessionPointerSettings"
            | "sessionClipboardText"
            | "subscribeSessionVideo"
            | "sessionKeyboardMode"
            | "sessionSwapControlAndCommand" => &["type", "sessionId", "generation"],
            "setSessionKeyboardMode" => &["type", "sessionId", "generation", "mode"],
            "setSessionSwapControlAndCommand" => &["type", "sessionId", "generation", "enabled"],
            "reportNativePermissions" => &["type", "screen", "input", "camera", "microphone"],
            "startNativeScreenSharing" => &[
                "type",
                "displayId",
                "width",
                "height",
                "pixelWidth",
                "pixelHeight",
            ],
            "startRecording" => {
                return raw.as_object().is_some_and(|v| {
                    [
                        "type",
                        "sessionId",
                        "generation",
                        "destination",
                        "maxBytes",
                        "maxDurationMs",
                    ]
                    .iter()
                    .all(|k| v.contains_key(*k))
                        && v.keys().all(|k| {
                            [
                                "type",
                                "sessionId",
                                "generation",
                                "destination",
                                "maxBytes",
                                "maxDurationMs",
                                "retention",
                            ]
                            .contains(&k.as_str())
                        })
                });
            }
            "recordingStatus" | "stopRecording" | "releaseRecording" => &["type", "id"],
            "remoteCameraDevices" => &["type", "sessionId", "generation"],
            "remoteAudioDevices" => &["type", "sessionId", "generation", "kind"],
            "setNativeCameraSources" => &["type", "ids"],
            "setNativeAudioSources" => &["type", "microphones", "system"],
            "setPowerPolicy" => &["type", "policy"],
            "nativeSharingStatus"
            | "stopNativeScreenSharing"
            | "nativeCameraDevices"
            | "nativeAudioDevices"
            | "nativeAudioOutputs"
            | "powerPolicy"
            | "powerStatus"
            | "nativeRuntime"
            | "enableNativeInput"
            | "keyboardMode"
            | "swapControlAndCommand"
            | "clipboardText"
            | "startHosting"
            | "stopHosting"
            | "stopHostedSession"
            | "resetIdentity"
            | "setRelayPublic"
            | "relayRestartRequired"
            | "savedRelays"
            | "controlCheck"
            | "controlRequest"
            | "rejectControlRequest"
            | "controllerSessions"
            | "hostedSessions"
            | "topology"
            | "captureDisplays" => &["type"],
            "setInputDefaults" => &["type", "mode", "swap", "pointer"],
            "enableFilesystemAccess" => &["type"],
            "setFileRoots" => &["type", "paths"],
            "setKeyboardMode" => &["type", "mode"],
            "setSwapControlAndCommand" => &["type", "enabled"],
            "setSessionPointerSettings" => &["type", "sessionId", "generation", "settings"],
            "setRelayCustom" => &["type", "urls"],
            "confirmControlRequest" => &["type", "digits"],
            "controllerSession"
            | "forgetControllerSession"
            | "sessionPaths"
            | "controllerSessionPaths" => &["type", "sessionId"],
            "pollEvents" => &["type", "subscriptionId", "maxEvents"],
            "releaseEvents" => &["type", "subscriptionId"],
            "sharingPolicy" => &["type"],
            "setSharingPolicy" => &["type", "policy"],
            "systemPolicy" => &["type"],
            "setSystemPolicy" => &["type", "policy"],
            "terminalPolicy" => &["type"],
            "setTerminalPolicy" => &["type", "policy"],
            "enableNativeTerminal" => &["type", "program", "directory"],
            "enableDefaultNativeTerminal" | "disableTerminal" | "hostedTerminalSessions" => {
                &["type"]
            }
            "remoteTerminalSessions" => &["type", "connectionId", "generation"],
            "closeRemoteTerminalSession" => &["type", "connectionId", "generation", "id"],
            "closeTerminalSession" => &["type", "id"],
            "subscribeFilesJobs" => &["type", "sessionId", "generation"],
            "pollFilesJobs" | "releaseFilesJobs" => &["type", "subscriptionId"],
            "filesJobCancel" | "filesJobResume" => &["type", "subscriptionId", "jobId"],
            "sessionTopology" => &["type", "sessionId", "generation"],
            "selectDisplays" => &["type", "sessionId", "generation", "ids"],
            "setTopology" => &["type", "revision", "displays"],
            "setDesktopCodecDefault" => &["type", "codec"],
            "setDesktopQualityDefaults" => &["type", "quality"],
            "updateSessionQuality" => &["type", "sessionId", "generation", "role", "quality"],
            "pollVideo" | "releaseVideo" => &["type", "subscriptionId"],
            "publishVideo" => &["type", "sessionId", "keyframe", "timestampUs", "data"],
            _ => return false,
        };
        object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
    }
}

fn ffi_error_name(error: &FfiError) -> &'static str {
    match error {
        FfiError::ShutDown => "ShutDown",
        FfiError::InvalidRelay { .. } => "InvalidRelay",
        FfiError::InvalidSetting { .. } => "InvalidSetting",
        FfiError::InvalidRelayUrl { .. } => "InvalidRelayUrl",
        FfiError::InvalidInvitation { .. } => "InvalidInvitation",
        FfiError::InvitationExpired => "InvitationExpired",
        FfiError::InvitationReplayed => "InvitationReplayed",
        FfiError::InvitationCacheFull => "InvitationCacheFull",
        FfiError::NotPaired => "NotPaired",
        FfiError::NotHosting => "NotHosting",
        FfiError::Busy => "Busy",
        FfiError::Unauthorized => "Unauthorized",
        FfiError::ClipboardDenied => "ClipboardDenied",
        FfiError::TransferDenied => "TransferDenied",
        FfiError::FileExists => "FileExists",
        FfiError::NoSession => "NoSession",
        FfiError::StaleSession => "StaleSession",
        FfiError::Unsupported { .. } => "Unsupported",
        FfiError::InputFailed { .. } => "InputFailed",
        FfiError::IncompatibleVersion => "IncompatibleVersion",
        FfiError::Malformed => "Malformed",
        FfiError::Timeout => "Timeout",
        FfiError::Network { .. } => "Network",
        FfiError::TooManyDevices => "TooManyDevices",
        FfiError::StoreUnavailable | FfiError::StoreRecoveryRequired => "StoreUnavailable",
        FfiError::StoreLocked => "StoreLocked",
        FfiError::CorruptRecord => "CorruptRecord",
        FfiError::InvalidPairingFile => "InvalidPairingFile",
        FfiError::PairingCheckMismatch => "PairingCheckMismatch",
        FfiError::NoControlRequest => "NoControlRequest",
    }
}

fn core_error(error: FfiError) -> Error {
    Error::from_reason(format!("{}: {error}", ffi_error_name(&error)))
}

#[cfg(test)]
mod ffi_store_error {
    use super::ffi_error_name;
    use vnilink_app::FfiError;

    #[test]
    fn recovery_reuses_the_unavailable_name() {
        let error = FfiError::StoreRecoveryRequired;
        assert_eq!(ffi_error_name(&error), "StoreUnavailable");
        assert_eq!(
            format!("{}: {error}", ffi_error_name(&error)),
            "StoreUnavailable: the installation store must be reopened and VniLink restarted"
        );
        assert_eq!(
            ffi_error_name(&FfiError::StoreUnavailable),
            "StoreUnavailable"
        );
        assert_eq!(ffi_error_name(&FfiError::StoreLocked), "StoreLocked");
        assert_eq!(ffi_error_name(&FfiError::CorruptRecord), "CorruptRecord");
    }
}
