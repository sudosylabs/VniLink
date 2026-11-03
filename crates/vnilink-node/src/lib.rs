// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Node-API control bridge. Input, session, and video-frame polling cross
//! here as JSON; callers transfer frame bytes as base64. Poll stateful video
//! subscriptions instead of holding pollers in JavaScript.
//! Management calls run on libuv workers. The separate data module uses
//! bounded, nonblocking native drains and input queues outside this mutex.
// trace: records every ignored failure
mod native;
mod storage;
mod system;
#[cfg(test)]
mod wait;

use native::{
    TransferCapability, TransferNativeBackend, TransferNativeDevice, TransferPowerPolicy,
    TransferPowerStatus, TransferRecordingRetention, TransferRecordingStatus,
    TransferSharingStatus,
};
#[cfg(feature = "reset-installation")]
pub use storage::reset_installation_cli;
mod data;
mod files;

use napi::threadsafe_function::ThreadsafeFunctionCallMode;
use napi::{
    bindgen_prelude::{AsyncTask, Function},
    Env, Error, Result, Status, Task,
};
use napi_derive::napi;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use vnilink_app::{EventPoller, FfiError, VnilinkCore};

/// Largest accepted command body. Clipboard text reaches 256 KiB and encoded
/// frames cross as base64, so this must clear a ~1 MiB frame plus JSON shape.
const COMMAND_BYTES_MAX: usize = 2 * 1024 * 1024;

