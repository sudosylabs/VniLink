//! Application adapters for the VniLink shared Rust core.
//!
//! This crate preserves the platform coordination, control API and regression
//! coverage formerly hosted by the native-app binding layer. It is ordinary Rust;
//! language bindings belong in a separate adapter crate.
//!
//! # Threading
//!
//! Call blocking methods from background threads. The core owns a Tokio runtime;
//! calling its blocking control methods from an async executor is unsupported.
//! Persistent installations require a [`CredentialStore`] before opening the
//! core. That adapter supplies the installation record; it is not itself the
//! platform credential store. Store records contain secrets and must never be
//! logged or exposed to the UI.
//!
//! Media adapters remain available to Rust callers. Electron media transport and
//! presentation are deliberately outside the current control-bridge work.

mod audio;
mod audio_native;
mod audio_output;
mod desktop_audio;
pub use desktop_audio::{DesktopAudio, DesktopAudioPhase, DesktopAudioStatus};
#[cfg(test)]
mod audio_tests;
mod native;
mod native_clipboard;
pub use native::FfiNativeBackend;
mod availability;
pub use audio_native::NativeAudioPlayback;
mod camera;
pub use audio::{
    AudioProvider, AudioService, AudioSource, FfiAudioBlock, FfiAudioConfig, FfiAudioDevice,
    FfiAudioKind, FfiAudioPacket, FfiAudioPlaybackStats,
};
pub use availability::{
    FfiAvailability, FfiAvailabilityReason, FfiBackendState, FfiCapability, FfiPolicyChangeScope,
    FfiPolicyDescriptor,
};
pub use camera::{
    BackendStartup, CameraProvider, CameraService, CameraSource, FfiCameraConfig, FfiCameraDevice,
    FfiCameraEnd, ALL_CAMERAS,
};
mod clipboard;
pub use clipboard::{
    ClipboardWriter, FfiClipboardFileEntry, FfiFileSelection, FfiStoredFileSelection,
};
mod error;
mod events;
mod files;
#[cfg(target_os = "macos")]
mod macos;
mod power;
mod recording;
#[cfg(windows)]
mod windows;
pub use power::{FfiPowerKind, FfiPowerPhase, FfiPowerPolicy, FfiPowerStatus};
pub use recording::{
    recover_desktop_recording, FfiRecordingFailure, FfiRecordingLimits, FfiRecordingPhase,
    FfiRecordingRecovery, FfiRecordingRetention, FfiRecordingStatus, RecordingHandle,
};
mod services;
mod snapshot;
pub use snapshot::{snapshot_error_outcome, FfiScreenAccess, FfiScreenSnapshot};
mod system;
mod terminal;
pub use services::{
    DesktopService, FfiAdaptationReason, FfiCloseCause, FfiDesktopControlState, FfiDesktopCursor,
    FfiDesktopCursorImage, FfiDesktopInfo, FfiDesktopMediaStatus, FfiServiceInfo, FfiServiceKind,
    FfiServicePermissions, FfiServicePhase, FfiServiceSupport, FfiSharingPolicy,
    FfiSharingPolicyUpdate, FilesService, ServiceLeasePoller,
};
pub use system::{
    FfiHostSystemReport, FfiOperationState, FfiPrivacyState, FfiScheduledSystemAction,
    FfiSessionState, FfiSystemAction, FfiSystemOperation, FfiSystemPolicy, FfiSystemSupport,
    SystemServiceHandle,
};
pub use terminal::{
    FfiShell, FfiTerminalEvent, FfiTerminalPolicy, FfiTerminalSessionInfo, FfiTerminalShells,
    FfiTerminalSize, TerminalService,
};
mod storage;
mod types;
#[cfg(target_os = "macos")]
pub use macos::{MacFrame, MacHost, MacViewer};
pub use storage::{CredentialStore, FfiRelay, FfiRelayInput};
#[cfg(windows)]
pub use windows::WindowsHost;

