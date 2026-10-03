//! Invitations, pairing files, and the wait for control approval.

use super::*;

use super::connect::peer_addresses;

use std::time::{Duration, SystemTime};

use iroh::{EndpointId, PublicKey};

use crate::error::Error;
use crate::events::CoreEvent;
use crate::invitation::{self};
use crate::pairing::{PairingFile, PairingPreview};
use crate::session::Grants;
use crate::trust::Device;

impl Core {
    pub fn create_invitation(&self, ttl: Duration) -> Result<String, Error> {
        self.ensure_alive()?;
        let relays = self.shared.relay.relay_urls()?;
        let direct = direct_addresses(&self.shared.endpoint);
        let (token, _) = invitation::issue(
            &self.shared.secret,
            &relays,
            &direct,
            ttl,
            SystemTime::now(),
        )?;
        Ok(token)
    }

    /// Builds a `.vnl` file for another computer to open.
    ///
    /// `ttl` follows the invitation rules: 1 second through 15 minutes.
    /// `allow_control` stays on this computer. While the file is unexpired,
    /// one incoming controller can be granted control after
    /// [`Core::confirm_control_request`]. The returned bytes do not contain
    /// that switch. The app writes them to a file ending in
    /// [`crate::PAIRING_FILE_EXTENSION`]. The steps are in
    /// `crates/vnilink/PAIRING.md`.
    pub fn export_pairing_file(
        &self,
        ttl: Duration,
        allow_control: bool,
    ) -> Result<PairingFile, Error> {
        self.export_pairing_file_at(ttl, allow_control, SystemTime::now())
    }

    pub(crate) fn export_pairing_file_at(
        &self,
        ttl: Duration,
        allow_control: bool,
        now: SystemTime,
    ) -> Result<PairingFile, Error> {
        self.ensure_alive()?;
        let relays = self.shared.relay.relay_urls()?;
        let direct = direct_addresses(&self.shared.endpoint);
        let (token, invitation) =
            invitation::issue(&self.shared.secret, &relays, &direct, ttl, now)?;
        let (_, nonce) = invitation::open(&token, now)?;
        let bytes = crate::pairing::enclose(&token)?;
        let check =
            crate::pairing::file_digits(&invitation.endpoint_id, &nonce, invitation.expires_at_ms);
        let mut state = self.alive_state()?;
        state.pairing_window = Some(PairingWindow {
            expires_at_ms: invitation.expires_at_ms,
            allow_control,
        });
        Ok(PairingFile {
            bytes,
            check,
            expires_at_ms: invitation.expires_at_ms,
        })
    }

    /// Reads a `.vnl` file. This does not save trust.
    ///
    /// Show `preview.check` and confirm with the digits on the computer that
    /// wrote the file.
    pub fn open_pairing_file(&self, bytes: &[u8]) -> Result<PairingPreview, Error> {
        self.open_pairing_file_at(bytes, SystemTime::now())
    }

    pub(crate) fn open_pairing_file_at(
        &self,
        bytes: &[u8],
        now: SystemTime,
    ) -> Result<PairingPreview, Error> {
        self.ensure_alive()?;
        crate::pairing::inspect(bytes, now, self.local_endpoint_id())
    }

    /// Saves the device that wrote `preview` when the digits match that screen.
    ///
    /// A mismatch saves nothing and emits no event. A match accepts the signed
    /// invitation. Control of this computer is not granted here.
    pub fn confirm_pairing(
        &self,
        preview: &PairingPreview,
        check_from_other_screen: &str,
    ) -> Result<DeviceInfo, Error> {
        crate::pairing::require_same_check(preview, check_from_other_screen)?;
        self.accept_invitation(&preview.token)
    }

    /// Three digits this computer shows while it is the controller.
    ///
    /// The host sees the same digits on [`CoreEvent::ControlRequested`].
    pub fn control_check(&self) -> String {
        crate::pairing::control_digits(&self.local_endpoint_id())
    }

    /// The controller waiting for this host to grant control, if one is connected.
    pub fn control_request(&self) -> Result<Option<ControlRequest>, Error> {
        let state = self.alive_state()?;
        Ok(state
            .control_request
            .as_ref()
            .map(|pending| ControlRequest {
                endpoint_id: pending.display_id.clone(),
                check: pending.check.clone(),
            }))
    }

    /// Grants control to the waiting controller when the digits match that screen.
    ///
    /// Clipboard and file transfer stay off. A mismatch rejects the connection
    /// and saves nothing. The pairing file can still admit a later attempt
    /// until it expires.
    pub fn confirm_control_request(
        &self,
        check_from_other_screen: &str,
    ) -> Result<DeviceInfo, Error> {
        self.finish_control_request(check_from_other_screen)
    }

    /// Rejects the waiting controller. The pairing file stays open until it expires.
    pub fn reject_control_request(&self) -> Result<(), Error> {
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        drop(state);
        let _ = pending.decision.send(false);
        Ok(())
    }

