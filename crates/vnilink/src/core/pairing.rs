//! Invitations, pairing files, and the wait for control approval.
// trace: records every ignored failure

use super::*;

use super::link::peer_addresses;

use std::time::{Duration, SystemTime};

use iroh::{EndpointId, PublicKey};

use crate::error::Error;
use crate::events::CoreEvent;
use crate::invitation::{self};
use crate::pairing::{PairingFile, PairingPreview};
use crate::trust::Device;

impl Core {
    pub fn create_invitation(&self, ttl: Duration) -> Result<String, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let relays = self.shared.relay.relay_urls()?;
        let direct = direct_addresses(&self.shared.endpoint);
        let (token, _) = invitation::issue(
            &self.shared.secret,
            &relays,
            &direct,
            ttl,
            SystemTime::now(),
            self.shared.local_hostname(),
        )?;
        tracing::info!(
            target: crate::trace::PAIRING,
            ttl_s = ttl.as_secs(),
            relays = relays.len(),
            direct = direct.len(),
            "invitation issued"
        );
        Ok(token)
    }

    /// Builds a `.vnl` file for another computer to open.
    ///
    /// `ttl` follows the invitation rules: 1 second through 15 minutes.
    /// `allow_control` stays on this computer. While the file is unexpired,
    /// one incoming peer can be remembered after
    /// [`Core::confirm_control_request`]. The returned bytes do not contain
    /// that switch. The app writes them to a file ending in
    /// [`crate::PAIRING_FILE_EXTENSION`]. The steps are in
    /// `crates/vnilink/PAIRING.md`.
    pub fn export_pairing_file(
        &self,
        ttl: Duration,
        allow_control: bool,
    ) -> Result<PairingFile, Error> {
        let _engine = self.engine();
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
        let (token, invitation) = invitation::issue(
            &self.shared.secret,
            &relays,
            &direct,
            ttl,
            now,
            self.shared.local_hostname(),
        )?;
        let (_, nonce) = invitation::open(&token, now)?;
        let bytes = crate::pairing::enclose(&token)?;
        let check =
            crate::pairing::file_digits(&invitation.endpoint_id, &nonce, invitation.expires_at_ms);
        let mut state = self.alive_state()?;
        state.pairing_window = Some(PairingWindow {
            expires_at_ms: invitation.expires_at_ms,
            allow_control,
        });
        tracing::info!(
            target: crate::trace::PAIRING,
            ttl_s = ttl.as_secs(),
            allow_control,
            relays = relays.len(),
            direct = direct.len(),
            "pairing file exported"
        );
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
        let _engine = self.engine();
        self.open_pairing_file_at(bytes, SystemTime::now())
    }

    pub(crate) fn open_pairing_file_at(
        &self,
        bytes: &[u8],
        now: SystemTime,
    ) -> Result<PairingPreview, Error> {
        self.ensure_alive()?;
        let preview = crate::pairing::inspect(bytes, now, self.local_endpoint_id());
        tracing::info!(
            target: crate::trace::PAIRING,
            bytes = bytes.len(),
            refused = preview.as_ref().err().map(Error::name),
            reason = preview.as_ref().err().and_then(Error::reason),
            "pairing file opened"
        );
        preview
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
        let _engine = self.engine();
        if let Err(error) = crate::pairing::require_same_check(preview, check_from_other_screen) {
            tracing::info!(target: crate::trace::PAIRING, "pairing digits did not match");
            return Err(error);
        }
        self.accept_invitation(&preview.token)
    }

    /// Three digits this computer shows while it is the controller.
    ///
    /// The host sees the same digits on [`CoreEvent::ControlRequested`].
    pub fn control_check(&self) -> String {
        let _engine = self.engine();
        crate::pairing::control_digits(&self.local_endpoint_id())
    }

    /// The authenticated peer waiting for this host to remember its identity.
    pub fn control_request(&self) -> Result<Option<ControlRequest>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        Ok(state
            .control_request
            .as_ref()
            .map(|pending| ControlRequest {
                endpoint_id: pending.display_id.clone(),
                check: pending.check.clone(),
            }))
    }

    /// Remembers the waiting peer when the digits match its screen.
    ///
    /// Global sharing consent is unchanged. A mismatch rejects the connection
    /// and saves nothing, as does an approval after the pairing window
    /// expired. The pairing file can still admit a later attempt until the
    /// invitation itself expires.
    pub fn confirm_control_request(
        &self,
        check_from_other_screen: &str,
    ) -> Result<DeviceInfo, Error> {
        let _engine = self.engine();
        self.finish_control_request(check_from_other_screen)
    }

    /// Rejects the waiting controller. The pairing file stays open until it expires.
    pub fn reject_control_request(&self) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        drop(state);
        tracing::info!(target: crate::trace::PAIRING, "control request rejected");
        crate::ignored!(
            crate::trace::PAIRING,
            "the asking computer stopped waiting for the answer",
            pending.decision.send(false)
        );
        Ok(())
    }

    /// Saves the device an invitation names; records how it went.
    pub fn accept_invitation(&self, token: &str) -> Result<DeviceInfo, Error> {
        let _engine = self.engine();
        let accepted = self.accept(token);
        tracing::info!(
            target: crate::trace::PAIRING,
            refused = accepted.as_ref().err().map(Error::name),
            reason = accepted.as_ref().err().and_then(Error::reason),
            peer = accepted
                .as_ref()
                .ok()
                .and_then(|device| parse_endpoint_id(&device.endpoint_id).ok())
                .map(|key| crate::diagnostics::fingerprint(&key)),
            "invitation accepted"
        );
        accepted
    }

    fn accept(&self, token: &str) -> Result<DeviceInfo, Error> {
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
        // Re-pairing refreshes addresses without silently revoking grants the
        // owner authorized earlier. Pairing alone never adds a grant. The
        // grants are re-read inside the write section below, so a revoke
        // that lands between the two cannot be overwritten here.
        let info = {
            let mut state = self.alive_state()?;
            let previous_trust = state.trust.clone();
            let previous_replay = state.replay.clone();
            if let Err(error) = state.replay.remember(nonce, opened.expires_at_ms, now_ms) {
                state.replay = previous_replay;
                return Err(error);
            }
            if let Err(error) = state.trust.pair(Device {
                endpoint_id: opened.endpoint_id,
                display_id: display.clone(),
                relays: opened.relays.clone(),
                direct: opened.direct.clone(),
            }) {
                state.replay = previous_replay;
                return Err(error);
            }
            // The writer named itself in the invitation: keep it for this
            // computer's device list, before the first session. An absent
            // name keeps whatever a previous pairing stored.
            state.trust.set_os(&opened.endpoint_id, opened.os)?;
            if let Some(name) = opened.hostname.clone() {
                if let Err(error) = state.trust.set_hostname(&opened.endpoint_id, name) {
                    state.replay = previous_replay;
                    return Err(error);
                }
            }
            if let Err(error) = persist_paired(&self.shared, &mut state, previous_trust) {
                state.replay = previous_replay;
                return Err(error);
            }
            DeviceInfo {
                endpoint_id: display.clone(),
                os: Some(opened.os),
                presence: super::link::presence(&state, &display),
                hostname: state.trust.hostname_of(&opened.endpoint_id),
                pet_name: state.trust.pet_name_of(&opened.endpoint_id),
                relays: opened.relays.clone(),
                direct: opened.direct.clone(),
                grants: state.trust.sharing.grants(),
            }
        };
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: display,
        });
        Ok(info)
    }

    pub(super) fn finish_control_request(&self, check: &str) -> Result<DeviceInfo, Error> {
        let _engine = self.engine();
        let answered = self.answer_control_request(check);
        tracing::info!(
            target: crate::trace::PAIRING,
            refused = answered.as_ref().err().map(Error::name),
            "control request answered"
        );
        answered
    }

    fn answer_control_request(&self, check: &str) -> Result<DeviceInfo, Error> {
        let mut state = self.alive_state()?;
        let pending = state
            .control_request
            .take()
            .ok_or(Error::NoControlRequest)?;
        if check != pending.check {
            crate::ignored!(
                crate::trace::PAIRING,
                "the asking computer stopped waiting for the answer",
                pending.decision.send(false)
            );
            return Err(Error::PairingCheckMismatch);
        }
        let previous_trust = state.trust.clone();
        let previous_window = state.pairing_window.take();
        // The Hello parked up to a minute. A switch that expired while the
        // owner hesitated no longer grants.
        let now_ms = match invitation::unix_ms(SystemTime::now()) {
            Ok(now_ms) => now_ms,
            Err(error) => {
                state.pairing_window = previous_window;
                crate::ignored!(
                    crate::trace::PAIRING,
                    "the asking computer stopped waiting for the answer",
                    pending.decision.send(false)
                );
                return Err(error);
            }
        };
        if previous_window
            .as_ref()
            .is_some_and(|window| now_ms >= window.expires_at_ms)
        {
            crate::ignored!(
                crate::trace::PAIRING,
                "the asking computer stopped waiting for the answer",
                pending.decision.send(false)
            );
            return Err(Error::InvitationExpired);
        }
        // Pairing remembers identity; only global sharing settings grant services.
        let info = DeviceInfo {
            os: None,
            presence: crate::DevicePresence::Disconnected,
            endpoint_id: pending.display_id.clone(),
            hostname: state.trust.hostname_of(&pending.endpoint_id),
            pet_name: state.trust.pet_name_of(&pending.endpoint_id),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
            grants: state.trust.sharing.grants(),
        };
        if let Err(error) = state.trust.pair(Device {
            endpoint_id: pending.endpoint_id,
            display_id: pending.display_id.clone(),
            relays: pending.relays.clone(),
            direct: pending.direct.clone(),
        }) {
            state.pairing_window = previous_window;
            crate::ignored!(
                crate::trace::PAIRING,
                "the asking computer stopped waiting for the answer",
                pending.decision.send(false)
            );
            return Err(error);
        }
        if let Err(error) = persist_paired(&self.shared, &mut state, previous_trust) {
            // A fatal save already cleared the window and stopped admission.
            if !state.shut_down {
                state.pairing_window = previous_window;
            }
            crate::ignored!(
                crate::trace::PAIRING,
                "the asking computer stopped waiting for the answer",
                pending.decision.send(false)
            );
            return Err(error);
        }
        drop(state);
        crate::ignored!(
            crate::trace::PAIRING,
            "the asking computer stopped waiting for the answer",
            pending.decision.send(true)
        );
        self.shared.events.publish(CoreEvent::DevicePaired {
            endpoint_id: info.endpoint_id.clone(),
        });
        Ok(info)
    }
}

pub(super) enum ControlAnswer {
    Approved,
    Rejected(Error),
}

/// Persists staged pairing changes. On a save failure the previous trust
/// entry returns, or the new endpoint leaves, so memory never runs ahead of
/// the store. Replay and window restores stay with the callers.
fn persist_paired(
    shared: &Shared,
    state: &mut State,
    previous: crate::trust::Trust,
) -> Result<(), Error> {
    if let Err(error) = persist(shared, state) {
        state.trust = previous;
        return Err(error);
    }
    Ok(())
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