pub use error::FfiError;
pub use events::{EventPoller, VideoPoller};
pub use files::{
    FfiConflictChoice, FfiDirPage, FfiEntryKind, FfiEntryToken, FfiFileResourcePolicy,
    FfiFileRetentionPolicy, FfiFileRoot, FfiFolderKind, FfiFsEntry, FfiJobDirection,
    FfiJobSnapshot, FfiJobState, FfiPlace, FfiPlaceRoot, FfiPlaces,
};
pub use types::{
    new_quality, FfiCapabilities, FfiCapabilityStatus, FfiCodecPreference, FfiCodecSupport,
    FfiControlRequest, FfiDeviceInfo, FfiDisplay, FfiEncodedFrame, FfiEvent, FfiFrameTiming,
    FfiGrants, FfiKeyboardMode, FfiPairingFile, FfiPairingPreview, FfiPointerButton,
    FfiPointerSettings, FfiQuality, FfiSessionInfo, FfiSessionPath, FfiSessionPhase,
    FfiSessionRole, FfiSnapshot, FfiVideoCodec, FfiVideoCodecCaps, FfiWindowDecoder,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use std::sync::Arc;
use vnilink::{Core, CoreConfig, CustomRelay, Listen, LocalKey, RelaySettings};

use error::map_error;

/// Owned core for one installation. See the module docs for threading.
pub struct VnilinkCore {
    inner: std::sync::Arc<Core>,
    previews: Mutex<HashMap<u64, vnilink::PairingPreview>>,
    next_preview: AtomicU64,
    audio_coordinator: Arc<audio_native::AudioCoordinator>,
}

impl VnilinkCore {
    /// Opens a process-local core. `custom_relays` replaces the public
    /// default; an empty list with `public_relay` false is rejected.

    pub fn create(public_relay: bool, custom_relays: Vec<String>) -> Result<Self, FfiError> {
        let relay = if public_relay {
            RelaySettings::public_n0()
        } else {
            let relays = custom_relays
                .into_iter()
                .map(CustomRelay::new)
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_error)?;
            RelaySettings::custom(relays).map_err(map_error)?
        };
        let core = Core::start(CoreConfig {
            relay,
            listen: Listen::Default,
            ..CoreConfig::default()
        })
        .map_err(map_error)?;
        Ok(Self {
            inner: std::sync::Arc::new(core),
            previews: Mutex::new(HashMap::new()),
            next_preview: AtomicU64::new(1),
            audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
        })
    }

    /// Temporary loopback-only fixture; never exposed through the desktop bridge.
    #[cfg(feature = "test-support")]
    pub fn create_loopback_for_test() -> Result<Self, FfiError> {
        let core = Core::start(CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        })
        .map_err(map_error)?;
        Ok(Self {
            inner: Arc::new(core),
            previews: Mutex::new(HashMap::new()),
            next_preview: AtomicU64::new(1),
            audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
        })
    }

    /// Synthetic desktop for transport tests; never installs native input/capture hooks.
    #[cfg(feature = "test-support")]
    pub fn install_test_desktop(&self) -> Result<Arc<std::sync::atomic::AtomicUsize>, FfiError> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Desktop(Arc<AtomicUsize>);
        impl vnilink::HostDesktop for Desktop {
            fn apply(&mut self, _: vnilink::InputEvent) -> Result<(), vnilink::Error> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn type_text(&mut self, _: &str) -> Result<(), vnilink::Error> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn monitor(&self) -> (u16, u16) {
                (1920, 1080)
            }
        }
        let applied = Arc::new(AtomicUsize::new(0));
        self.inner
            .set_desktop(Box::new(Desktop(applied.clone())))
            .map_err(map_error)?;
        Ok(applied)
    }

    /// Opens the persistent installation through a [`CredentialStore`].

    pub fn open_persistent(store: Box<dyn CredentialStore>) -> Result<Self, FfiError> {
        let core = Core::open(
            std::sync::Arc::new(storage::NativeStore(store.into())),
            vnilink::OpenConfig::default(),
        )
        .map_err(map_error)?;
        Ok(Self {
            inner: std::sync::Arc::new(core),
            previews: Mutex::new(HashMap::new()),
            next_preview: AtomicU64::new(1),
            audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
        })
    }

    /// Loopback core on an injected store. Relays stay disabled. Tests use this
    /// so a real adapter can be armed without binding a public endpoint.
    #[cfg(feature = "test-support")]
    pub fn open_persistent_direct(store: Box<dyn CredentialStore>) -> Result<Self, FfiError> {
        let core = Core::open_direct(
            Arc::new(storage::NativeStore(store.into())),
            RelaySettings::public_n0(),
        )
        .map_err(map_error)?;
        Ok(Self {
            inner: Arc::new(core),
            previews: Mutex::new(HashMap::new()),
            next_preview: AtomicU64::new(1),
            audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
        })
    }

    /// How many times this core claimed endpoint close.
    #[cfg(feature = "test-support")]
    pub fn endpoint_close_count(&self) -> usize {
        self.inner.endpoint_close_count()
    }

    /// Saved relays for the next launch. An empty list means public relays.
    pub fn relay_restart_required(&self) -> Result<bool, FfiError> {
        self.inner.relay_restart_required().map_err(map_error)
    }

    pub fn saved_relays(&self) -> Result<Vec<FfiRelay>, FfiError> {
        Ok(match self.inner.relay_configuration().map_err(map_error)? {
            RelaySettings::Public => Vec::new(),
            RelaySettings::Custom(relays) => relays
                .into_iter()
                .map(|r| FfiRelay {
                    url: r.url().to_string(),
                    authenticated: r.has_auth_token(),
                })
                .collect(),
        })
    }

    pub fn save_custom_relays(&self, inputs: Vec<FfiRelayInput>) -> Result<(), FfiError> {
        let saved = self.inner.relay_configuration().map_err(map_error)?;
        let old = match saved {
            RelaySettings::Custom(relays) => relays,
            _ => Vec::new(),
        };
        let mut relays = Vec::new();
        for input in inputs {
            let mut relay = CustomRelay::new(&input.url).map_err(map_error)?;
            if input.authenticated {
                if input.replacement_token.is_empty() {
                    relay = old
                        .iter()
                        .find(|r| r.url() == relay.url() && r.has_auth_token())
                        .cloned()
                        .ok_or(FfiError::InvalidSetting {
                            message: "Enter a token for this relay".into(),
                        })?;
                } else {
                    relay = relay
                        .with_auth_token(&input.replacement_token)
                        .map_err(map_error)?;
                }
            }
            relays.push(relay);
        }
        self.inner
            .set_relay(RelaySettings::custom(relays).map_err(map_error)?)
            .map_err(map_error)
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub fn endpoint_id(&self) -> String {
        self.inner.endpoint_id()
    }

    pub fn snapshot(&self) -> Result<FfiSnapshot, FfiError> {
        self.inner
            .snapshot()
            .map(types::snapshot)
            .map_err(map_error)
    }

    pub fn capabilities(&self) -> FfiCapabilities {
        types::capabilities(&self.inner.capabilities())
    }

    /// Registers the platform's probe-validated H.264 abilities after its
    /// VA-API / VideoToolbox / Media Foundation probe. Every set bit must
    /// mean a backend processed a sample.
    pub fn set_video_codecs(&self, caps: FfiVideoCodecCaps) -> Result<(), FfiError> {
        self.inner.set_video_codecs(caps.into()).map_err(map_error)
    }

    pub fn video_codecs(&self) -> Result<FfiVideoCodecCaps, FfiError> {
        self.inner.video_codecs().map(Into::into).map_err(map_error)
    }

    pub fn start_hosting(&self) -> Result<(), FfiError> {
        self.inner.start_hosting().map_err(map_error)
    }

    pub fn stop_hosting(&self) -> Result<(), FfiError> {
        self.inner.stop_hosting().map_err(map_error)
    }

    pub fn stop_hosted_session(&self) -> Result<(), FfiError> {
        self.inner.stop_hosted_session().map_err(map_error)
    }

    /// Keeps a link to every paired computer so presence means reachability.
    pub fn set_presence_links(&self, enabled: bool) -> Result<(), FfiError> {
        self.inner.set_presence_links(enabled).map_err(map_error)
    }

    /// What a paired computer last reported it allows; `None` before its
    /// link has delivered a report.
    pub fn peer_availability(
        &self,
        endpoint_id: String,
    ) -> Result<Option<Vec<FfiCapabilityStatus>>, FfiError> {
        Ok(self
            .inner
            .peer_availability(&endpoint_id)
            .map_err(map_error)?
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|(capability, status)| FfiCapabilityStatus {
                        capability: types::capability_name(capability).to_string(),
                        status: status.label().to_string(),
                    })
                    .collect()
            }))
    }

    /// Ends one hosted peer; other peers and hosting itself stay up.
    pub fn stop_hosted_peer(&self, session_id: u64) -> Result<(), FfiError> {
        self.inner.stop_hosted_peer(session_id).map_err(map_error)
    }

    pub fn reset_identity(&self) -> Result<(), FfiError> {
        self.inner.reset_identity().map_err(map_error)
    }

    pub fn set_relay_public(&self) -> Result<(), FfiError> {
        self.inner
            .set_relay(RelaySettings::public_n0())
            .map_err(map_error)
    }

    /// Replaces the relay set with unauthenticated URLs. Use
    /// `save_custom_relays` to configure or preserve credentials.
    pub fn set_relay_custom(&self, urls: Vec<String>) -> Result<(), FfiError> {
        let relays = urls
            .into_iter()
            .map(CustomRelay::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_error)?;
        self.inner
            .set_relay(RelaySettings::custom(relays).map_err(map_error)?)
            .map_err(map_error)
    }

    pub fn create_invitation(&self, ttl_secs: u64) -> Result<String, FfiError> {
        self.inner
            .create_invitation(Duration::from_secs(ttl_secs))
            .map_err(map_error)
    }

    pub fn accept_invitation(&self, token: String) -> Result<FfiDeviceInfo, FfiError> {
        self.inner
            .accept_invitation(&token)
            .map(types::device_info)
            .map_err(map_error)
    }

    pub fn export_pairing_file(
        &self,
        ttl_secs: u64,
        allow_control: bool,
    ) -> Result<FfiPairingFile, FfiError> {
        self.inner
            .export_pairing_file(Duration::from_secs(ttl_secs), allow_control)
            .map(types::pairing_file)
            .map_err(map_error)
    }

    /// Opens a `.vnl` file and keeps the preview server-side under a handle.
    /// The handle, not the token, crosses the FFI.
    pub fn open_pairing_file(&self, bytes: Vec<u8>) -> Result<FfiPairingPreview, FfiError> {
        let preview = self.inner.open_pairing_file(&bytes).map_err(map_error)?;
        let id = self.next_preview.fetch_add(1, Ordering::Relaxed);
        let record = types::pairing_preview(id, &preview);
        self.previews
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(id, preview);
        Ok(record)
    }

    pub fn confirm_pairing(
        &self,
        preview_id: u64,
        digits: String,
    ) -> Result<FfiDeviceInfo, FfiError> {
        let preview = self
            .previews
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&preview_id)
            .ok_or(FfiError::InvalidPairingFile)?;
        self.inner
            .confirm_pairing(&preview, &digits)
            .map(types::device_info)
            .map_err(map_error)
    }

    pub fn set_desktop_sharing(
        &self,
        view: bool,
        control: bool,
        clipboard: bool,
        transfer: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .set_desktop_sharing(vnilink::Grants {
                view,

                control,
                clipboard,
                transfer,
            })
            .map_err(map_error)
    }

    pub fn revoke(&self, endpoint_id: String) -> Result<(), FfiError> {
        self.inner.revoke(&endpoint_id).map_err(map_error)
    }

    pub fn unpair(&self, endpoint_id: String) -> Result<(), FfiError> {
        self.inner.unpair(&endpoint_id).map_err(map_error)
    }

    pub fn control_check(&self) -> String {
        self.inner.control_check()
    }

    pub fn control_request(&self) -> Result<Option<FfiControlRequest>, FfiError> {
        self.inner
            .control_request()
            .map(|request| request.map(types::control_request))
            .map_err(map_error)
    }

    /// Display name a paired peer reported on connect, if any.
    /// Set a local name, or clear it to show the peer's hostname again.
    pub fn set_peer_pet_name(
        &self,
        endpoint_id: String,
        name: Option<String>,
    ) -> Result<(), FfiError> {
        self.inner
            .set_peer_pet_name(&endpoint_id, name.as_deref())
            .map_err(map_error)
    }

    pub fn peer_hostname(&self, endpoint_id: String) -> Result<Option<String>, FfiError> {
        self.inner.peer_hostname(&endpoint_id).map_err(map_error)
    }

    pub fn confirm_control_request(&self, digits: String) -> Result<FfiDeviceInfo, FfiError> {
        self.inner
            .confirm_control_request(&digits)
            .map(types::device_info)
            .map_err(map_error)
    }

    pub fn reject_control_request(&self) -> Result<(), FfiError> {
        self.inner.reject_control_request().map_err(map_error)
    }

    /// Dials a paired device. Blocks up to 75 seconds; call off the UI thread.
    pub fn connect(&self, endpoint_id: String) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .connect(&endpoint_id)
            .map(types::session_info)
            .map_err(map_error)
    }

    /// Dials for file roots/listings/transfers only, without requiring a
    /// video decoder or host screen sharing. Host admission needs Transfer
    /// (Control not required).
    pub fn connect_files(&self, endpoint_id: String) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .connect_files(&endpoint_id)
            .map(types::session_info)
            .map_err(map_error)
    }

    /// Nonblocking start for a tab. Its local id exists before peer approval.
    pub fn begin_connect(&self, endpoint_id: String) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .begin_connect(&endpoint_id)
            .map(types::session_info)
            .map_err(map_error)
    }

    pub fn begin_connect_files(&self, endpoint_id: String) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .begin_connect_files(&endpoint_id)
            .map(types::session_info)
            .map_err(map_error)
    }

    pub fn controller_sessions(&self) -> Result<Vec<FfiSessionInfo>, FfiError> {
        self.inner
            .controller_sessions()
            .map(|sessions| sessions.into_iter().map(types::session_info).collect())
            .map_err(map_error)
    }

    pub fn hosted_sessions(&self) -> Result<Vec<FfiSessionInfo>, FfiError> {
        self.inner
            .hosted_sessions()
            .map(|sessions| sessions.into_iter().map(types::session_info).collect())
            .map_err(map_error)
    }

    pub fn clipboard_owner_session(&self) -> Result<Option<FfiSessionInfo>, FfiError> {
        self.inner
            .clipboard_owner_session()
            .map(|session| session.map(types::session_info))
            .map_err(map_error)
    }

    pub fn controller_session(&self, session_id: u64) -> Result<FfiSessionInfo, FfiError> {
        self.inner
            .controller_session(session_id)
            .map(types::session_info)
            .map_err(map_error)
    }

    pub fn disconnect_session(&self, session_id: u64) -> Result<(), FfiError> {
        self.inner.disconnect_session(session_id).map_err(map_error)
    }

    pub fn forget_controller_session(&self, session_id: u64) -> Result<(), FfiError> {
        self.inner
            .forget_controller_session(session_id)
            .map_err(map_error)
    }

    pub fn session_clipboard_text(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<Option<String>, FfiError> {
        self.inner
            .session_clipboard_text(session_id, generation)
            .map_err(map_error)
    }

    pub fn subscribe_session_video(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<VideoPoller, FfiError> {
        self.inner
            .subscribe_session_video(session_id, generation)
            .map(VideoPoller::new)
            .map_err(map_error)
    }

    pub fn release_session_input(&self, session_id: u64, generation: u64) -> Result<(), FfiError> {
        self.inner
            .release_session_input(session_id, generation)
            .map_err(map_error)
    }

    pub fn session_keyboard_mode(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<FfiKeyboardMode, FfiError> {
        self.inner
            .session_keyboard_mode(session_id, generation)
            .map(types::keyboard_mode)
            .map_err(map_error)
    }

    pub fn session_swap_control_and_command(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<bool, FfiError> {
        self.inner
            .session_swap_control_and_command(session_id, generation)
            .map_err(map_error)
    }

    pub fn set_session_keyboard_mode(
        &self,
        session_id: u64,
        generation: u64,
        mode: FfiKeyboardMode,
    ) -> Result<(), FfiError> {
        self.inner
            .set_session_keyboard_mode(session_id, generation, types::to_keyboard_mode(mode))
            .map_err(map_error)
    }

    pub fn set_session_swap_control_and_command(
        &self,
        session_id: u64,
        generation: u64,
        enabled: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .set_session_swap_control_and_command(session_id, generation, enabled)
            .map_err(map_error)
    }

    pub fn controller_session_paths(
        &self,
        session_id: u64,
    ) -> Result<Vec<FfiSessionPath>, FfiError> {
        self.inner
            .controller_session_paths(session_id)
            .map(|paths| paths.iter().map(types::session_path).collect())
            .map_err(map_error)
    }

    pub fn disconnect(&self) -> Result<(), FfiError> {
        self.inner.disconnect().map_err(map_error)
    }

    pub fn session_paths(&self, session_id: u64) -> Result<Vec<FfiSessionPath>, FfiError> {
        self.inner
            .session_paths(session_id)
            .map(|paths| paths.iter().map(types::session_path).collect())
            .map_err(map_error)
    }

    pub fn submit_key(
        &self,
        session_id: u64,
        generation: u64,
        usage: u16,
        down: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::Key { usage, down },
            )
            .map_err(map_error)
    }

    pub fn submit_pointer(
        &self,
        session_id: u64,
        generation: u64,
        x: i32,
        y: i32,
        display: u64,
        revision: u64,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::pointer(x, y, display, revision),
            )
            .map_err(map_error)
    }

    pub fn set_topology(
        &self,
        revision: u64,
        displays: Vec<types::FfiDisplay>,
    ) -> Result<(), FfiError> {
        let displays = displays.iter().map(types::display_info).collect::<Vec<_>>();
        self.inner
            .set_topology(revision, &displays)
            .map_err(map_error)
    }

    pub fn topology(&self) -> Result<Vec<types::FfiDisplay>, FfiError> {
        self.inner
            .topology()
            .map(|snapshot| {
                snapshot
                    .displays
                    .iter()
                    .map(types::display_record)
                    .collect()
            })
            .map_err(map_error)
    }

    pub fn session_topology(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<Vec<types::FfiDisplay>, FfiError> {
        self.inner
            .session_topology(session_id, generation)
            .map(|displays| displays.iter().map(types::display_record).collect())
            .map_err(map_error)
    }

    pub fn select_displays(
        &self,
        session_id: u64,
        generation: u64,
        ids: Vec<u64>,
    ) -> Result<(), FfiError> {
        self.inner
            .select_displays(session_id, generation, &ids)
            .map_err(map_error)
    }

    pub fn capture_displays(&self) -> Result<Vec<u64>, FfiError> {
        self.inner.capture_displays().map_err(map_error)
    }

    pub fn submit_button(
        &self,
        session_id: u64,
        generation: u64,
        button: FfiPointerButton,
        down: bool,
    ) -> Result<(), FfiError> {
        let button = match button {
            FfiPointerButton::Left => vnilink::PointerButton::Left,
            FfiPointerButton::Right => vnilink::PointerButton::Right,
            FfiPointerButton::Middle => vnilink::PointerButton::Middle,
            FfiPointerButton::Back => vnilink::PointerButton::Back,
            FfiPointerButton::Forward => vnilink::PointerButton::Forward,
        };
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::Button { button, down },
            )
            .map_err(map_error)
    }

    pub fn submit_wheel(
        &self,
        session_id: u64,
        generation: u64,
        dx: i16,
        dy: i16,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::Wheel { dx, dy },
            )
            .map_err(map_error)
    }

    pub fn session_pointer_settings(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<FfiPointerSettings, FfiError> {
        self.inner
            .session_pointer_settings(id, generation)
            .map(Into::into)
            .map_err(map_error)
    }
    pub fn set_session_pointer_settings(
        &self,
        id: u64,
        generation: u64,
        settings: FfiPointerSettings,
    ) -> Result<(), FfiError> {
        self.inner
            .set_session_pointer_settings(id, generation, settings.into())
            .map_err(map_error)
    }

    /// Fractional wheel units: 120 per detent; positive down/right.
    pub fn submit_precise_wheel(
        &self,
        session_id: u64,
        generation: u64,
        dx: i32,
        dy: i32,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::precise_wheel(dx, dy),
            )
            .map_err(map_error)
    }

    /// Relative device counts, not absolute screen coordinates.
    pub fn submit_motion(
        &self,
        session_id: u64,
        generation: u64,
        dx: i16,
        dy: i16,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(session_id, generation, vnilink::InputEvent::motion(dx, dy))
            .map_err(map_error)
    }

    pub fn submit_local_key(
        &self,
        session_id: u64,
        generation: u64,
        usage: u16,
        down: bool,
        text: String,
        composing: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_local_key(
                session_id,
                generation,
                LocalKey {
                    usage,
                    down,
                    text: &text,
                    composing,
                },
            )
            .map_err(map_error)
    }

    pub fn submit_text(
        &self,
        session_id: u64,
        generation: u64,
        text: String,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_text(session_id, generation, &text)
            .map_err(map_error)
    }

    pub fn keyboard_mode(&self) -> Result<FfiKeyboardMode, FfiError> {
        self.inner
            .keyboard_mode()
            .map(types::keyboard_mode)
            .map_err(map_error)
    }

    pub fn set_keyboard_mode(&self, mode: FfiKeyboardMode) -> Result<(), FfiError> {
        self.inner
            .set_keyboard_mode(types::to_keyboard_mode(mode))
            .map_err(map_error)
    }

    pub fn swap_control_and_command(&self) -> Result<bool, FfiError> {
        self.inner.swap_control_and_command().map_err(map_error)
    }

    pub fn set_swap_control_and_command(&self, enabled: bool) -> Result<(), FfiError> {
        self.inner
            .set_swap_control_and_command(enabled)
            .map_err(map_error)
    }

    pub fn set_input_defaults(
        &self,
        mode: FfiKeyboardMode,
        swap: bool,
        pointer: FfiPointerSettings,
    ) -> Result<(), FfiError> {
        self.inner
            .set_input_defaults(types::to_keyboard_mode(mode), swap, pointer.into())
            .map_err(map_error)
    }

    /// The codecs the window decodes; future Desktop owners offer only these.
    pub fn set_window_decoders(&self, decoders: Vec<FfiWindowDecoder>) -> Result<(), FfiError> {
        self.inner
            .set_window_decoders(decoders.into_iter().map(Into::into).collect())
            .map_err(map_error)
    }

    /// Selects the codec for future outgoing Desktop owners.
    pub fn set_desktop_codec_default(&self, codec: FfiCodecPreference) -> Result<(), FfiError> {
        self.inner
            .set_desktop_codec_default(codec.into())
            .map_err(map_error)
    }

    pub fn set_desktop_quality_defaults(&self, quality: FfiQuality) -> Result<(), FfiError> {
        self.inner
            .set_desktop_quality_defaults(types::to_quality(quality)?)
            .map_err(map_error)
    }

    pub fn update_session_quality_for_generation(
        &self,
        session_id: u64,
        generation: u64,
        role: FfiSessionRole,
        quality: FfiQuality,
    ) -> Result<(), FfiError> {
        self.inner
            .update_session_quality_for_generation(
                session_id,
                generation,
                types::to_session_role(role),
                types::to_quality(quality)?,
            )
            .map_err(map_error)
    }

    pub fn update_session_quality(
        &self,
        session_id: u64,
        role: FfiSessionRole,
        quality: FfiQuality,
    ) -> Result<(), FfiError> {
        self.inner
            .update_session_quality(
                session_id,
                types::to_session_role(role),
                types::to_quality(quality)?,
            )
            .map_err(map_error)
    }

    pub fn submit_clipboard(
        &self,
        session_id: u64,
        generation: u64,
        text: String,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_clipboard(session_id, generation, &text)
            .map_err(map_error)
    }

    pub fn clipboard_text(&self) -> Result<Option<String>, FfiError> {
        self.inner.clipboard_text().map_err(map_error)
    }

    pub fn publish_video(
        &self,
        session_id: u64,
        keyframe: bool,
        timestamp_us: u64,
        payload: Vec<u8>,
    ) -> Result<(), FfiError> {
        let frame =
            vnilink::EncodedFrame::h264(keyframe, timestamp_us, payload).map_err(map_error)?;
        self.inner
            .publish_video(session_id, frame)
            .map_err(map_error)
    }

    /// Publish a typed packet without assuming the negotiated codec is H.264.
    pub fn publish_video_frame(
        &self,
        session_id: u64,
        frame: FfiEncodedFrame,
    ) -> Result<(), FfiError> {
        self.inner
            .publish_video(session_id, frame.into_core()?)
            .map_err(map_error)
    }

    pub fn subscribe_events(&self) -> Result<EventPoller, FfiError> {
        self.inner
            .subscribe()
            .map(EventPoller::new)
            .map_err(map_error)
    }

    pub fn subscribe_video(&self) -> Result<VideoPoller, FfiError> {
        self.inner
            .subscribe_video()
            .map(VideoPoller::new)
            .map_err(map_error)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod desktop_tests;