    pub fn accept_invitation(&self, token: &str) -> Result<DeviceInfo, Error> {
        let (opened, nonce) = invitation::open(token, SystemTime::now())?;
        if opened.endpoint_id == *self.shared.secret.public().as_bytes() {
            return Err(Error::InvalidInvitation(
                "an installation cannot pair with itself",
            ));
        }
        let now_ms = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| Error::InvalidInvitation("system clock is before the unix epoch"))?
            .as_millis() as u64;
        let display = PublicKey::from_bytes(&opened.endpoint_id)
            .map_err(|_| Error::InvalidInvitation("endpoint id is not a valid key"))?
            .to_string();
        let info = DeviceInfo {
            endpoint_id: display.clone(),
            relays: opened.relays.clone(),
            direct: opened.direct.clone(),
            grants: Grants::default(),
        };
        {
            let mut state = self.alive_state()?;
            let previous_device = state.trust.get(&opened.endpoint_id).cloned();
            let previous_replay = state.replay.clone();
            if let Err(error) = state.replay.remember(nonce, opened.expires_at_ms, now_ms) {
                state.replay = previous_replay;
                return Err(error);
            }
            if let Err(error) = state.trust.pair(Device {
                endpoint_id: opened.endpoint_id,
                display_id: display.clone(),
                relays: opened.relays,
                direct: opened.direct,
                grants: Grants::default(),
            }) {
                state.replay = previous_replay;
                return Err(error);
            }
            if let Err(error) = persist(&self.shared, &state) {
                state.replay = previous_replay;
                match previous_device {
                    Some(device) => {
                        let _ = state.trust.pair(device);
                    }
                    None => {
                        state.trust.remove(&opened.endpoint_id);
                    }
                }
                return Err(error);
            }
        }
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: display,
        });
        Ok(info)
    }

    pub(super) fn finish_control_request(&self, check: &str) -> Result<DeviceInfo, Error> {
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        if check != pending.check {
            let _ = pending.decision.send(false);
            return Err(Error::PairingCheckMismatch);
        }
        let grants = Grants {
            control: true,
            clipboard: false,
            transfer: false,
        };
        let info = DeviceInfo {
            endpoint_id: pending.display_id.clone(),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
            grants,
        };
        let previous = state.trust.get(&pending.endpoint_id).cloned();
        let previous_window = state.pairing_window.take();
        if let Err(error) = state.trust.pair(Device {
            endpoint_id: pending.endpoint_id,
            display_id: pending.display_id.clone(),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
            grants,
        }) {
            state.pairing_window = previous_window;
            let _ = pending.decision.send(false);
            return Err(error);
        }
        if let Err(error) = persist(&self.shared, &state) {
            match previous {
                Some(device) => {
                    let _ = state.trust.pair(device);
                }
                None => {
                    state.trust.remove(&pending.endpoint_id);
                }
            }
            state.pairing_window = previous_window;
            let _ = pending.decision.send(false);
            return Err(error);
        }
        drop(state);
        let _ = pending.decision.send(true);
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: info.endpoint_id.clone(),
        });
        self.shared.events.publish(CoreEvent::DeviceAuthorized {
            endpoint_id: info.endpoint_id.clone(),
        });
        Ok(info)
    }
}

pub(super) enum ControlAnswer {
    Approved,
    Rejected(Error),
}

/// Parks an ungranted Hello while the host types the controller's digits.
///
/// Returns [`ControlAnswer::Rejected`] with the original unauthorized error
/// when this host did not export a file with the allow switch on. That path
/// does not wait.
pub(super) async fn wait_for_control_approval(
    shared: &Shared,
    connection: &iroh::endpoint::Connection,
    peer: EndpointId,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> ControlAnswer {
    let (relays, direct) = peer_addresses(&shared.endpoint, peer).await;
    let (id, receiver) = {
        let mut state = lock(&shared.state);
        if state.shut_down {
            return ControlAnswer::Rejected(Error::ShutDown);
        }
        if state.control_request.is_some() {
            return ControlAnswer::Rejected(Error::Busy);
        }
        let now_ms = match invitation::unix_ms(SystemTime::now()) {
            Ok(now_ms) => now_ms,
            Err(error) => return ControlAnswer::Rejected(error),
        };
        let window_open = state
            .pairing_window
            .as_ref()
            .is_some_and(|window| window.allow_control && now_ms < window.expires_at_ms);
        if !window_open {
            if state
                .pairing_window
                .as_ref()
                .is_some_and(|window| now_ms >= window.expires_at_ms)
            {
                state.pairing_window = None;
            }
            return ControlAnswer::Rejected(Error::Unauthorized);
        }
        let id = state.next_control_request;
        state.next_control_request = state.next_control_request.saturating_add(1);
        let check = crate::pairing::control_digits(peer.as_bytes());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        state.control_request = Some(PendingControl {
            id,
            endpoint_id: *peer.as_bytes(),
            display_id: peer.to_string(),
            check: check.clone(),
            relays,
            direct,
            decision: sender,
        });
        shared.events.publish(CoreEvent::ControlRequested {
            endpoint_id: peer.to_string(),
            check,
        });
        (id, receiver)
    };
    let approved = tokio::select! {
        decision = receiver => decision.unwrap_or(false),
        _ = tokio::time::sleep(CONTROL_APPROVAL_WAIT) => false,
        _ = stop_rx.changed() => false,
        _ = connection.closed() => false,
    };
    if approved {
        return ControlAnswer::Approved;
    }
    let mut state = lock(&shared.state);
    if state
        .control_request
        .as_ref()
        .is_some_and(|pending| pending.id == id)
    {
        state.control_request = None;
    }
    ControlAnswer::Rejected(Error::Unauthorized)
}
