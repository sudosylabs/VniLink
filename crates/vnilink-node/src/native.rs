//! Native runtime, explicit source selection, and owned capture/recording resources.
use super::{core_error, parse_id, session_target, Command, Reply, State};
use crate::data;
use napi::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Serialize)]
pub(super) struct TransferNativeBackend {
    name: String,
    ready: bool,
    error: Option<String>,
}
impl From<vnilink_app::FfiNativeBackend> for TransferNativeBackend {
    fn from(value: vnilink_app::FfiNativeBackend) -> Self {
        Self {
            name: value.name,
            ready: value.ready,
            error: value.error,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferSharingStatus {
    display_id: u32,
    phase: String,
    error: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TransferRecordingRetention {
    max_segments: u32,
    max_total_bytes: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TransferRecordingStatus {
    id: String,
    phase: String,
    failure: Option<String>,
    frames: String,
    bytes: String,
    duration_ms: String,
    segments: u32,
}
pub(super) struct RecordingOwner {
    handle: Arc<vnilink_app::RecordingHandle>,
    _source: RecordedService,
}
/// Keeps a desktop recording's service open; a camera recording follows its tab's stream.
enum RecordedService {
    Desktop { _owner: Arc<data::DesktopOwner> },
    Camera,
}
fn recording_status(id: u64, handle: &vnilink_app::RecordingHandle) -> TransferRecordingStatus {
    let status = handle.status();
    TransferRecordingStatus {
        id: id.to_string(),
        phase: format!("{:?}", status.phase),
        failure: status.failure.map(|f| format!("{:?}", f)),
        frames: status.frames.to_string(),
        bytes: status.bytes.to_string(),
        duration_ms: status.duration_ms.to_string(),
        segments: status.segments,
    }
}

#[derive(Serialize)]
pub(super) struct TransferNativeDevice {
    id: String,
    name: String,
    kind: String,
}
pub(super) use vnilink_app::FfiPowerPolicy as TransferPowerPolicy;
#[derive(Serialize)]
pub(super) struct TransferPowerStatus {
    kind: String,
    owners: u32,
    phase: String,
    error: Option<String>,
}

#[derive(Serialize)]
pub(super) struct TransferCapability {
    name: String,
    available: bool,
    summary: String,
}

pub(super) struct NativeRuntimeState {
    backends: Vec<TransferNativeBackend>,
    desktops: Arc<data::DesktopChannels>,
    cameras: Arc<data::CameraChannels>,
    recordings: HashMap<u64, RecordingOwner>,
    next_recording: u64,
    #[cfg(target_os = "macos")]
    screen_hosts: HashMap<u32, vnilink_app::MacHost>,
    /// The primary output; Windows hosts one desktop for every viewer.
    #[cfg(windows)]
    windows_host: Option<(u32, vnilink_app::WindowsHost)>,
    /// The monitor chosen in the portal dialog, served to every viewer.
    #[cfg(target_os = "linux")]
    linux_host: Option<(u32, vnilink_app::LinuxHost)>,
    /// Where the host keeps the portal's grant between starts.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    screen_grant: Option<std::path::PathBuf>,
    input_ready: bool,
}
impl NativeRuntimeState {
    pub(super) fn new(
        backends: Vec<TransferNativeBackend>,
        desktops: Arc<data::DesktopChannels>,
        cameras: Arc<data::CameraChannels>,
        screen_grant: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            backends,
            desktops,
            cameras,
            recordings: HashMap::new(),
            next_recording: 1,
            #[cfg(target_os = "macos")]
            screen_hosts: HashMap::new(),
            #[cfg(windows)]
            windows_host: None,
            #[cfg(target_os = "linux")]
            linux_host: None,
            screen_grant,
            input_ready: false,
        }
    }
}
pub(super) fn dispatch(state: &mut State, command: &Command) -> Result<Reply> {
    match command {
        Command::StartNativeScreenSharing {
            display_id,
            width,
            height,
            pixel_width,
            pixel_height,
        } => {
            #[cfg(target_os = "macos")]
            {
                if state.native.screen_hosts.contains_key(display_id) {
                    return Err(Error::from_reason("This display is already being shared"));
                }
                if state.native.screen_hosts.len() >= 16 {
                    return Err(Error::from_reason("Too many shared displays"));
                }
                let host = state
                    .core
                    .mac_host(
                        *display_id,
                        *width,
                        *height,
                        *pixel_width,
                        *pixel_height,
                        // The window draws the pointer itself.
                        true,
                    )
                    .map_err(core_error)?;
                // The host idles until a viewer connects; capture starts for
                // that viewer, so there is nothing to wait for here. Waiting
                // blocked the engine for 15 seconds whenever no one watched.
                if let Some(error) = host.error() {
                    host.stop();
                    return Err(Error::from_reason(error));
                }
                state.native.screen_hosts.insert(*display_id, host);
                Ok(Reply::Done)
            }
            #[cfg(windows)]
            {
                // Desktop Duplication reads the output itself; the geometry only names it.
                let _ = (width, height, pixel_width, pixel_height);
                if state.native.windows_host.is_none() {
                    let host = state.core.windows_host().map_err(core_error)?;
                    state.native.windows_host = Some((*display_id, host));
                }
                Ok(Reply::Done)
            }
            #[cfg(target_os = "linux")]
            {
                // The portal dialog chooses the monitor; the geometry only names it.
                let _ = (width, height, pixel_width, pixel_height);
                // A declined or ended grant is asked for again; a live or
                // pending one is kept, so no second dialog appears.
                if state
                    .native
                    .linux_host
                    .as_ref()
                    .is_none_or(|(_, host)| host.has_ended())
                {
                    let host = state
                        .core
                        .linux_host(state.native.screen_grant.clone())
                        .map_err(core_error)?;
                    state.native.linux_host = Some((*display_id, host));
                }
                Ok(Reply::Done)
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
            {
                let _ = (display_id, width, height, pixel_width, pixel_height);
                Err(Error::from_reason(
                    "Native screen sharing is not connected on this platform",
                ))
            }
        }
        Command::StopNativeScreenSharing => {
            #[cfg(target_os = "macos")]
            {
                state.core.stop_mac_desktop_sharing().map_err(core_error)?;
                for (_, host) in state.native.screen_hosts.drain() {
                    host.stop();
                }
            }
            #[cfg(windows)]
            {
                state.native.windows_host = None;
            }
            #[cfg(target_os = "linux")]
            {
                // Closes the portal session, and with it the grant.
                state.native.linux_host = None;
            }
            #[cfg(any(target_os = "macos", target_os = "linux", windows))]
            {
                Ok(Reply::Done)
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
            {
                Err(Error::from_reason(
                    "Native screen sharing is not connected on this platform",
                ))
            }
        }
        Command::NativeSharingStatus => {
            #[cfg(target_os = "macos")]
            let displays = state
                .native
                .screen_hosts
                .iter()
                .map(|(id, host)| TransferSharingStatus {
                    display_id: *id,
                    phase: if host.error().is_some() {
                        "Failed"
                    } else if host.is_started() {
                        "Sharing"
                    } else {
                        "Starting"
                    }
                    .into(),
                    error: host.error(),
                })
                .collect();
            #[cfg(windows)]
            let displays = state
                .native
                .windows_host
                .iter()
                .map(|(id, host)| TransferSharingStatus {
                    display_id: *id,
                    phase: if host.error().is_some() {
                        "Failed"
                    } else {
                        "Sharing"
                    }
                    .into(),
                    error: host.error(),
                })
                .collect();
            #[cfg(target_os = "linux")]
            let displays = state
                .native
                .linux_host
                .iter()
                .map(|(id, host)| TransferSharingStatus {
                    display_id: *id,
                    phase: if host.error().is_some() {
                        "Failed"
                    } else if host.is_started() {
                        "Sharing"
                    } else {
                        "Starting"
                    }
                    .into(),
                    error: host.error(),
                })
                .collect();
            #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
            let displays = vec![];
            Ok(Reply::NativeSharingStatus { displays })
        }
        Command::StartRecording {
            session_id,
            generation,
            destination,
            max_bytes,
            max_duration_ms,
            retention,
            source,
        } => {
            if state.native.recordings.len() >= 16 {
                return Err(Error::from_reason("Too many recordings"));
            }
            if destination.is_empty() || destination.len() > 4096 || destination.contains('\0') {
                return Err(Error::from_reason("Invalid recording destination"));
            }
            let (session, generation) = session_target(session_id, generation)?;
            let limits = vnilink_app::FfiRecordingLimits {
                max_bytes: parse_id(max_bytes, "Invalid recording byte limit")?,
                max_duration_ms: parse_id(max_duration_ms, "Invalid recording duration limit")?,
            };
            let retention = retention
                .as_ref()
                .map(|r| {
                    Ok::<_, napi::Error>(vnilink_app::FfiRecordingRetention {
                        max_segments: r.max_segments,
                        max_total_bytes: parse_id(
                            &r.max_total_bytes,
                            "Invalid retention byte limit",
                        )?,
                    })
                })
                .transpose()?;
            let id = state.native.next_recording;
            let next = id
                .checked_add(1)
                .ok_or_else(|| Error::from_reason("Recording IDs exhausted"))?;
            let (handle, source) = if *source == Some(crate::RecordingSource::Camera) {
                let camera = state.native.cameras.get(session, generation)?;
                let handle = state
                    .core
                    .start_camera_recording(&camera, destination.clone(), limits, retention)
                    .map_err(core_error)?;
                (handle, RecordedService::Camera)
            } else {
                let desktop = state
                    .native
                    .desktops
                    .open(&state.core, session, generation)?;
                let handle = state
                    .core
                    .start_desktop_recording(
                        desktop.service.clone(),
                        destination.clone(),
                        limits,
                        retention,
                    )
                    .map_err(core_error)?;
                (handle, RecordedService::Desktop { _owner: desktop })
            };
            let status = recording_status(id, &handle);
            state.native.recordings.insert(
                id,
                RecordingOwner {
                    handle,
                    _source: source,
                },
            );
            state.native.next_recording = next;
            Ok(Reply::RecordingStatus { status })
        }
        Command::RecordingStatus { id } => {
            let id = parse_id(id, "Invalid recording ID")?;
            let recording = state
                .native
                .recordings
                .get(&id)
                .ok_or_else(|| Error::from_reason("Unknown recording"))?;
            Ok(Reply::RecordingStatus {
                status: recording_status(id, &recording.handle),
            })
        }
        Command::StopRecording { id } => {
            let id = parse_id(id, "Invalid recording ID")?;
            let recording = state
                .native
                .recordings
                .get(&id)
                .ok_or_else(|| Error::from_reason("Unknown recording"))?;
            recording.handle.stop();
            Ok(Reply::RecordingStatus {
                status: recording_status(id, &recording.handle),
            })
        }
        Command::ReleaseRecording { id } => {
            let id = parse_id(id, "Invalid recording ID")?;
            let recording = state
                .native
                .recordings
                .get(&id)
                .ok_or_else(|| Error::from_reason("Unknown recording"))?;
            if matches!(
                recording.handle.status().phase,
                vnilink_app::FfiRecordingPhase::Starting
                    | vnilink_app::FfiRecordingPhase::Recording
            ) {
                return Err(Error::from_reason(
                    "Stop and finalize the recording before releasing it",
                ));
            }
            state.native.recordings.remove(&id);
            Ok(Reply::Done)
        }
        Command::RemoteCameraDevices {
            session_id,
            generation,
        } => {
            let (id, generation) = session_target(session_id, generation)?;
            let devices = state
                .core
                .remote_camera_devices(id, generation)
                .map_err(core_error)?
                .into_iter()
                .map(|d| TransferNativeDevice {
                    id: d.id,
                    name: d.name,
                    kind: "camera".into(),
                })
                .collect();
            Ok(Reply::NativeDevices { devices })
        }
        Command::RemoteAudioDevices {
            session_id,
            generation,
            kind,
        } => {
            let (id, generation) = session_target(session_id, generation)?;
            let kind = match kind.as_str() {
                "Microphone" => vnilink_app::FfiAudioKind::Microphone,
                "System" => vnilink_app::FfiAudioKind::System,
                _ => return Err(Error::from_reason("Invalid audio kind")),
            };
            let devices = state
                .core
                .remote_audio_devices(id, generation, kind)
                .map_err(core_error)?
                .into_iter()
                .map(|d| TransferNativeDevice {
                    id: d.id,
                    name: d.name,
                    kind: match d.kind {
                        vnilink_app::FfiAudioKind::Microphone => "microphone",
                        vnilink_app::FfiAudioKind::System => "system",
                    }
                    .into(),
                })
                .collect();
            Ok(Reply::NativeDevices { devices })
        }
        Command::NativeCameraDevices => {
            let devices = state
                .core
                .native_camera_devices()
                .map_err(core_error)?
                .into_iter()
                .map(|d| TransferNativeDevice {
                    id: d.id,
                    name: d.name,
                    kind: "camera".into(),
                })
                .collect();
            Ok(Reply::NativeDevices { devices })
        }
        Command::NativeAudioDevices => {
            let devices = state
                .core
                .native_audio_devices()
                .map_err(core_error)?
                .into_iter()
                .map(|d| TransferNativeDevice {
                    id: d.id,
                    name: d.name,
                    kind: match d.kind {
                        vnilink_app::FfiAudioKind::Microphone => "microphone",
                        vnilink_app::FfiAudioKind::System => "system",
                    }
                    .into(),
                })
                .collect();
            Ok(Reply::NativeDevices { devices })
        }
        Command::NativeAudioOutputs => {
            let devices = state
                .core
                .native_audio_output_devices()
                .map_err(core_error)?
                .into_iter()
                .map(|d| TransferNativeDevice {
                    id: d.id,
                    name: d.name,
                    kind: "output".into(),
                })
                .collect();
            Ok(Reply::NativeDevices { devices })
        }
        Command::SetNativeCameraSources { ids } => {
            if ids.len() > 32 {
                return Err(Error::from_reason("Too many approved cameras"));
            }
            if ids.is_empty() {
                state.core.disable_camera().map_err(core_error)?;
            } else {
                state
                    .core
                    .enable_native_camera(ids.clone())
                    .map_err(core_error)?;
            }
            Ok(Reply::Done)
        }
        Command::SetNativeAudioSources {
            microphones,
            system,
        } => {
            if microphones.is_empty() && system.is_empty() {
                state.core.set_audio_provider(None).map_err(core_error)?;
            } else {
                state
                    .core
                    .enable_native_audio(microphones.clone(), system.clone())
                    .map_err(core_error)?;
            }
            Ok(Reply::Done)
        }
        Command::PowerPolicy => Ok(Reply::PowerPolicy {
            policy: state.core.power_policy().map_err(core_error)?.into(),
        }),
        Command::SetPowerPolicy { policy } => {
            state
                .core
                .set_power_policy(policy.clone().into())
                .map_err(core_error)?;
            Ok(Reply::Done)
        }
        Command::PowerStatus => {
            use vnilink_app::{FfiPowerKind as K, FfiPowerPhase as P};
            let statuses = state
                .core
                .power_status()
                .map_err(core_error)?
                .into_iter()
                .map(|s| {
                    let (phase, error) = match s.phase {
                        P::Idle => ("Idle", None),
                        P::Unavailable => ("Unavailable", None),
                        P::Starting => ("Starting", None),
                        P::Active => ("Active", None),
                        P::Failed { .. } => {
                            ("Failed", Some("The OS rejected power inhibition".into()))
                        }
                    };
                    TransferPowerStatus {
                        kind: match s.kind {
                            K::Display => "Display",
                            K::System => "System",
                        }
                        .into(),
                        owners: s.owners,
                        phase: phase.into(),
                        error,
                    }
                })
                .collect();
            Ok(Reply::PowerStatus { statuses })
        }
        Command::NativeRuntime => {
            use vnilink_app::FfiCapability as C;
            let entries = [
                ("desktopView", C::DesktopView),
                ("desktopControl", C::DesktopControl),
                ("clipboard", C::Clipboard),
                ("files", C::Files),
                ("terminal", C::Terminal),
                ("camera", C::Camera),
                ("systemAudio", C::SystemAudio),
                ("microphone", C::Microphone),
                ("lock", C::SystemLock),
                ("restart", C::SystemRestart),
                ("shutdown", C::SystemShutdown),
                ("privacy", C::SystemPrivacy),
                ("recording", C::Recording),
            ];
            #[cfg(target_os = "macos")]
            if !state.native.screen_hosts.is_empty() {
                let failed = !state
                    .native
                    .screen_hosts
                    .values()
                    // An idle host has not started capture yet; only an error is a failure.
                    .any(|host| host.error().is_none());
                state
                    .core
                    .report_backend_failure(
                        C::DesktopView,
                        failed.then(|| "Native screen capture stopped".into()),
                    )
                    .map_err(core_error)?;
            }
            let capabilities = entries
                .into_iter()
                .map(|(name, capability)| {
                    let report = state
                        .core
                        .capability_availability(None, capability)
                        .map_err(core_error)?;
                    Ok(TransferCapability {
                        name: name.into(),
                        available: report.available,
                        summary: report.summary,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let backends = state
                .native
                .backends
                .iter()
                .map(|b| TransferNativeBackend {
                    name: b.name.clone(),
                    ready: b.ready,
                    error: b.error.clone(),
                })
                .collect();
            Ok(Reply::NativeRuntime {
                backends,
                capabilities,
            })
        }
        Command::ReportNativePermissions {
            screen,
            input,
            camera,
            microphone,
        } => {
            use vnilink_app::FfiCapability as C;
            for (capability, allowed) in [
                (C::DesktopView, screen),
                (C::DesktopControl, input),
                (C::Camera, camera),
                (C::Microphone, microphone),
            ] {
                state
                    .core
                    .report_os_permission(capability, !*allowed)
                    .map_err(core_error)?;
            }
            Ok(Reply::Done)
        }
        Command::EnableNativeInput => {
            // On Linux the portal grant carries input; X11 injection would
            // reach only X clients under Wayland.
            #[cfg(target_os = "linux")]
            if state.native.linux_host.is_some() {
                return Ok(Reply::Done);
            }
            if !state.native.input_ready {
                let result = state.core.enable_native_input_without_prompt();
                state.native.backends.retain(|b| b.name != "input");
                state.native.backends.push(TransferNativeBackend { name: "input".into(), ready: result.is_ok(), error: result.as_ref().err().map(|_| "Native input could not be initialized; check OS permissions and the desktop session".into()) });
                result.map_err(core_error)?;
                state.native.input_ready = true;
            }
            Ok(Reply::Done)
        }
        _ => Err(Error::from_reason("Invalid native runtime command")),
    }
}
pub(super) fn shutdown(state: &mut State) -> Result<()> {
    for recording in state.native.recordings.values() {
        recording.handle.stop();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while state.native.recordings.values().any(|recording| {
        matches!(
            recording.handle.status().phase,
            vnilink_app::FfiRecordingPhase::Starting | vnilink_app::FfiRecordingPhase::Recording
        )
    }) {
        if std::time::Instant::now() >= deadline {
            state.core.shutdown();
            return Err(Error::from_reason("Recording finalization timed out"));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    state.native.recordings.clear();
    #[cfg(target_os = "macos")]
    for (_, host) in state.native.screen_hosts.drain() {
        host.stop();
    }
    #[cfg(target_os = "linux")]
    {
        state.native.linux_host = None;
    }
    state.core.shutdown();
    Ok(())
}
