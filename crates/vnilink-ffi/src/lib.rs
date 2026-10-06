//! UniFFI control-plane bindings for VniLink.
//!
//! SwiftUI (Swift) and WinUI (C#) host the shared Rust core through this
//! crate. Only value types cross the boundary: records, enums, byte vectors,
//! and strings. Media stays in Rust behind [`vnilink_media`] — encoded
//! frames cross as byte vectors, never as shared surfaces.
//!
//! # Threading
//!
//! [`VnilinkCore`] owns the same Tokio runtime as [`vnilink::Core`]. Call
//! every method from a background thread, never from an async executor or
//! the UI thread: [`VnilinkCore::connect`] blocks up to 75 seconds. Poll
//! [`EventPoller`] and [`VideoPoller`] on a timer and marshal results onto
//! the UI thread in the app.
//!
//! # Errors
//!
//! Every fallible method returns [`FfiError`], a flat mapping of
//! [`vnilink::Error`] that keeps each distinguishable reason. Messages name
//! the reason only; they never carry secrets, pixels, or bitstreams.
//!
//! # Out of scope for v1
//!
//! Desktop/capture adapter installation stays in platform Rust. Native UI
//! pasteboards implement `ClipboardWriter`; `install_clipboard` installs that
//! callback before hosting, while received files use `set_transfer_directory`.
//! The persistent
//! installation store (`Core::open`) needs the platform credential store
//! first. `open_persistent` delegates opaque record storage to the native
//! credential-store callback; saved relay credentials are never read back.

mod clipboard;
pub use clipboard::ClipboardWriter;
mod error;
mod events;
#[cfg(target_os = "macos")]
mod macos;
mod storage;
mod types;
#[cfg(target_os = "macos")]
pub use macos::{MacFrame, MacHost, MacViewer};
pub use storage::{CredentialStore, FfiRelay, FfiRelayInput};

pub use error::FfiError;
pub use events::{EventPoller, VideoPoller};
pub use types::{
    new_quality, FfiCapabilities, FfiControlRequest, FfiDeviceInfo, FfiEncodedFrame, FfiEvent,
    FfiGrants, FfiKeyboardMode, FfiPairingFile, FfiPairingPreview, FfiPointerButton, FfiQuality,
    FfiSessionInfo, FfiSessionPath, FfiSessionPhase, FfiSessionRole, FfiSnapshot,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use vnilink::{Core, CoreConfig, CustomRelay, Listen, LocalKey, RelaySettings};

use error::map_error;

/// Owned core for one installation. See the module docs for threading.
#[derive(uniffi::Object)]
pub struct VnilinkCore {
    inner: std::sync::Arc<Core>,
    previews: Mutex<HashMap<u64, vnilink::PairingPreview>>,
    next_preview: AtomicU64,
}

#[uniffi::export]
impl VnilinkCore {
    /// Opens a process-local core. `custom_relays` replaces the public
    /// default; an empty list with `public_relay` false is rejected.
    #[uniffi::constructor]
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
        })
    }

    /// Opens the persistent installation using a native credential store.
    #[uniffi::constructor]
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
        })
    }

    /// Saved relays for the next launch. An empty list means public relays.
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

    /// Records the platform's H.264 availability after its VA-API /
    /// VideoToolbox / Media Foundation probe.
    pub fn set_h264_support(&self, encode: bool, decode: bool) -> Result<(), FfiError> {
        self.inner
            .set_h264_support(encode, decode)
            .map_err(map_error)
    }

    pub fn set_transfer_directory(&self, path: String) -> Result<(), FfiError> {
        self.inner.set_transfer_directory(path).map_err(map_error)
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

    pub fn authorize(
        &self,
        endpoint_id: String,
        control: bool,
        clipboard: bool,
        transfer: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .authorize(
                &endpoint_id,
                vnilink::Grants {
                    control,
                    clipboard,
                    transfer,
                },
            )
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
        x: u16,
        y: u16,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_input(
                session_id,
                generation,
                vnilink::InputEvent::Pointer { x, y },
            )
            .map_err(map_error)
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

    pub fn submit_local_key(
        &self,
        session_id: u64,
        generation: u64,
        usage: u16,
        down: bool,
        text: String,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_local_key(
                session_id,
                generation,
                LocalKey {
                    usage,
                    down,
                    text: &text,
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

    /// One adaptive step for the controlling session. Returns the applied
    /// quality, or `None` to hold. Call every ~2s from the viewer.
    pub fn auto_quality_tick(&self, session_id: u64) -> Result<Option<FfiQuality>, FfiError> {
        self.inner
            .auto_quality_tick(session_id)
            .map(|opt| opt.as_ref().map(types::quality_record))
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

    pub fn submit_file(
        &self,
        session_id: u64,
        generation: u64,
        name: String,
        bytes: Vec<u8>,
        replace: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_file(session_id, generation, &name, &bytes, replace)
            .map_err(map_error)
    }

    pub fn publish_file(
        &self,
        session_id: u64,
        name: String,
        bytes: Vec<u8>,
        replace: bool,
    ) -> Result<(), FfiError> {
        self.inner
            .publish_file(session_id, &name, &bytes, replace)
            .map_err(map_error)
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

uniffi::setup_scaffolding!();

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod desktop_tests;
