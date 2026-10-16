//! Effective permissions combine remembered peer identity and global consent.
use super::*;
use crate::{ServiceKind, ServicePermissions};
impl Core {
    pub fn service_permissions(&self, endpoint_id: &str) -> Result<ServicePermissions, Error> {
        let peer = parse_endpoint_id(endpoint_id)?;
        self.alive_state()?.trust.service_permissions(&peer)
    }
    pub fn set_service_sharing(&self, kind: ServiceKind, allowed: bool) -> Result<(), Error> {
        self.update_sharing_policy(|policy| match kind {
            ServiceKind::Desktop => policy.view = allowed,
            ServiceKind::Files => policy.files = allowed,
            ServiceKind::Terminal => policy.terminal = allowed,
            ServiceKind::Camera => policy.camera = allowed,
            ServiceKind::SystemAudio => policy.system_audio = allowed,
            ServiceKind::Microphone => policy.microphone = allowed,
            ServiceKind::System => policy.system_actions = allowed,
        })
    }
    pub fn set_recording_sharing(&self, allowed: bool) -> Result<(), Error> {
        self.update_sharing_policy(|policy| policy.recording = allowed)
    }
}
pub(super) fn admitted(state: &State, peer: &[u8; 32], kind: ServiceKind) -> bool {
    state.trust.service_granted(peer, kind)
}
