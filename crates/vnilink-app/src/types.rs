//! Application adapter value types. Every type is records, enums, strings,
//! numbers, bytes, options, and vectors only.

use crate::error::{map_error, FfiError};

/// Directional grants. Pairing alone sets none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FfiGrants {
    pub view: bool,
    pub control: bool,
    pub clipboard: bool,
    pub transfer: bool,
}

/// What this installation can do today. Audio stays false.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FfiCapabilities {
    pub desktop_capture: bool,
    pub keyboard: bool,
    pub pointer: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub audio: bool,
    pub monitors: u8,
    pub h264_encode: bool,
    pub h264_decode: bool,
}

/// Probe-validated video abilities. Every set bit means a backend
/// initialized and processed a sample.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfiCodecSupport {
    pub codec: FfiVideoCodec,
    pub encode: bool,
    pub decode: bool,
    pub hardware: bool,
}
impl From<vnilink::CodecSupport> for FfiCodecSupport {
    fn from(value: vnilink::CodecSupport) -> Self {
        Self {
            codec: value.codec.into(),
            encode: value.encode,
            decode: value.decode,
            hardware: value.hardware,
        }
    }
}
impl From<FfiCodecSupport> for vnilink::CodecSupport {
    fn from(value: FfiCodecSupport) -> Self {
        Self {
            codec: value.codec.into(),
            encode: value.encode,
            decode: value.decode,
            hardware: value.hardware,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfiVideoCodecCaps {
    #[serde(default)]
    pub additional: Vec<FfiCodecSupport>,
    pub encode_baseline: bool,
    pub encode_high: bool,
    pub decode_baseline: bool,
    pub decode_high: bool,
    pub max_width: u16,
    pub max_height: u16,
    pub hardware: bool,
    pub device: String,
    pub chroma444: bool,
}
impl From<vnilink::VideoCodecCaps> for FfiVideoCodecCaps {
    fn from(value: vnilink::VideoCodecCaps) -> Self {
        Self {
            additional: value.additional.into_iter().map(Into::into).collect(),
            encode_baseline: value.encode_baseline,
            encode_high: value.encode_high,
            decode_baseline: value.decode_baseline,
            decode_high: value.decode_high,
            max_width: value.max_width,
            max_height: value.max_height,
            hardware: value.hardware,
            device: value.device,
            chroma444: value.chroma444,
        }
    }
}
impl From<FfiVideoCodecCaps> for vnilink::VideoCodecCaps {
    fn from(value: FfiVideoCodecCaps) -> Self {
        Self {
            additional: value.additional.into_iter().map(Into::into).collect(),
            encode_baseline: value.encode_baseline,
            encode_high: value.encode_high,
            decode_baseline: value.decode_baseline,
            decode_high: value.decode_high,
            max_width: value.max_width,
            max_height: value.max_height,
            hardware: value.hardware,
            device: value.device,
            chroma444: value.chroma444,
        }
    }
}

/// Preferred limits for the platform encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FfiQuality {
    pub max_bitrate_kbps: u32,
    pub max_frame_rate: u8,
    pub max_width: u16,
    pub max_height: u16,
}
impl From<vnilink::Quality> for FfiQuality {
    fn from(value: vnilink::Quality) -> Self {
        Self {
            max_bitrate_kbps: value.max_bitrate_kbps,
            max_frame_rate: value.max_frame_rate,
            max_width: value.max_width,
            max_height: value.max_height,
        }
    }
}

pub fn new_quality(
    max_bitrate_kbps: u32,
    max_frame_rate: u8,
    max_width: u16,
    max_height: u16,
) -> Result<FfiQuality, FfiError> {
    vnilink::Quality::new(max_bitrate_kbps, max_frame_rate, max_width, max_height)
        .map(|quality| FfiQuality {
            max_bitrate_kbps: quality.max_bitrate_kbps,
            max_frame_rate: quality.max_frame_rate,
            max_width: quality.max_width,
            max_height: quality.max_height,
        })
        .map_err(map_error)
}

/// Which side of a session this installation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FfiSessionRole {
    Controller,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum FfiSessionPhase {
    Connecting,
    Authenticated,
    Authorized,
    Negotiating,
    Active,
    Reconnecting,
    Stopping,
    Closed,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiSessionInfo {
    #[serde(serialize_with = "crate::types::serialize_u64")]
    pub id: u64,
    #[serde(serialize_with = "crate::types::serialize_u64")]
    pub generation: u64,
    pub role: FfiSessionRole,
    pub phase: FfiSessionPhase,
    pub peer_id: String,
    pub quality: Option<FfiQuality>,
    pub monitor_width: u16,
    pub monitor_height: u16,
}

/// One capability a paired computer reported, by stable name and coarse status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiCapabilityStatus {
    pub capability: String,
    pub status: String,
}

pub(crate) fn capability_name(capability: vnilink::Capability) -> &'static str {
    match capability {
        vnilink::Capability::DesktopView => "desktopView",
        vnilink::Capability::DesktopControl => "desktopControl",
        vnilink::Capability::Clipboard => "clipboard",
        vnilink::Capability::Files => "files",
        vnilink::Capability::Terminal => "terminal",
        vnilink::Capability::Camera => "camera",
        vnilink::Capability::SystemAudio => "systemAudio",
        vnilink::Capability::Microphone => "microphone",
        vnilink::Capability::SystemLock => "systemLock",
        vnilink::Capability::SystemRestart => "systemRestart",
        vnilink::Capability::SystemShutdown => "systemShutdown",
        vnilink::Capability::SystemPrivacy => "systemPrivacy",
        vnilink::Capability::Recording => "recording",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiDeviceInfo {
    pub os: Option<String>,
    pub presence: String,
    pub endpoint_id: String,
    pub hostname: Option<String>,
    pub pet_name: Option<String>,
    pub relays: Vec<String>,
    pub direct: Vec<String>,
    pub grants: FfiGrants,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiSnapshot {
    pub endpoint_id: String,
    pub public_relay: bool,
    pub relay_urls: Vec<String>,
    pub hosting: bool,
    pub devices: Vec<FfiDeviceInfo>,
    pub incoming: Option<FfiSessionInfo>,
    pub outgoing: Option<FfiSessionInfo>,
    pub capabilities: FfiCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiPairingFile {
    pub bytes: Vec<u8>,
    pub check: String,
    #[serde(serialize_with = "crate::types::serialize_u64")]
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiPairingPreview {
    /// Opaque handle for `confirm_pairing`. The invitation token never
    /// crosses the FFI.
    #[serde(serialize_with = "crate::types::serialize_u64")]
    pub id: u64,
    pub endpoint_id: String,
    pub check: String,
    #[serde(serialize_with = "crate::types::serialize_u64")]
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FfiControlRequest {
    pub endpoint_id: String,
    pub check: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiSessionPath {
    pub relay: bool,
    pub selected: bool,
    pub rtt_ms: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// One encoded video frame. Screen content: never logged.
///
/// `display_id`/`revision` attest the captured source (topology display id
/// and revision); zero means unstamped. Viewers map pointer input by this
/// id instead of guessing from picture dimensions.
#[derive(Clone, PartialEq, Eq)]
pub struct FfiEncodedFrame {
    pub codec: FfiVideoCodec,
    pub keyframe: bool,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
    pub display_id: u64,
    pub revision: u64,
}
impl FfiEncodedFrame {
    pub fn new(
        codec: FfiVideoCodec,
        keyframe: bool,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<Self, FfiError> {
        vnilink::EncodedFrame::new(codec.into(), keyframe, timestamp_us, payload)
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn h264(keyframe: bool, timestamp_us: u64, payload: Vec<u8>) -> Result<Self, FfiError> {
        Self::new(FfiVideoCodec::H264, keyframe, timestamp_us, payload)
    }
    pub fn with_source(mut self, display_id: u64, revision: u64) -> Self {
        self.display_id = display_id;
        self.revision = revision;
        self
    }
    pub(crate) fn into_core(self) -> Result<vnilink::EncodedFrame, FfiError> {
        vnilink::EncodedFrame::new(
            self.codec.into(),
            self.keyframe,
            self.timestamp_us,
            self.payload,
        )
        .map(|frame| frame.with_source(self.display_id, self.revision))
        .map_err(map_error)
    }
}
impl From<vnilink::EncodedFrame> for FfiEncodedFrame {
    fn from(value: vnilink::EncodedFrame) -> Self {
        Self {
            codec: value.codec.into(),
            keyframe: value.keyframe,
            timestamp_us: value.timestamp_us,
            payload: value.payload,
            display_id: value.display_id,
            revision: value.revision,
        }
    }
}
impl std::fmt::Debug for FfiEncodedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfiEncodedFrame")
            .field("codec", &self.codec)
            .field("keyframe", &self.keyframe)
            .field("timestamp_us", &self.timestamp_us)
            .field("display_id", &self.display_id)
            .field("revision", &self.revision)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// Stable application codec identity; domain negotiation owns preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FfiVideoCodec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
}
impl From<vnilink::VideoCodec> for FfiVideoCodec {
    fn from(value: vnilink::VideoCodec) -> Self {
        match value {
            vnilink::VideoCodec::H264 => Self::H264,
            vnilink::VideoCodec::H265 => Self::H265,
            vnilink::VideoCodec::Vp8 => Self::Vp8,
            vnilink::VideoCodec::Vp9 => Self::Vp9,
            vnilink::VideoCodec::Av1 => Self::Av1,
        }
    }
}
impl From<FfiVideoCodec> for vnilink::VideoCodec {
    fn from(value: FfiVideoCodec) -> Self {
        match value {
            FfiVideoCodec::H264 => Self::H264,
            FfiVideoCodec::H265 => Self::H265,
            FfiVideoCodec::Vp8 => Self::Vp8,
            FfiVideoCodec::Vp9 => Self::Vp9,
            FfiVideoCodec::Av1 => Self::Av1,
        }
    }
}
impl FfiVideoCodec {
    pub fn label(self) -> &'static str {
        vnilink::VideoCodec::from(self).label()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiKeyboardMode {
    Character,
    Map,
    Translate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiPointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FfiEvent {
    HostingChanged {
        hosting: bool,
    },
    DevicePaired {
        endpoint_id: String,
    },
    DeviceAuthorized {
        endpoint_id: String,
    },
    DeviceRevoked {
        endpoint_id: String,
    },
    DeviceUnpaired {
        endpoint_id: String,
    },
    ControlRequested {
        endpoint_id: String,
        check: String,
    },
    AddressesUpdated {
        endpoint_id: String,
    },
    PeerRenamed {
        endpoint_id: String,
        hostname: Option<String>,
    },
    PresenceChanged {
        endpoint_id: String,
        presence: String,
    },
    PeerAvailabilityChanged {
        endpoint_id: String,
    },
    SessionChanged {
        id: u64,
        generation: u64,
        role: FfiSessionRole,
        phase: FfiSessionPhase,
        peer_id: String,
    },
    ServiceChanged {
        service: crate::FfiServiceInfo,
    },
    HostedServiceChanged {
        service: crate::FfiServiceInfo,
    },
    QualityUpdated {
        id: u64,
        quality: FfiQuality,
    },
    InputFailed {
        id: u64,
        message: String,
    },
    KeyframeRequested {
        id: u64,
    },
    ClipboardReceived {
        id: u64,
    },
    ClipboardFilesReceived {
        id: u64,
    },
    DesktopClipboardReceived {
        service: crate::FfiServiceInfo,
    },
    TopologyChanged {
        revision: u64,
    },
    HostLocked {
        locked: bool,
    },
}

/// One display in a host snapshot. Origins are desktop pixels and may be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiDisplay {
    pub id: u64,
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u16,
    pub height: u16,
    pub rotation_deg: u16,
    pub scale_percent: u16,
    pub revision: u64,
}

pub(crate) fn grants(grants: &vnilink::Grants) -> FfiGrants {
    FfiGrants {
        view: grants.view,
        control: grants.control,
        clipboard: grants.clipboard,
        transfer: grants.transfer,
    }
}

pub(crate) fn capabilities(capabilities: &vnilink::Capabilities) -> FfiCapabilities {
    FfiCapabilities {
        desktop_capture: capabilities.desktop_capture,
        keyboard: capabilities.keyboard,
        pointer: capabilities.pointer,
        clipboard: capabilities.clipboard,
        file_transfer: capabilities.file_transfer,
        audio: capabilities.audio,
        monitors: capabilities.monitors,
        h264_encode: capabilities.h264_encode,
        h264_decode: capabilities.h264_decode,
    }
}

pub(crate) fn quality_record(quality: &vnilink::Quality) -> FfiQuality {
    (*quality).into()
}

pub(crate) fn to_quality(quality: FfiQuality) -> Result<vnilink::Quality, FfiError> {
    vnilink::Quality::new(
        quality.max_bitrate_kbps,
        quality.max_frame_rate,
        quality.max_width,
        quality.max_height,
    )
    .map_err(map_error)
}

pub(crate) fn session_role(role: vnilink::SessionRole) -> FfiSessionRole {
    match role {
        vnilink::SessionRole::Controller => FfiSessionRole::Controller,
        vnilink::SessionRole::Host => FfiSessionRole::Host,
    }
}

pub(crate) fn to_session_role(role: FfiSessionRole) -> vnilink::SessionRole {
    match role {
        FfiSessionRole::Controller => vnilink::SessionRole::Controller,
        FfiSessionRole::Host => vnilink::SessionRole::Host,
    }
}

pub(crate) fn session_phase(phase: &vnilink::SessionPhase) -> FfiSessionPhase {
    match phase {
        vnilink::SessionPhase::Connecting => FfiSessionPhase::Connecting,
        vnilink::SessionPhase::Authenticated => FfiSessionPhase::Authenticated,
        vnilink::SessionPhase::Authorized => FfiSessionPhase::Authorized,
        vnilink::SessionPhase::Negotiating => FfiSessionPhase::Negotiating,
        vnilink::SessionPhase::Active => FfiSessionPhase::Active,
        vnilink::SessionPhase::Reconnecting => FfiSessionPhase::Reconnecting,
        vnilink::SessionPhase::Stopping => FfiSessionPhase::Stopping,
        vnilink::SessionPhase::Closed => FfiSessionPhase::Closed,
        vnilink::SessionPhase::Failed(message) => FfiSessionPhase::Failed {
            message: message.clone(),
        },
    }
}

pub(crate) fn session_info(info: vnilink::SessionInfo) -> FfiSessionInfo {
    FfiSessionInfo {
        id: info.id,
        generation: info.generation,
        role: session_role(info.role),
        phase: session_phase(&info.phase),
        peer_id: info.peer_id,
        quality: info.quality.as_ref().map(quality_record),
        monitor_width: info.monitor_width,
        monitor_height: info.monitor_height,
    }
}

pub(crate) fn device_info(info: vnilink::DeviceInfo) -> FfiDeviceInfo {
    FfiDeviceInfo {
        os: info.os.map(|os| {
            match os {
                vnilink::OperatingSystem::Macos => "macos",
                vnilink::OperatingSystem::Windows => "windows",
                vnilink::OperatingSystem::Linux => "linux",
                vnilink::OperatingSystem::Other => "other",
            }
            .to_string()
        }),
        presence: presence_label(info.presence).to_string(),
        endpoint_id: info.endpoint_id,
        hostname: info.hostname,
        pet_name: info.pet_name,
        relays: info.relays,
        direct: info.direct.iter().map(|addr| addr.to_string()).collect(),
        grants: grants(&info.grants),
    }
}

pub(crate) fn snapshot(snapshot: vnilink::Snapshot) -> FfiSnapshot {
    FfiSnapshot {
        endpoint_id: snapshot.endpoint_id,
        public_relay: snapshot.public_relay,
        relay_urls: snapshot.relay_urls,
        hosting: snapshot.hosting,
        devices: snapshot.devices.into_iter().map(device_info).collect(),
        incoming: snapshot.incoming.map(session_info),
        outgoing: snapshot.outgoing.map(session_info),
        capabilities: capabilities(&snapshot.capabilities),
    }
}

pub(crate) fn pairing_file(file: vnilink::PairingFile) -> FfiPairingFile {
    FfiPairingFile {
        bytes: file.bytes,
        check: file.check,
        expires_at_ms: file.expires_at_ms,
    }
}

pub(crate) fn pairing_preview(id: u64, preview: &vnilink::PairingPreview) -> FfiPairingPreview {
    FfiPairingPreview {
        id,
        endpoint_id: preview.endpoint_id.clone(),
        check: preview.check.clone(),
        expires_at_ms: preview.expires_at_ms,
    }
}

pub(crate) fn control_request(request: vnilink::ControlRequest) -> FfiControlRequest {
    FfiControlRequest {
        endpoint_id: request.endpoint_id,
        check: request.check,
    }
}

pub(crate) fn session_path(path: &vnilink::SessionPath) -> FfiSessionPath {
    FfiSessionPath {
        relay: path.relay,
        selected: path.selected,
        rtt_ms: path.rtt_ms,
        tx_bytes: path.tx_bytes,
        rx_bytes: path.rx_bytes,
    }
}

pub(crate) fn keyboard_mode(mode: vnilink::KeyboardMode) -> FfiKeyboardMode {
    match mode {
        vnilink::KeyboardMode::Character => FfiKeyboardMode::Character,
        vnilink::KeyboardMode::Map => FfiKeyboardMode::Map,
        vnilink::KeyboardMode::Translate => FfiKeyboardMode::Translate,
    }
}

pub(crate) fn to_keyboard_mode(mode: FfiKeyboardMode) -> vnilink::KeyboardMode {
    match mode {
        FfiKeyboardMode::Character => vnilink::KeyboardMode::Character,
        FfiKeyboardMode::Map => vnilink::KeyboardMode::Map,
        FfiKeyboardMode::Translate => vnilink::KeyboardMode::Translate,
    }
}

pub(crate) fn presence_label(presence: vnilink::DevicePresence) -> &'static str {
    match presence {
        vnilink::DevicePresence::Connected => "connected",
        vnilink::DevicePresence::Connecting => "connecting",
        vnilink::DevicePresence::Reconnecting => "reconnecting",
        vnilink::DevicePresence::NotAccepting => "notAccepting",
        vnilink::DevicePresence::Disconnected => "disconnected",
    }
}

pub(crate) fn event(event: vnilink::CoreEvent) -> FfiEvent {
    match event {
        vnilink::CoreEvent::PresenceChanged {
            endpoint_id,
            presence,
        } => FfiEvent::PresenceChanged {
            endpoint_id,
            presence: presence_label(presence).to_string(),
        },
        vnilink::CoreEvent::PeerAvailabilityChanged { endpoint_id } => {
            FfiEvent::PeerAvailabilityChanged { endpoint_id }
        }
        vnilink::CoreEvent::HostingChanged { hosting } => FfiEvent::HostingChanged { hosting },
        vnilink::CoreEvent::DevicePaired { endpoint_id } => FfiEvent::DevicePaired { endpoint_id },
        vnilink::CoreEvent::DeviceAuthorized { endpoint_id } => {
            FfiEvent::DeviceAuthorized { endpoint_id }
        }
        vnilink::CoreEvent::DeviceRevoked { endpoint_id } => {
            FfiEvent::DeviceRevoked { endpoint_id }
        }
        vnilink::CoreEvent::DeviceUnpaired { endpoint_id } => {
            FfiEvent::DeviceUnpaired { endpoint_id }
        }
        vnilink::CoreEvent::ControlRequested { endpoint_id, check } => {
            FfiEvent::ControlRequested { endpoint_id, check }
        }
        vnilink::CoreEvent::AddressesUpdated { endpoint_id } => {
            FfiEvent::AddressesUpdated { endpoint_id }
        }
        vnilink::CoreEvent::PeerRenamed {
            endpoint_id,
            hostname,
        } => FfiEvent::PeerRenamed {
            endpoint_id,
            hostname,
        },
        vnilink::CoreEvent::SessionChanged {
            id,
            generation,
            role,
            phase,
            peer_id,
        } => FfiEvent::SessionChanged {
            id,
            generation,
            role: session_role(role),
            phase: session_phase(&phase),
            peer_id,
        },
        vnilink::CoreEvent::ServiceChanged { service } => FfiEvent::ServiceChanged {
            service: service.into(),
        },
        vnilink::CoreEvent::HostedServiceChanged { service } => FfiEvent::HostedServiceChanged {
            service: service.into(),
        },
        vnilink::CoreEvent::QualityUpdated { id, quality } => FfiEvent::QualityUpdated {
            id,
            quality: quality_record(&quality),
        },
        vnilink::CoreEvent::InputFailed { id, message } => FfiEvent::InputFailed {
            id,
            message: message.to_string(),
        },
        vnilink::CoreEvent::KeyframeRequested { id } => FfiEvent::KeyframeRequested { id },
        vnilink::CoreEvent::ClipboardReceived { id } => FfiEvent::ClipboardReceived { id },
        vnilink::CoreEvent::ClipboardFilesReceived { id } => {
            FfiEvent::ClipboardFilesReceived { id }
        }
        vnilink::CoreEvent::DesktopClipboardReceived { service } => {
            FfiEvent::DesktopClipboardReceived {
                service: service.into(),
            }
        }
        vnilink::CoreEvent::TopologyChanged { revision } => FfiEvent::TopologyChanged { revision },
        vnilink::CoreEvent::HostLocked { locked } => FfiEvent::HostLocked { locked },
    }
}

pub(crate) fn display_record(display: &vnilink::DisplayInfo) -> FfiDisplay {
    FfiDisplay {
        id: display.id,
        origin_x: display.origin_x,
        origin_y: display.origin_y,
        width: display.width,
        height: display.height,
        rotation_deg: display.rotation_deg,
        scale_percent: display.scale_percent,
        revision: display.revision,
    }
}

pub(crate) fn display_info(display: &FfiDisplay) -> vnilink::DisplayInfo {
    vnilink::DisplayInfo {
        id: display.id,
        origin_x: display.origin_x,
        origin_y: display.origin_y,
        width: display.width,
        height: display.height,
        rotation_deg: display.rotation_deg,
        scale_percent: display.scale_percent,
        revision: display.revision,
    }
}

/// Per-owner pointer policy; 100 percent leaves relative device counts unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfiPointerSettings {
    pub sensitivity_percent: u16,
    pub reverse_wheel: bool,
    pub swap_buttons: bool,
}
impl From<FfiPointerSettings> for vnilink::PointerSettings {
    fn from(value: FfiPointerSettings) -> Self {
        Self {
            sensitivity_percent: value.sensitivity_percent,
            reverse_wheel: value.reverse_wheel,
            swap_buttons: value.swap_buttons,
        }
    }
}
impl From<vnilink::PointerSettings> for FfiPointerSettings {
    fn from(value: vnilink::PointerSettings) -> Self {
        Self {
            sensitivity_percent: value.sensitivity_percent,
            reverse_wheel: value.reverse_wheel,
            swap_buttons: value.swap_buttons,
        }
    }
}

fn serialize_u64<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

#[cfg(test)]
mod bridge_serialization_tests {
    use super::*;
    #[test]
    fn facade_codec_values_preserve_wire_shapes_and_all_probe_fields() {
        let mut caps = vnilink::VideoCodecCaps::h264_full(4096, 2160, "probe");
        caps.chroma444 = true;
        caps.hardware = true;
        caps.additional = vnilink::VideoCodec::ALL
            .into_iter()
            .map(|codec| vnilink::CodecSupport {
                codec,
                encode: true,
                decode: false,
                hardware: false,
            })
            .collect();
        let facade: FfiVideoCodecCaps = caps.clone().into();
        assert_eq!(
            serde_json::to_value(&facade).unwrap(),
            serde_json::to_value(&caps).unwrap()
        );
        assert_eq!(vnilink::VideoCodecCaps::from(facade), caps);
    }

    #[test]
    fn facade_frames_preserve_source_and_revalidate_mutable_payload_without_logging_it() {
        for codec in vnilink::VideoCodec::ALL {
            let original =
                vnilink::EncodedFrame::new(codec, true, u64::MAX, b"private pixels".to_vec())
                    .unwrap()
                    .with_source(u64::MAX, u64::MAX);
            let mut facade: FfiEncodedFrame = original.clone().into();
            let debug = format!("{facade:?}");
            assert!(debug.contains("payload_len"));
            assert!(!debug.contains("payload:"));
            assert_eq!(facade.clone().into_core().unwrap(), original);
            facade.payload.clear();
            assert!(matches!(
                facade.into_core(),
                Err(FfiError::InvalidSetting { .. })
            ));
        }
    }

    #[test]
    fn facade_policy_defaults_and_round_trips_preserve_core_meanings() {
        let system = vnilink::SystemPolicy::default();
        let facade = crate::FfiSystemPolicy::default();
        assert_eq!(vnilink::SystemPolicy::from(facade), system);
        assert!(facade.allow_lock);
        let power = vnilink::power::PowerPolicy {
            incoming_display: true,
            outgoing_system: true,
            incoming_idle_secs: 86400,
            outgoing_idle_secs: 1,
            lock_after_control: true,
            ..Default::default()
        };
        let facade: crate::FfiPowerPolicy = power.into();
        assert_eq!(
            serde_json::to_value(facade).unwrap(),
            serde_json::to_value(power).unwrap()
        );
        assert_eq!(vnilink::power::PowerPolicy::from(facade), power);
        assert_eq!(
            vnilink::power::PowerPolicy::from(crate::FfiPowerPolicy::default()),
            vnilink::power::PowerPolicy::default()
        );
        assert!(matches!(
            to_quality(FfiQuality {
                max_bitrate_kbps: 0,
                max_frame_rate: 30,
                max_width: 1920,
                max_height: 1080
            }),
            Err(FfiError::InvalidSetting { .. })
        ));
    }

    #[test]
    fn pairing_handles_and_expiry_are_lossless_strings() {
        let preview = FfiPairingPreview {
            id: u64::MAX,
            endpoint_id: "peer".into(),
            check: "123".into(),
            expires_at_ms: u64::MAX,
        };
        let value = serde_json::to_value(preview).unwrap();
        assert_eq!(value["id"], u64::MAX.to_string());
        assert_eq!(value["expires_at_ms"], u64::MAX.to_string());
    }
    #[test]
    fn session_ownership_is_lossless_across_json() {
        let session = FfiSessionInfo {
            id: u64::MAX,
            generation: u64::MAX,
            role: FfiSessionRole::Controller,
            phase: FfiSessionPhase::Active,
            peer_id: "peer".into(),
            quality: None,
            monitor_width: 0,
            monitor_height: 0,
        };
        let value = serde_json::to_value(session).unwrap();
        assert_eq!(value["id"], u64::MAX.to_string());
        assert_eq!(value["generation"], u64::MAX.to_string());
    }
}

/// Saved codec preference for new outgoing Desktop owners.
#[derive(Debug, Clone, Copy)]
pub enum FfiCodecPreference {
    /// Choose a common codec automatically.
    Auto,
    /// Require H.264.
    H264,
    /// Require H.265.
    H265,
    /// Require VP8.
    Vp8,
    /// Require VP9.
    Vp9,
    /// Require AV1.
    Av1,
}
impl From<FfiCodecPreference> for Option<vnilink::VideoCodec> {
    fn from(value: FfiCodecPreference) -> Self {
        match value {
            FfiCodecPreference::Auto => None,
            FfiCodecPreference::H264 => Some(vnilink::VideoCodec::H264),
            FfiCodecPreference::H265 => Some(vnilink::VideoCodec::H265),
            FfiCodecPreference::Vp8 => Some(vnilink::VideoCodec::Vp8),
            FfiCodecPreference::Vp9 => Some(vnilink::VideoCodec::Vp9),
            FfiCodecPreference::Av1 => Some(vnilink::VideoCodec::Av1),
        }
    }
}