/// Entries per local listing page: the core's largest page.
const LIST_PAGE: u16 = 500;

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
#[serde(deny_unknown_fields)]
struct TransferWindowDecoder {
    codec: String,
    hardware: bool,
    chroma444: bool,
    bt601: bool,
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

/// A shell this computer can offer, as the window lists it.
#[derive(Serialize)]
struct TransferShell {
    id: String,
    label: String,
}

/// Longest shell id: a path on Unix, a name on Windows.
const MAX_SHELL_ID: usize = 1024;

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
pub(crate) struct TransferJob {
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
    current_file: Option<String>,
    error: Option<String>,
    destination: Option<TransferDestination>,
}

/// Where a transfer lands, with its root as a decimal string like other ids.
#[derive(Serialize)]
pub(crate) struct TransferDestination {
    root: String,
    #[serde(rename = "path")]
    folder: String,
    items: Vec<TransferItem>,
}

/// A file or folder on this computer, by its root, its folder and its name.
#[derive(Serialize)]
pub(crate) struct TransferLocation {
    root: String,
    #[serde(rename = "path")]
    folder: String,
    name: String,
}

/// One file or folder a transfer lands, by name.
#[derive(Serialize)]
pub(crate) struct TransferItem {
    name: String,
    folder: bool,
}

pub(crate) fn transfer_job(snapshot: &vnilink_app::FfiJobSnapshot) -> TransferJob {
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
        current_file: snapshot.current_file.clone(),
        error: snapshot.error.clone(),
        destination: snapshot
            .destination
            .as_ref()
            .map(|destination| TransferDestination {
                root: destination.root.to_string(),
                folder: destination.folder.clone(),
                items: destination
                    .items
                    .iter()
                    .map(|item| TransferItem {
                        name: item.name.clone(),
                        folder: item.folder,
                    })
                    .collect(),
            }),
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
            vnilink_app::FfiServiceKind::ScreenSnapshot => "ScreenSnapshot",
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
            vnilink_app::FfiCloseCause::Failed(_) => "Failed",
        }
    }

    fn stream_failure(failure: &vnilink_app::FfiStreamFailure) -> &'static str {
        match failure {
            vnilink_app::FfiStreamFailure::Encoder => "encoder",
            vnilink_app::FfiStreamFailure::Capture => "capture",
            vnilink_app::FfiStreamFailure::Permission => "permission",
            vnilink_app::FfiStreamFailure::DisplayGone => "displayGone",
        }
    }

    fn service_phase(phase: &vnilink_app::FfiServicePhase) -> serde_json::Value {
        match phase {
            vnilink_app::FfiServicePhase::Waiting => serde_json::Value::String("Waiting".into()),
            vnilink_app::FfiServicePhase::Active => serde_json::Value::String("Active".into()),
            vnilink_app::FfiServicePhase::Suspended => {
                serde_json::Value::String("Suspended".into())
            }
            vnilink_app::FfiServicePhase::Closed {
                cause: vnilink_app::FfiCloseCause::Failed(failure),
            } => {
                serde_json::json!({ "Closed": { "cause": "Failed", "failure": Self::stream_failure(failure) } })
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
            vnilink_app::FfiEvent::FolderChanged { watch } => {
                serde_json::json!({ "type": "folderChanged", "watch": watch.to_string() })
            }
            vnilink_app::FfiEvent::SystemActionScheduled {
                operation_id,
                action,
                endpoint_id,
                remaining_ms,
            } => serde_json::json!({
                "type": "systemActionScheduled",
                "id": operation_id.to_string(),
                "action": system::action_name(*action),
                "endpoint_id": endpoint_id,
                "remainingMs": remaining_ms,
            }),
            vnilink_app::FfiEvent::SystemActionEnded {
                operation_id,
                state,
            } => serde_json::json!({
                "type": "systemActionEnded",
                "id": operation_id.to_string(),
                "state": system::state_name(*state),
            }),
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
            vnilink_app::FfiEvent::ControlRequested { endpoint_id } => {
                serde_json::json!({ "type": "controlRequested", "endpoint_id": endpoint_id })
            }
            vnilink_app::FfiEvent::AdmissionRequested {
                request_id,
                endpoint_id,
                remaining_ms,
            } => serde_json::json!({
                "type": "admissionRequested",
                "id": request_id.to_string(),
                "endpoint_id": endpoint_id,
                "remainingMs": remaining_ms,
            }),
            vnilink_app::FfiEvent::AdmissionRequestEnded { request_id } => {
                serde_json::json!({ "type": "admissionRequestEnded", "id": request_id.to_string() })
            }
            vnilink_app::FfiEvent::AdmissionWaiting {
                endpoint_id,
                waiting,
            } => serde_json::json!({
                "type": "admissionWaiting",
                "endpoint_id": endpoint_id,
                "waiting": waiting,
            }),
            vnilink_app::FfiEvent::ControlCheck { endpoint_id, check } => {
                serde_json::json!({ "type": "controlCheck", "endpoint_id": endpoint_id, "check": check })
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
            vnilink_app::FfiEvent::PresenceChanged {
                endpoint_id,
                presence,
            } => {
                serde_json::json!({ "type": "presenceChanged", "endpoint_id": endpoint_id, "presence": presence })
            }
            vnilink_app::FfiEvent::PeerAvailabilityChanged { endpoint_id } => {
                serde_json::json!({ "type": "peerAvailabilityChanged", "endpoint_id": endpoint_id })
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
            vnilink_app::FfiEvent::InputFailed { id, message } => {
                serde_json::json!({ "type": "inputFailed", "id": id.to_string(), "message": message })
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

/// A paired computer waiting for the person here, as the window reads it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferAdmissionRequest {
    id: String,
    #[serde(rename = "endpoint_id")]
    endpoint_id: String,
    remaining_ms: u64,
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
enum Call {
    BeginConnectServices {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
    Snapshot,
    /// The bounded diagnostic snapshot as JSON; see `vnilink::diagnostics`.
    DiagnosticSnapshot,
    /// The flight recorder's events as JSON lines; see `vnilink_log`.
    DiagnosticRecorder,
    /// The engine log's filter in force and where it came from.
    LogFilter,
    /// Applies the application's log filter setting, in `tracing`'s
    /// directive syntax; `null` asks for the default.
    SetLogFilter {
        directives: Option<String>,
    },
    /// The end of this run's engine log file.
    EngineLogTail,
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
    /// Paired computers waiting for the person here to accept them.
    AdmissionRequests,
    AnswerAdmissionRequest {
        id: String,
        accept: bool,
    },
    SharingPolicy,
    SetSharingPolicy {
        policy: vnilink_app::FfiSharingPolicyUpdate,
    },
    SystemPolicy,
    SetSystemPolicy {
        policy: TransferSystemPolicy,
    },
    /// Asks a paired computer to lock, restart or shut down.
    SystemAction {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        action: String,
    },
    SystemOperation {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        id: String,
    },
    CancelSystemAction {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        id: String,
    },
    /// Blanks a paired computer's screen and blocks its keyboard and
    /// mouse, or gives them back. The lease lasts while this computer's
    /// System owner on that connection is open.
    SetSystemPrivacy {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        enabled: bool,
    },
    SystemPrivacy {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    /// Restarts and shutdowns counting down on this computer.
    ScheduledSystemActions,
    CancelHostedSystemAction {
        id: String,
    },
    RunHostedSystemActionNow {
        id: String,
    },
    TerminalPolicy,
    SetTerminalPolicy {
        policy: TransferTerminalPolicy,
    },
    TerminalShells,
    /// `null` offers the automatic shell; otherwise a detected shell's id.
    SetTerminalShell {
        shell: Option<String>,
    },
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
    ComputerName,
    SetComputerName {
        name: Option<String>,
    },
    Disconnect,
    DisconnectSession {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    /// Desktop input, from `SubmitKey` to `ReleaseSessionInput`: decoded on
    /// a desktop's data channel only (`data.rs`), naming the desktop's
    /// connection and generation.
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
    SubmitLocalKey {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        usage: u16,
        down: bool,
        text: String,
        /// A dead key or an input-method step that has not typed a character yet.
        composing: bool,
    },
    ReleaseSessionInput {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
    },
    /// The display a desktop shows and its pointer lands on, sent on the
    /// desktop's data channel in order with its input.
    SelectDisplays {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        ids: Vec<String>,
    },
    /// Whether this desktop exchanges Control and Command, sent in order
    /// with its keys; refused while a key is held.
    SwapSessionModifiers {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        enabled: bool,
    },
    /// Whether this computer's own shortcuts, such as Alt-Tab, go to this
    /// desktop; on while it has the keyboard.
    CaptureSystemKeys {
        #[serde(rename = "sessionId")]
        session_id: String,
        generation: String,
        enabled: bool,
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
        /// `camera` records the connection's open camera; otherwise its desktop.
        source: Option<RecordingSource>,
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
    SetTransferDirectory {
        path: String,
    },
    /// This computer's roots, home folder and usual folders.
    LocalPlaces,
    /// Names a path on this computer, such as one dropped from the system's file manager.
    LocateLocal {
        path: String,
    },
    /// Watches one folder on this computer; its changes arrive as `folderChanged` events.
    WatchLocal {
        root: String,
        path: String,
    },
    UnwatchLocal {
        watch: String,
    },
    ListLocal {
        root: String,
        path: String,
        offset: String,
    },
    CreateLocalDir {
        root: String,
        path: String,
        name: String,
    },
    RenameLocal {
        root: String,
        path: String,
        name: String,
        token: Option<files::TransferToken>,
        #[serde(rename = "newName")]
        new_name: String,
    },
    /// To this computer's Trash when `trash`, else permanently.
    DeleteLocal {
        root: String,
        path: String,
        name: String,
        token: Option<files::TransferToken>,
        trash: bool,
    },
    KeyboardMode,
    SetKeyboardMode {
        mode: String,
    },
    SwapControlAndCommand,
    SetSwapControlAndCommand {
        enabled: bool,
    },
    StartHosting,
    StopHosting,
    StopHostedSession,
    StopHostedPeer {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    ResetIdentity,
    SetRelayPublic,
    SetRelayCustom {
        urls: Vec<String>,
    },
    SavedRelays,
    RelayRestartRequired,
    ControlRequest,
    ConfirmControlRequest {
        digits: String,
    },
    RejectControlRequest,
    ControllerSessions,
    HostedSessions,
    HostedServices,
    PeerAvailability {
        #[serde(rename = "endpointId")]
        endpoint_id: String,
    },
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
    CaptureDisplays,
    SetTopology {
        revision: String,
        displays: Vec<TransferDisplay>,
    },
    SetDesktopCodecDefault {
        codec: String,
    },
    SetWindowDecoders {
        decoders: Vec<TransferWindowDecoder>,
    },
    SetDesktopQualityDefaults {
        quality: TransferQuality,
    },
    Shutdown,
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
    /// At most `vnilink::diagnostics::MAX_EXPORT_BYTES` of JSON, carried as
    /// a string so the window saves exactly what the engine exported.
    DiagnosticSnapshot {
        snapshot: String,
    },
    /// At most `vnilink_log::MAX_EXPORT_BYTES` of JSON lines.
    DiagnosticRecorder {
        events: String,
    },
    LogFilter {
        directives: String,
        /// `environment`, `setting` or `default`.
        source: &'static str,
    },
    /// At most [`MAX_LOG_TAIL`] bytes of JSON lines, and the file's path
    /// with the home folder written `~`; no path when the engine writes no
    /// log file.
    EngineLogTail {
        path: Option<String>,
        text: String,
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
    Services {
        services: Vec<serde_json::Value>,
    },
    PeerAvailability {
        /// `null` until the paired computer's link has delivered a report.
        report: Option<Vec<serde_json::Value>>,
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
    ComputerName {
        chosen: Option<String>,
        system: Option<String>,
    },
    EventSubscription {
        #[serde(rename = "subscriptionId")]
        subscription_id: String,
    },
    Events {
        events: Vec<serde_json::Value>,
        lagged: String,
    },
    AdmissionRequests {
        requests: Vec<TransferAdmissionRequest>,
    },
    SharingPolicy {
        policy: TransferSharingPolicy,
    },
    SystemPolicy {
        policy: TransferSystemPolicy,
        support: TransferSystemSupport,
    },
    SystemOperation {
        operation: system::TransferSystemOperation,
    },
    ScheduledSystemActions {
        actions: Vec<system::TransferScheduledSystemAction>,
    },
    SystemPrivacy {
        privacy: system::TransferPrivacy,
    },
    TerminalPolicy {
        policy: TransferTerminalPolicy,
    },
    TerminalSessions {
        sessions: Vec<TransferTerminalSession>,
    },
    TerminalShells {
        shells: Vec<TransferShell>,
        automatic: String,
        current: Option<String>,
    },
    Places {
        places: files::TransferPlaces,
    },
    Location {
        location: Option<TransferLocation>,
    },
    Watching {
        watch: String,
    },
    DirPage {
        page: files::TransferDirPage,
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
// Event pollers stay here behind subscription ids because pollers cannot
// cross the JSON bridge; callers poll and release them by id.
struct State {
    core: Arc<VnilinkCore>,
    native: native::NativeRuntimeState,
    /// The System service held on each connection this computer asked.
    systems: HashMap<(u64, u64), Arc<vnilink_app::SystemServiceHandle>>,
}
/// The event queues the window host reads. Apart from [`State`] so that a
/// slow native command cannot hold a read of events.
struct Subscriptions {
    events: HashMap<u64, EventPoller>,
    next: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RecordingSource {
    Desktop,
    Camera,
}
#[napi]
pub struct Engine {
    core: Arc<VnilinkCore>,
    desktops: Arc<data::DesktopChannels>,
    cameras: Arc<data::CameraChannels>,
    state: Arc<Mutex<State>>,
    subscriptions: Arc<Mutex<Subscriptions>>,
    closed: Arc<AtomicBool>,
}

pub struct OpenedEngine {
    core: VnilinkCore,
    backends: Vec<TransferNativeBackend>,
    screen_grant: Option<std::path::PathBuf>,
}
pub struct OpenTask {
    temporary: bool,
    /// A temporary engine that may host this computer's screen, for the
    /// latency measurement. Only video is prepared: no input, files or shell.
    capture: bool,
    /// Where a temporary engine that captures keeps its screen sharing
    /// grant, so a measurement asks once; a persistent engine keeps it in
    /// its installation's private directory.
    screen_grant: Option<std::path::PathBuf>,
}
impl Task for OpenTask {
    type Output = OpenedEngine;
    type JsValue = Engine;
    fn compute(&mut self) -> Result<Self::Output> {
        let (result, screen_grant, screenshot_grant) = if self.temporary {
            start_log_file(None);
            (
                VnilinkCore::create(true, vec![]),
                self.screen_grant.clone().filter(|_| self.capture),
                None,
            )
        } else {
            let store =
                storage::EncryptedInstallationStore::open_production().map_err(core_error)?;
            start_log_file(Some(&store.log_folder()));
            let (screen_grant, screenshot_grant) = (store.screen_grant(), store.screenshot_grant());
            (
                VnilinkCore::open_persistent(Box::new(store)),
                Some(screen_grant),
                Some(screenshot_grant),
            )
        };
        let core = result.map_err(core_error)?;
        let backends = if self.temporary && self.capture {
            let video = core.prepare_native_video();
            vec![vnilink_app::FfiNativeBackend {
                name: "video".into(),
                ready: video.is_ok(),
                error: video
                    .err()
                    .map(|_| "The native video backend could not be initialized".into()),
            }
            .into()]
        } else if self.temporary {
            vec![]
        } else {
            core.initialize_native_backends(screenshot_grant)
                .into_iter()
                .map(Into::into)
                .collect()
        };
        core.start_file_job_driver();
        // Paired computers show as connected only while they are reachable.
        core.set_presence_links(true).map_err(core_error)?;
        Ok(OpenedEngine {
            core,
            backends,
            screen_grant,
        })
    }
    fn resolve(&mut self, _: Env, opened: Self::Output) -> Result<Engine> {
        let core = Arc::new(opened.core);
        let desktops = Arc::new(data::DesktopChannels::default());
        let cameras = Arc::new(data::CameraChannels::default());
        Ok(Engine {
            core: core.clone(),
            desktops: desktops.clone(),
            cameras: cameras.clone(),
            state: Arc::new(Mutex::new(State {
                core,
                native: native::NativeRuntimeState::new(
                    opened.backends,
                    desktops,
                    cameras,
                    opened.screen_grant,
                ),
                systems: HashMap::new(),
            })),
            subscriptions: Arc::new(Mutex::new(Subscriptions {
                events: HashMap::new(),
                next: 1,
            })),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }
}
/// The most of the engine log file one reply carries.
const MAX_LOG_TAIL: usize = 256 * 1024;

fn log_filter_reply(state: vnilink_log::FilterState) -> Reply {
    Reply::LogFilter {
        directives: state.directives,
        source: state.source.as_str(),
    }
}

/// `path` with the home folder written `~`, so a reply names no account.
fn without_home(path: &std::path::Path) -> String {
    let home = dirs::home_dir();
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(inside) => std::path::Path::new("~").join(inside).display().to_string(),
        None => path.display().to_string(),
    }
}

/// Starts this run's engine log file: a persistent engine's in its own
/// private folder, a temporary engine's only where `VNILINK_LOG_DIR` asks.
/// An engine that cannot write one still runs, with its recorder.
fn start_log_file(folder: Option<&std::path::Path>) {
    let configured = std::env::var_os(vnilink_log::LOG_DIR_ENV).filter(|value| !value.is_empty());
    let Some(folder) = folder.or(configured.as_deref().map(std::path::Path::new)) else {
        return;
    };
    if vnilink_log::write_to(folder).is_err() {
        tracing::warn!(target: "vnilink::storage", "the engine log file could not be opened");
    }
}

#[napi]
pub fn open_engine() -> AsyncTask<OpenTask> {
    vnilink_log::install();
    AsyncTask::new(OpenTask {
        temporary: false,
        capture: false,
        screen_grant: None,
    })
}
// Explicit isolated test/preview mode; never a fallback for unavailable storage.
// `capture` lets it host this computer's screen, for the latency measurement,
// keeping its screen sharing grant in `screen_grant`.
#[napi]
pub fn open_temporary_engine(
    capture: Option<bool>,
    screen_grant: Option<String>,
) -> AsyncTask<OpenTask> {
    vnilink_log::install();
    AsyncTask::new(OpenTask {
        temporary: true,
        capture: capture.unwrap_or(false),
        screen_grant: screen_grant.map(std::path::PathBuf::from),
    })
}

impl EngineBridge {
    /// Commands that describe displays and set desktop codec and quality defaults.
    fn media(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::Topology => Reply::Displays {
                displays: core
                    .topology()
                    .map_err(core_error)?
                    .iter()
                    .map(transfer_display)
                    .collect(),
            },
            Call::SessionTopology {
                session_id,
                generation,
            } => {
                let (id, gen) = session_target(session_id, generation)?;
                Reply::Displays {
                    displays: core
                        .session_topology(id, gen)
                        .map_err(core_error)?
                        .iter()
                        .map(transfer_display)
                        .collect(),
                }
            }
            Call::CaptureDisplays => Reply::DisplayIds {
                displays: core
                    .capture_displays()
                    .map_err(core_error)?
                    .into_iter()
                    .map(|display| display.to_string())
                    .collect(),
            },
            Call::SetTopology { revision, displays } => {
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
                core.set_topology(revision, displays).map_err(core_error)?;
                Reply::Done
            }
            Call::SetWindowDecoders { decoders } => {
                use vnilink_app::FfiVideoCodec as C;
                let decoders = decoders
                    .iter()
                    .map(|decoder| {
                        let codec = match decoder.codec.as_str() {
                            "h264" => C::H264,
                            "h265" => C::H265,
                            "vp8" => C::Vp8,
                            "vp9" => C::Vp9,
                            "av1" => C::Av1,
                            _ => {
                                return Err(core_error(FfiError::InvalidSetting {
                                    message: "invalid window decoder".into(),
                                }))
                            }
                        };
                        Ok(vnilink_app::FfiWindowDecoder {
                            codec,
                            hardware: decoder.hardware,
                            chroma444: decoder.chroma444,
                            bt601: decoder.bt601,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                core.set_window_decoders(decoders).map_err(core_error)?;
                Reply::Done
            }
            Call::SetDesktopCodecDefault { codec } => {
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
                core.set_desktop_codec_default(preference)
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::SetDesktopQualityDefaults { quality } => {
                let quality = vnilink_app::new_quality(
                    quality.max_bitrate_kbps,
                    quality.max_frame_rate,
                    quality.max_width,
                    quality.max_height,
                )
                .map_err(core_error)?;
                core.set_desktop_quality_defaults(quality)
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
    fn hosting(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::StartHosting => {
                core.start_hosting().map_err(core_error)?;
                Reply::Done
            }
            Call::StopHosting => {
                core.stop_hosting().map_err(core_error)?;
                Reply::Done
            }
            Call::StopHostedSession => {
                core.stop_hosted_session().map_err(core_error)?;
                Reply::Done
            }
            Call::StopHostedPeer { session_id } => {
                let id = parse_id(session_id, "Invalid session ID")?;
                core.stop_hosted_peer(id).map_err(core_error)?;
                Reply::Done
            }
            Call::ResetIdentity => {
                core.reset_identity().map_err(core_error)?;
                Reply::Done
            }
            Call::SetRelayPublic => {
                core.set_relay_public().map_err(core_error)?;
                Reply::Done
            }
            Call::SetRelayCustom { urls } => {
                core.set_relay_custom(urls.clone()).map_err(core_error)?;
                Reply::Done
            }
            Call::SaveCustomRelays { relays } => {
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
            Call::RelayRestartRequired => Reply::RestartRequired {
                required: core.relay_restart_required().map_err(core_error)?,
            },
            Call::SavedRelays => Reply::Relays {
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
    /// The input defaults the controller's desktops open with, and this
    /// computer's own files.
    fn input(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::SetInputDefaults {
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
            Call::EnableFilesystemAccess => {
                core.enable_filesystem_access().map_err(core_error)?;
                Reply::Done
            }
            Call::SetTransferDirectory { path } => {
                core.set_transfer_directory(path.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::LocalPlaces => Reply::Places {
                places: files::transfer_places(core.places().map_err(core_error)?),
            },
            Call::WatchLocal { root, path } => Reply::Watching {
                watch: core
                    .watch_folder(parse_id(root, "Invalid root")?, path.clone())
                    .map_err(core_error)?
                    .to_string(),
            },
            Call::UnwatchLocal { watch } => {
                core.unwatch_folder(parse_id(watch, "Invalid watch")?);
                Reply::Done
            }
            Call::LocateLocal { path } => Reply::Location {
                location: core.locate(path.clone()).map_err(core_error)?.map(|found| {
                    TransferLocation {
                        root: found.root.to_string(),
                        folder: found.path,
                        name: found.name,
                    }
                }),
            },
            Call::ListLocal { root, path, offset } => Reply::DirPage {
                page: files::transfer_page(
                    core.list_dir(
                        parse_id(root, "Invalid root")?,
                        path.clone(),
                        parse_id(offset, "Invalid offset")?,
                        LIST_PAGE,
                    )
                    .map_err(core_error)?,
                ),
            },
            Call::CreateLocalDir { root, path, name } => {
                core.create_dir(parse_id(root, "Invalid root")?, path.clone(), name.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::RenameLocal {
                root,
                path,
                name,
                token,
                new_name,
            } => {
                core.rename_entry(
                    parse_id(root, "Invalid root")?,
                    path.clone(),
                    name.clone(),
                    token.as_ref().map(files::entry_token).transpose()?,
                    new_name.clone(),
                )
                .map_err(core_error)?;
                Reply::Done
            }
            Call::DeleteLocal {
                root,
                path,
                name,
                token,
                trash,
            } => {
                core.delete_entry(
                    parse_id(root, "Invalid root")?,
                    path.clone(),
                    name.clone(),
                    token.as_ref().map(files::entry_token).transpose()?,
                    *trash,
                )
                .map_err(core_error)?;
                Reply::Done
            }
            Call::KeyboardMode => Reply::KeyboardMode {
                mode: keyboard_mode_name(core.keyboard_mode().map_err(core_error)?).to_string(),
            },
            Call::SetKeyboardMode { mode } => {
                core.set_keyboard_mode(keyboard_mode(mode)?)
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::SwapControlAndCommand => Reply::Swap {
                enabled: core.swap_control_and_command().map_err(core_error)?,
            },
            Call::SetSwapControlAndCommand { enabled } => {
                core.set_swap_control_and_command(*enabled)
                    .map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that dial, list, and tear down sessions.
    fn sessions(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::BeginConnectServices { endpoint_id } => Reply::Session {
                session: core
                    .begin_connect_services(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            Call::Disconnect => {
                core.disconnect().map_err(core_error)?;
                Reply::Done
            }
            Call::DisconnectSession { session_id } => {
                let id = parse_id(session_id, "Invalid session ID")?;
                core.disconnect_session(id).map_err(core_error)?;
                Reply::Done
            }
            Call::ControllerSessions => Reply::Sessions {
                sessions: core.controller_sessions().map_err(core_error)?,
            },
            Call::HostedSessions => Reply::Sessions {
                sessions: core.hosted_sessions().map_err(core_error)?,
            },
            Call::HostedServices => Reply::Services {
                services: core
                    .hosted_services()
                    .map_err(core_error)?
                    .iter()
                    .map(EventTransfer::service)
                    .collect(),
            },
            Call::PeerAvailability { endpoint_id } => Reply::PeerAvailability {
                report: core
                    .peer_availability(endpoint_id.clone())
                    .map_err(core_error)?
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|entry| {
                                serde_json::json!({ "capability": entry.capability, "status": entry.status })
                            })
                            .collect()
                    }),
            },
            Call::ControllerSession { session_id } => Reply::Session {
                session: core
                    .controller_session(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?,
            },
            Call::ForgetControllerSession { session_id } => {
                core.forget_controller_session(parse_id(session_id, "Invalid session ID")?)
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::SessionPaths { session_id } => Reply::Paths {
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
            Call::ControllerSessionPaths { session_id } => Reply::Paths {
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
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }
}

impl EngineBridge {
    /// Commands that pair devices and manage the device list.
    fn pairing(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::CreateInvitation { ttl_seconds } => Reply::Invitation {
                token: core
                    .create_invitation(u64::from(*ttl_seconds))
                    .map_err(core_error)?,
            },
            Call::AcceptInvitation { token } => Reply::Device {
                device: core.accept_invitation(token.clone()).map_err(core_error)?,
            },
            Call::ExportPairingFile {
                ttl_seconds,
                allow_control,
            } => Reply::PairingFile {
                file: core
                    .export_pairing_file(u64::from(*ttl_seconds), *allow_control)
                    .map_err(core_error)?,
            },
            Call::OpenPairingFile { bytes } => Reply::PairingPreview {
                preview: core.open_pairing_file(bytes.clone()).map_err(core_error)?,
            },
            Call::OpenPairingFileBase64 { data } => {
                use base64::Engine as _;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid pairing file data"))?;
                Reply::PairingPreview {
                    preview: core.open_pairing_file(bytes).map_err(core_error)?,
                }
            }
            Call::ConfirmPairing { preview_id, digits } => {
                let id = preview_id
                    .parse::<u64>()
                    .map_err(|_| Error::new(Status::InvalidArg, "Invalid preview ID"))?;
                Reply::Device {
                    device: core
                        .confirm_pairing(id, digits.clone())
                        .map_err(core_error)?,
                }
            }
            Call::Revoke { endpoint_id } => {
                core.revoke(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Call::Unpair { endpoint_id } => {
                core.unpair(endpoint_id.clone()).map_err(core_error)?;
                Reply::Done
            }
            Call::SetPeerName { endpoint_id, name } => {
                core.set_peer_pet_name(endpoint_id.clone(), Some(name.clone()))
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::ComputerName => {
                let (chosen, system) = core.computer_name().map_err(core_error)?;
                Reply::ComputerName { chosen, system }
            }
            Call::SetComputerName { name } => {
                core.set_computer_name(name.clone()).map_err(core_error)?;
                Reply::Done
            }
            Call::PeerHostname { endpoint_id } => Reply::Hostname {
                hostname: core
                    .peer_hostname(endpoint_id.clone())
                    .map_err(core_error)?,
            },
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }

    /// Commands that answer incoming control requests.
    fn access(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::ControlRequest => Reply::ControlRequest {
                request: core.control_request().map_err(core_error)?.map(|request| {
                    TransferControlRequest {
                        endpoint_id: request.endpoint_id,
                    }
                }),
            },
            Call::ConfirmControlRequest { digits } => Reply::Device {
                device: core
                    .confirm_control_request(digits.clone())
                    .map_err(core_error)?,
            },
            Call::RejectControlRequest => {
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
    fn identity(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::Snapshot => Reply::Snapshot {
                snapshot: core.snapshot().map_err(core_error)?,
            },
            Call::DiagnosticSnapshot => Reply::DiagnosticSnapshot {
                snapshot: core.diagnostic_snapshot_json().map_err(core_error)?,
            },
            Call::DiagnosticRecorder => Reply::DiagnosticRecorder {
                events: vnilink_log::export(),
            },
            Call::LogFilter => log_filter_reply(vnilink_log::filter()),
            Call::SetLogFilter { directives } => log_filter_reply(
                vnilink_log::set_filter(directives.as_deref())
                    .map_err(|error| Error::new(Status::InvalidArg, error.to_string()))?,
            ),
            Call::EngineLogTail => Reply::EngineLogTail {
                path: vnilink_log::log_file().map(|path| without_home(&path)),
                text: vnilink_log::tail(MAX_LOG_TAIL)
                    .map_err(|_| Error::from_reason("The engine log could not be read"))?,
            },
            Call::Endpoint => Reply::Endpoint {
                endpoint_id: core.endpoint_id(),
            },
            Call::Capabilities => Reply::Capabilities {
                capabilities: core.capabilities(),
            },
            Call::VideoCodecs => Reply::VideoCodecs {
                codecs: core.video_codecs().map_err(core_error)?,
            },
            Call::SetVideoCodecs { codecs } => {
                core.set_video_codecs(codecs.clone()).map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }

    /// Commands that grant service permissions and inspect terminal and file jobs.
    fn services(core: &VnilinkCore, command: &Call) -> Result<Reply> {
        let reply: Reply = match command {
            Call::SharingPolicy => Reply::SharingPolicy {
                policy: core.sharing_policy().map_err(core_error)?,
            },
            Call::AdmissionRequests => Reply::AdmissionRequests {
                requests: core
                    .admission_requests()
                    .map_err(core_error)?
                    .into_iter()
                    .map(|request| TransferAdmissionRequest {
                        id: request.id.to_string(),
                        endpoint_id: request.endpoint_id,
                        remaining_ms: request.remaining_ms,
                    })
                    .collect(),
            },
            Call::AnswerAdmissionRequest { id, accept } => {
                let id = parse_id(id, "Invalid admission request ID")?;
                core.answer_admission_request(id, *accept)
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::SetSharingPolicy { policy } => {
                core.set_sharing_policy(*policy).map_err(core_error)?;
                Reply::Done
            }
            Call::SystemPolicy => {
                let policy = core.system_policy().map_err(core_error)?;
                let support = core.system_support().map_err(core_error)?.support;
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
            Call::SetSystemPolicy { policy } => {
                core.set_system_policy(*policy).map_err(core_error)?;
                Reply::Done
            }
            Call::TerminalPolicy => Reply::TerminalPolicy {
                policy: TransferTerminalPolicy::from(core.terminal_policy().map_err(core_error)?),
            },
            Call::SetTerminalPolicy { policy } => {
                core.set_terminal_policy(policy.clone().into())
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::TerminalShells => {
                let shells = core.terminal_shells().map_err(core_error)?;
                Reply::TerminalShells {
                    shells: shells
                        .shells
                        .into_iter()
                        .map(|shell| TransferShell {
                            id: shell.id,
                            label: shell.label,
                        })
                        .collect(),
                    automatic: shells.automatic,
                    current: shells.current,
                }
            }
            Call::SetTerminalShell { shell } => {
                if shell
                    .as_ref()
                    .is_some_and(|id| id.is_empty() || id.len() > MAX_SHELL_ID)
                {
                    return Err(Error::from_reason("Invalid shell"));
                }
                core.set_terminal_shell(shell.clone()).map_err(core_error)?;
                Reply::Done
            }
            Call::RemoteTerminalSessions {
                connection_id,
                generation,
            } => {
                let connection = parse_id(connection_id, "Invalid connection ID")?;
                let generation = parse_id(generation, "Invalid generation")?;
                Reply::TerminalSessions {
                    sessions: core
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
            Call::CloseRemoteTerminalSession {
                connection_id,
                generation,
                id,
            } => {
                let connection = parse_id(connection_id, "Invalid connection ID")?;
                let generation = parse_id(generation, "Invalid generation")?;
                core.close_remote_terminal_session(connection, generation, id.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            Call::HostedTerminalSessions => Reply::TerminalSessions {
                sessions: core
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
            Call::CloseTerminalSession { id } => {
                core.close_terminal_session(id.clone())
                    .map_err(core_error)?;
                Reply::Done
            }
            _ => return Err(Error::new(Status::InvalidArg, "Invalid engine command")),
        };
        Ok(reply)
    }

    /// Commands that drain the core event queue into transfer records.
    fn events(
        core: &VnilinkCore,
        subscriptions: &mut Subscriptions,
        command: &Call,
    ) -> Result<Reply> {
        let reply: Reply = match command {
            Call::SubscribeEvents => {
                let poller = core.subscribe_events().map_err(core_error)?;
                let subscription = subscriptions.next;
                subscriptions.next = subscription.saturating_add(1);
                subscriptions.events.insert(subscription, poller);
                Reply::EventSubscription {
                    subscription_id: subscription.to_string(),
                }
            }
            Call::PollEvents {
                subscription_id,
                max_events,
            } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                let poller = subscriptions
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
                    lagged: poller.take_lagged().to_string(),
                }
            }
            Call::ReleaseEvents { subscription_id } => {
                let subscription = parse_id(subscription_id, "Invalid subscription ID")?;
                subscriptions.events.remove(&subscription);
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
///
/// Only the families that touch native or System state take the `State`
/// lock; the rest run on the core alone, so a slow pairing or session command
/// does not hold the others, and event reads have a lock of their own.
struct EngineBridge {
    core: Arc<VnilinkCore>,
    state: Arc<Mutex<State>>,
    subscriptions: Arc<Mutex<Subscriptions>>,
    closed: Arc<AtomicBool>,
}

/// Takes a lock even after a command panicked while holding it. The panic is
/// contained in [`CommandTask::compute`] and reported to its caller; the
/// guarded maps stay valid because each command changes them in one step, so
/// refusing every later command would turn one failure into a dead engine.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl EngineBridge {
    fn run(&self, command: &Call) -> Result<String> {
        if self.closed.load(Ordering::Acquire) {
            return Err(core_error(FfiError::ShutDown));
        }
        let reply = self.dispatch(command)?;
        serde_json::to_string(&reply).map_err(|_| Error::from_reason("Cannot encode engine reply"))
    }

    /// Routes every command to its family. There is deliberately no fallback
    /// arm: adding a variant to `Call` fails to compile here until it is
    /// placed with its family.
    fn dispatch(&self, command: &Call) -> Result<Reply> {
        match command {
            Call::Snapshot
            | Call::DiagnosticSnapshot
            | Call::DiagnosticRecorder
            | Call::LogFilter
            | Call::SetLogFilter { .. }
            | Call::EngineLogTail
            | Call::Endpoint
            | Call::Capabilities
            | Call::VideoCodecs
            | Call::SetVideoCodecs { .. } => Self::identity(&self.core, command),
            Call::CreateInvitation { .. }
            | Call::AcceptInvitation { .. }
            | Call::ExportPairingFile { .. }
            | Call::OpenPairingFile { .. }
            | Call::OpenPairingFileBase64 { .. }
            | Call::ConfirmPairing { .. }
            | Call::Revoke { .. }
            | Call::Unpair { .. }
            | Call::SetPeerName { .. }
            | Call::ComputerName
            | Call::SetComputerName { .. }
            | Call::PeerHostname { .. } => Self::pairing(&self.core, command),
            Call::ControlRequest
            | Call::ConfirmControlRequest { .. }
            | Call::RejectControlRequest => Self::access(&self.core, command),
            Call::BeginConnectServices { .. }
            | Call::Disconnect
            | Call::DisconnectSession { .. }
            | Call::ControllerSessions
            | Call::HostedServices
            | Call::PeerAvailability { .. }
            | Call::HostedSessions
            | Call::ControllerSession { .. }
            | Call::ForgetControllerSession { .. }
            | Call::SessionPaths { .. }
            | Call::ControllerSessionPaths { .. } => Self::sessions(&self.core, command),
            Call::SetInputDefaults { .. }
            | Call::EnableFilesystemAccess
            | Call::SetTransferDirectory { .. }
            | Call::LocalPlaces
            | Call::LocateLocal { .. }
            | Call::WatchLocal { .. }
            | Call::UnwatchLocal { .. }
            | Call::ListLocal { .. }
            | Call::CreateLocalDir { .. }
            | Call::RenameLocal { .. }
            | Call::DeleteLocal { .. }
            | Call::KeyboardMode
            | Call::SetKeyboardMode { .. }
            | Call::SwapControlAndCommand
            | Call::SetSwapControlAndCommand { .. } => Self::input(&self.core, command),
            // Desktop input goes on its desktop's data channel (`data.rs`),
            // which decodes these shapes; no command carries it.
            Call::SubmitKey { .. }
            | Call::SubmitPointer { .. }
            | Call::SubmitButton { .. }
            | Call::SubmitWheel { .. }
            | Call::SubmitMotion { .. }
            | Call::SubmitPreciseWheel { .. }
            | Call::SubmitText { .. }
            | Call::SubmitLocalKey { .. }
            | Call::ReleaseSessionInput { .. }
            | Call::SelectDisplays { .. }
            | Call::SwapSessionModifiers { .. }
            | Call::CaptureSystemKeys { .. } => Err(Error::new(
                Status::InvalidArg,
                "Desktop input goes on its data channel",
            )),
            Call::SaveCustomRelays { .. }
            | Call::StartHosting
            | Call::StopHosting
            | Call::StopHostedSession
            | Call::StopHostedPeer { .. }
            | Call::ResetIdentity
            | Call::SetRelayPublic
            | Call::SetRelayCustom { .. }
            | Call::SavedRelays
            | Call::RelayRestartRequired => Self::hosting(&self.core, command),
            Call::StartNativeScreenSharing { .. }
            | Call::StopNativeScreenSharing
            | Call::NativeSharingStatus
            | Call::StartRecording { .. }
            | Call::RecordingStatus { .. }
            | Call::StopRecording { .. }
            | Call::ReleaseRecording { .. }
            | Call::RemoteCameraDevices { .. }
            | Call::RemoteAudioDevices { .. }
            | Call::NativeCameraDevices
            | Call::NativeAudioDevices
            | Call::NativeAudioOutputs
            | Call::SetNativeCameraSources { .. }
            | Call::SetNativeAudioSources { .. }
            | Call::PowerPolicy
            | Call::SetPowerPolicy { .. }
            | Call::PowerStatus
            | Call::NativeRuntime
            | Call::ReportNativePermissions { .. }
            | Call::EnableNativeInput => native::dispatch(&mut lock(&self.state), command),
            Call::Topology
            | Call::SessionTopology { .. }
            | Call::CaptureDisplays
            | Call::SetTopology { .. }
            | Call::SetDesktopCodecDefault { .. }
            | Call::SetWindowDecoders { .. }
            | Call::SetDesktopQualityDefaults { .. } => Self::media(&self.core, command),
            Call::SubscribeEvents | Call::PollEvents { .. } | Call::ReleaseEvents { .. } => {
                Self::events(&self.core, &mut lock(&self.subscriptions), command)
            }
            Call::SharingPolicy
            | Call::SetSharingPolicy { .. }
            | Call::AdmissionRequests
            | Call::AnswerAdmissionRequest { .. }
            | Call::SystemPolicy
            | Call::SetSystemPolicy { .. }
            | Call::TerminalPolicy
            | Call::SetTerminalPolicy { .. }
            | Call::TerminalShells
            | Call::SetTerminalShell { .. }
            | Call::RemoteTerminalSessions { .. }
            | Call::CloseRemoteTerminalSession { .. }
            | Call::HostedTerminalSessions
            | Call::CloseTerminalSession { .. } => Self::services(&self.core, command),
            Call::SystemAction { .. }
            | Call::SystemOperation { .. }
            | Call::CancelSystemAction { .. }
            | Call::SetSystemPrivacy { .. }
            | Call::SystemPrivacy { .. }
            | Call::ScheduledSystemActions
            | Call::CancelHostedSystemAction { .. }
            | Call::RunHostedSystemActionNow { .. } => {
                system::dispatch(&mut lock(&self.state), command)
            }
            Call::Shutdown => {
                self.closed.store(true, Ordering::Release);
                native::shutdown(&mut lock(&self.state))?;
                Ok(Reply::Done)
            }
        }
    }
}

/// How long the wake thread waits before it checks whether it was closed.
const EVENT_WATCH_CHECK: std::time::Duration = std::time::Duration::from_secs(1);

/// Tells the window host that engine events are waiting, so it reads them
/// instead of asking on a timer. It carries no events: the host reads them
/// from its subscription, which keeps the lag and resync accounting in one
/// place.
#[napi]
pub struct EventWatch {
    closed: Arc<AtomicBool>,
}
#[napi]
impl EventWatch {
    /// Stops waking the host: no wake starts after this returns. The wake
    /// thread ends within a second.
    #[napi]
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}
impl Drop for EventWatch {
    fn drop(&mut self) {
        self.close();
    }
}

#[napi]
impl Engine {
    /// Calls `callback` when engine events are waiting to be read. Calls
    /// that wait to run merge, and the callback keeps no process alive. The
    /// watch has a subscription of its own, opened before this returns, so
    /// an event after this call always wakes the host, even one a read
    /// finishes just before.
    #[napi]
    pub fn watch_events(&self, callback: Function<(), ()>) -> Result<EventWatch> {
        let wake = callback
            .build_threadsafe_function::<()>()
            .callee_handled::<false>()
            .weak::<true>()
            .max_queue_size::<1>()
            .build()?;
        let subscription = self.core.subscribe_events().map_err(core_error)?;
        let closed = Arc::new(AtomicBool::new(false));
        let stop = closed.clone();
        let engine_closed = self.closed.clone();
        std::thread::Builder::new()
            .name("event-watch".into())
            .spawn(move || {
                while !stop.load(Ordering::Acquire) && !engine_closed.load(Ordering::Acquire) {
                    // Queued events are dropped before the wake, so one that
                    // arrives later wakes the host again.
                    if subscription.wait(EVENT_WATCH_CHECK) && !stop.load(Ordering::Acquire) {
                        // Full means a wake is already waiting for this read.
                        let _queued = wake.call((), ThreadsafeFunctionCallMode::NonBlocking);
                    }
                }
            })
            .map_err(|_| Error::from_reason("Cannot watch engine events"))?;
        Ok(EventWatch { closed })
    }
}

/// Runs one command so that a panic fails it alone. napi runs `compute` in an
/// `extern "C"` callback, where an unwinding panic aborts the whole engine
/// process, taking every session with it.
fn contain(command: impl FnOnce() -> Result<String>) -> Result<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(command)).unwrap_or_else(|_| {
        tracing::error!(target: vnilink::trace::BRIDGE, "a command panicked");
        Err(Error::from_reason(
            "Internal: the engine command failed unexpectedly",
        ))
    })
}

pub struct CommandTask {
    bridge: EngineBridge,
    command: Call,
    /// Its type, which the shape check admitted from the known ones.
    kind: String,
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
            // Its type may be anything the caller sent: not recorded.
            tracing::info!(target: vnilink::trace::BRIDGE, "a command of an unknown shape was refused");
            return Err(Error::new(Status::InvalidArg, "Invalid engine command"));
        }
        let kind = raw["type"].as_str().unwrap_or_default().to_owned();
        let decoded: Call = serde_json::from_value(raw).map_err(|_| {
            tracing::info!(target: vnilink::trace::BRIDGE, command = kind.as_str(), "a command's fields were refused");
            Error::new(Status::InvalidArg, "Invalid engine command")
        })?;
        Ok(AsyncTask::new(CommandTask {
            bridge: EngineBridge {
                core: self.core.clone(),
                state: self.state.clone(),
                subscriptions: self.subscriptions.clone(),
                closed: self.closed.clone(),
            },
            command: decoded,
            kind,
        }))
    }
}
impl Task for CommandTask {
    type Output = String;
    type JsValue = String;
    /// Runs the command inside a `command` span, so what it records says
    /// which command it served; a failure is recorded by name.
    fn compute(&mut self) -> Result<String> {
        let span =
            tracing::info_span!(target: "vnilink::bridge", "command", name = self.kind.as_str());
        let _command = span.enter();
        let started = std::time::Instant::now();
        let reply = contain(|| self.bridge.run(&self.command));
        if let Err(error) = &reply {
            tracing::info!(
                target: vnilink::trace::BRIDGE,
                status = ?error.status,
                took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "command failed"
            );
        }
        reply
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
            "snapshot" | "diagnosticSnapshot" | "disconnect" | "shutdown" => &["type"],
            "diagnosticRecorder" | "logFilter" | "engineLogTail" => &["type"],
            "setLogFilter" => &["type", "directives"],
            "endpoint" | "capabilities" | "videoCodecs" | "subscribeEvents" => &["type"],
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
            "computerName" => &["type"],
            "setComputerName" => &["type", "name"],
            "disconnectSession" | "stopHostedPeer" => &["type", "sessionId"],
            "peerAvailability" => &["type", "endpointId"],
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
            "submitText" => &["type", "sessionId", "generation", "text"],
            "swapSessionModifiers" | "captureSystemKeys" => {
                &["type", "sessionId", "generation", "enabled"]
            }
            "submitLocalKey" => &[
                "type",
                "sessionId",
                "generation",
                "usage",
                "down",
                "text",
                "composing",
            ],
            "releaseSessionInput"
            | "sessionPointerSettings"
            | "sessionKeyboardMode"
            | "sessionSwapControlAndCommand" => &["type", "sessionId", "generation"],
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
                                "source",
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
            | "startHosting"
            | "stopHosting"
            | "stopHostedSession"
            | "resetIdentity"
            | "setRelayPublic"
            | "relayRestartRequired"
            | "savedRelays"
            | "controlRequest"
            | "rejectControlRequest"
            | "controllerSessions"
            | "hostedSessions"
            | "hostedServices"
            | "topology"
            | "captureDisplays" => &["type"],
            "setInputDefaults" => &["type", "mode", "swap", "pointer"],
            "enableFilesystemAccess" => &["type"],
            "setTransferDirectory" => &["type", "path"],
            "localPlaces" => &["type"],
            "locateLocal" => &["type", "path"],
            "watchLocal" => &["type", "root", "path"],
            "unwatchLocal" => &["type", "watch"],
            "listLocal" => &["type", "root", "path", "offset"],
            "createLocalDir" => &["type", "root", "path", "name"],
            "renameLocal" => &["type", "root", "path", "name", "token", "newName"],
            "deleteLocal" => &["type", "root", "path", "name", "token", "trash"],
            "setKeyboardMode" => &["type", "mode"],
            "setSwapControlAndCommand" => &["type", "enabled"],
            "setRelayCustom" => &["type", "urls"],
            "confirmControlRequest" => &["type", "digits"],
            "controllerSession"
            | "forgetControllerSession"
            | "sessionPaths"
            | "controllerSessionPaths" => &["type", "sessionId"],
            "pollEvents" => &["type", "subscriptionId", "maxEvents"],
            "releaseEvents" => &["type", "subscriptionId"],
            "sharingPolicy" | "admissionRequests" => &["type"],
            "answerAdmissionRequest" => &["type", "id", "accept"],
            "setSharingPolicy" => &["type", "policy"],
            "systemPolicy" => &["type"],
            "setSystemPolicy" => &["type", "policy"],
            "systemAction" => &["type", "sessionId", "generation", "action"],
            "systemOperation" | "cancelSystemAction" => &["type", "sessionId", "generation", "id"],
            "setSystemPrivacy" => &["type", "sessionId", "generation", "enabled"],
            "systemPrivacy" => &["type", "sessionId", "generation"],
            "scheduledSystemActions" => &["type"],
            "cancelHostedSystemAction" | "runHostedSystemActionNow" => &["type", "id"],
            "terminalPolicy" => &["type"],
            "setTerminalPolicy" => &["type", "policy"],
            "terminalShells" | "hostedTerminalSessions" => &["type"],
            "setTerminalShell" => &["type", "shell"],
            "remoteTerminalSessions" => &["type", "connectionId", "generation"],
            "closeRemoteTerminalSession" => &["type", "connectionId", "generation", "id"],
            "closeTerminalSession" => &["type", "id"],
            "sessionTopology" => &["type", "sessionId", "generation"],
            "selectDisplays" => &["type", "sessionId", "generation", "ids"],
            "setTopology" => &["type", "revision", "displays"],
            "setDesktopCodecDefault" => &["type", "codec"],
            "setWindowDecoders" => &["type", "decoders"],
            "setDesktopQualityDefaults" => &["type", "quality"],
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
        FfiError::UnprovenEntry => "UnprovenEntry",
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
        FfiError::Declined => "Declined",
        FfiError::NotAdmitted => "NotAdmitted",
        FfiError::Unanswered => "Unanswered",
        FfiError::NoAdmissionRequest => "NoAdmissionRequest",
    }
}

fn core_error(error: FfiError) -> Error {
    tracing::debug!(target: vnilink::trace::BRIDGE, failure = ffi_error_name(&error), "an engine call failed");
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

#[cfg(test)]
mod containment {
    use super::{contain, lock};
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_panicking_command_fails_alone_and_the_next_one_runs() {
        let failed = contain(|| panic!("a bug in one command"));
        assert_eq!(
            failed.unwrap_err().reason,
            "Internal: the engine command failed unexpectedly"
        );
        assert_eq!(contain(|| Ok("{}".into())).unwrap(), "{}");
    }

    #[test]
    fn a_lock_poisoned_by_a_panic_is_taken_again() {
        let shared = Arc::new(Mutex::new(1_u8));
        let poisoner = shared.clone();
        let panicked = std::thread::spawn(move || {
            let _held = poisoner.lock().unwrap();
            panic!("holding the lock");
        })
        .join();
        assert!(panicked.is_err());
        assert!(shared.is_poisoned());
        assert_eq!(*lock(&shared), 1);
    }
}
