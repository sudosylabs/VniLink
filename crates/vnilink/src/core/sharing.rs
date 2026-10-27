//! Global sharing transactions. Persist consent before revoking any live work.
use super::*;
use crate::{CloseCause, ServicePhase, SharingPolicy};

impl Core {
    pub fn sharing_policy(&self) -> Result<SharingPolicy, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.trust.sharing)
    }

    pub fn change_sharing_policy(&self, update: crate::SharingPolicyUpdate) -> Result<(), Error> {
        let _engine = self.engine();
        self.update_sharing_policy(|policy| update.apply(policy))
    }

    pub fn set_sharing_policy(&self, policy: SharingPolicy) -> Result<(), Error> {
        let _engine = self.engine();
        self.update_sharing_policy(|current| *current = policy)
    }

    pub(super) fn update_sharing_policy(
        &self,
        update: impl FnOnce(&mut SharingPolicy),
    ) -> Result<(), Error> {
        let _serial = lock(&self.shared.sharing_serial);
        let peers;
        let policy;
        {
            let mut state = self.alive_state()?;
            let previous = state.trust.sharing;
            policy = {
                let mut next = previous;
                update(&mut next);
                next
            };
            if previous == policy {
                return Ok(());
            }
            state.trust.sharing = policy;
            if let Err(error) = persist(&self.shared, &mut state) {
                state.trust.sharing = previous;
                return Err(error);
            }
            peers = state
                .trust
                .devices()
                .map(|device| device.endpoint_id)
                .collect::<Vec<_>>();
            for peer in &peers {
                let epoch = state.transfer_epochs.entry(*peer).or_default();
                if previous.files && !policy.files {
                    *epoch = epoch.saturating_add(1);
                }
                super::desktop_stream::reconcile_grants(
                    &self.shared,
                    &mut state,
                    peer,
                    policy.grants(),
                );
            }
            let permissions = policy.permissions();
            for record in state.hosted_services.values_mut() {
                if let Some(desktop) = &record.desktop {
                    desktop.set_recording_allowed(policy.recording);
                }
                let allowed = match record.info.kind {
                    crate::ServiceKind::Desktop => permissions.desktop,
                    crate::ServiceKind::Files => permissions.files,
                    crate::ServiceKind::Terminal => permissions.terminal,
                    crate::ServiceKind::Camera => permissions.camera,
                    crate::ServiceKind::SystemAudio => permissions.system_audio,
                    crate::ServiceKind::Microphone => permissions.microphone,
                    crate::ServiceKind::System => permissions.system_actions,
                    crate::ServiceKind::ScreenSnapshot => permissions.desktop,
                };
                if !allowed
                    && record.change(
                        record.info.generation,
                        ServicePhase::Closed(CloseCause::Revoked),
                    )
                {
                    self.shared.events.publish(CoreEvent::HostedServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
        }
        for peer in peers {
            if !policy.files {
                super::transfer::close_peer_files(&self.shared, &peer);
                super::jobs::interrupt_peer_jobs(&self.shared, &peer, false);
            }
            self.shared.events.publish(CoreEvent::DeviceAuthorized {
                endpoint_id: PublicKey::from_bytes(&peer)
                    .map_err(|_| Error::Malformed)?
                    .to_string(),
            });
        }
        let privacy_result = if !policy.system_actions {
            super::system_stream::release_all_privacy(&self.shared)
        } else {
            Ok(())
        };
        if !policy.terminal {
            for terminal in lock(&self.shared.terminal_sessions).values() {
                terminal
                    .stop
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        }
        super::service_jobs::interrupt_orphaned(&self.shared);
        privacy_result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::installation::MemoryStore;
    use crate::test_support::loopback_core;
    use crate::{Core, Error, RelaySettings, ServiceKind, SharingPolicy, SharingPolicyUpdate};

    #[test]
    fn global_consent_is_durable_covers_future_peers_and_failed_save_changes_nothing() {
        let store = MemoryStore::empty();
        let host = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()).unwrap();
        assert_eq!(host.sharing_policy().unwrap(), SharingPolicy::default());
        host.change_sharing_policy(SharingPolicyUpdate {
            terminal: Some(true),
            files: Some(true),
            ..Default::default()
        })
        .unwrap();
        let a = loopback_core();
        let b = loopback_core();
        for peer in [&a, &b] {
            host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
                .unwrap();
            assert!(
                host.service_permissions(&peer.endpoint_id())
                    .unwrap()
                    .terminal
            );
            assert!(host.service_permissions(&peer.endpoint_id()).unwrap().files);
        }
        let saved = store.record();
        store.fail_next_save(Error::StoreUnavailable);
        assert!(matches!(
            host.set_service_sharing(ServiceKind::Terminal, false),
            Err(Error::StoreUnavailable)
        ));
        assert_eq!(store.record(), saved);
        for peer in [&a, &b] {
            assert!(
                host.service_permissions(&peer.endpoint_id())
                    .unwrap()
                    .terminal
            );
        }
        host.set_service_sharing(ServiceKind::Terminal, false)
            .unwrap();
        for peer in [&a, &b] {
            let permissions = host.service_permissions(&peer.endpoint_id()).unwrap();
            assert!(!permissions.terminal);
            assert!(permissions.files);
        }
        host.shutdown();
        drop(host);
        let reopened = Core::open_direct(Arc::new(store), RelaySettings::public_n0()).unwrap();
        assert!(!reopened.sharing_policy().unwrap().terminal);
        assert!(reopened.sharing_policy().unwrap().files);
        for device in reopened.snapshot().unwrap().devices {
            assert_eq!(device.os, Some(crate::OperatingSystem::local()));
            assert_eq!(device.presence, crate::DevicePresence::Disconnected);
        }
    }
}
