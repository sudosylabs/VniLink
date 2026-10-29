use std::collections::BTreeMap;
use std::net::SocketAddr;

use crate::error::Error;
use crate::session::Grant;

pub(crate) const MAX_DEVICES: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub endpoint_id: [u8; 32],
    pub display_id: String,
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Trust {
    pub(crate) sharing: crate::SharingPolicy,
    operating_systems: BTreeMap<[u8; 32], crate::OperatingSystem>,
    devices: BTreeMap<[u8; 32], Device>,
    /// Display names peers reported, by endpoint id. Display-only metadata
    /// beside the security-critical record: renaming never affects grants,
    /// and unpairing drops the name with the device.
    hostnames: BTreeMap<[u8; 32], String>,
    /// Local pet names the owner set, by endpoint id. Shown instead of the
    /// reported name. Same lifetime rules as reported names.
    pet_names: BTreeMap<[u8; 32], String>,
}

impl Trust {
    pub(crate) fn pair(&mut self, device: Device) -> Result<(), Error> {
        if !self.devices.contains_key(&device.endpoint_id) && self.devices.len() >= MAX_DEVICES {
            return Err(Error::TooManyDevices(MAX_DEVICES));
        }
        self.devices.insert(device.endpoint_id, device);
        Ok(())
    }

    pub(crate) fn service_permissions(
        &self,
        endpoint_id: &[u8; 32],
    ) -> Result<crate::ServicePermissions, Error> {
        self.get(endpoint_id).ok_or(Error::NotPaired)?;
        Ok(self.sharing.permissions())
    }

    pub(crate) fn os_of(&self, endpoint: &[u8; 32]) -> Option<crate::OperatingSystem> {
        self.operating_systems.get(endpoint).copied()
    }

    pub(crate) fn set_os(
        &mut self,
        endpoint: &[u8; 32],
        os: crate::OperatingSystem,
    ) -> Result<(), Error> {
        self.get(endpoint).ok_or(Error::NotPaired)?;
        self.operating_systems.insert(*endpoint, os);
        Ok(())
    }

    pub(crate) fn service_granted(&self, endpoint_id: &[u8; 32], kind: crate::ServiceKind) -> bool {
        let Ok(permissions) = self.service_permissions(endpoint_id) else {
            return false;
        };
        match kind {
            crate::ServiceKind::Desktop => permissions.desktop,
            crate::ServiceKind::Files => permissions.files,
            crate::ServiceKind::Terminal => permissions.terminal,
            crate::ServiceKind::Camera => permissions.camera,
            crate::ServiceKind::SystemAudio => permissions.system_audio,
            crate::ServiceKind::Microphone => permissions.microphone,
            crate::ServiceKind::System => permissions.system_actions,
            crate::ServiceKind::ScreenSnapshot => permissions.desktop,
        }
    }

    pub(crate) fn remove(&mut self, endpoint_id: &[u8; 32]) -> Option<Device> {
        self.operating_systems.remove(endpoint_id);
        self.hostnames.remove(endpoint_id);
        self.pet_names.remove(endpoint_id);
        self.devices.remove(endpoint_id)
    }

    /// Stores a peer-reported display name. Fails for unknown devices so a
    /// name never creates a trust entry on its own.
    pub(crate) fn set_hostname(
        &mut self,
        endpoint_id: &[u8; 32],
        hostname: String,
    ) -> Result<(), Error> {
        if !self.devices.contains_key(endpoint_id) {
            return Err(Error::NotPaired);
        }
        self.hostnames.insert(*endpoint_id, hostname);
        Ok(())
    }

    pub(crate) fn hostname_of(&self, endpoint_id: &[u8; 32]) -> Option<String> {
        self.hostnames.get(endpoint_id).cloned()
    }

    pub(crate) fn clear_hostname(&mut self, endpoint_id: &[u8; 32]) {
        self.hostnames.remove(endpoint_id);
    }

    /// Stores the owner's pet name. Fails for unknown devices so a name
    /// never creates a trust entry on its own.
    pub(crate) fn set_pet_name(
        &mut self,
        endpoint_id: &[u8; 32],
        pet_name: String,
    ) -> Result<(), Error> {
        if !self.devices.contains_key(endpoint_id) {
            return Err(Error::NotPaired);
        }
        self.pet_names.insert(*endpoint_id, pet_name);
        Ok(())
    }

    pub(crate) fn pet_name_of(&self, endpoint_id: &[u8; 32]) -> Option<String> {
        self.pet_names.get(endpoint_id).cloned()
    }

    pub(crate) fn clear_pet_name(&mut self, endpoint_id: &[u8; 32]) {
        self.pet_names.remove(endpoint_id);
    }

    pub(crate) fn pet_names(&self) -> impl Iterator<Item = (&[u8; 32], &String)> {
        self.pet_names.iter()
    }

    pub(crate) fn hostnames(&self) -> impl Iterator<Item = (&[u8; 32], &String)> {
        self.hostnames.iter()
    }

    pub(crate) fn get(&self, endpoint_id: &[u8; 32]) -> Option<&Device> {
        self.devices.get(endpoint_id)
    }

    pub(crate) fn granted(&self, endpoint_id: &[u8; 32], grant: Grant) -> bool {
        let Some(_device) = self.devices.get(endpoint_id) else {
            return false;
        };
        let grants = self.sharing.grants();
        match grant {
            Grant::View => grants.view,
            Grant::Control => grants.control,
            Grant::Clipboard => grants.clipboard,
            Grant::Transfer => grants.transfer,
        }
    }

    pub(crate) fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(byte: u8) -> Device {
        let mut endpoint_id = [0u8; 32];
        endpoint_id[0] = byte;
        Device {
            endpoint_id,
            display_id: format!("{byte}"),
            relays: Vec::new(),
            direct: Vec::new(),
        }
    }

    #[test]
    fn global_consent_applies_to_all_known_peers_and_never_unknown_peers() {
        let mut trust = Trust::default();
        let a = device(1);
        let aid = a.endpoint_id;
        let b = device(2);
        let bid = b.endpoint_id;
        trust.pair(a).unwrap();
        trust.pair(b).unwrap();
        assert!(!trust.granted(&aid, Grant::Control));
        trust.sharing = crate::SharingPolicy {
            view: true,
            control: true,
            terminal: true,
            ..Default::default()
        };
        assert!(trust.granted(&aid, Grant::Control) && trust.granted(&bid, Grant::Control));
        assert_eq!(
            trust.service_permissions(&aid).unwrap(),
            trust.service_permissions(&bid).unwrap()
        );
        assert!(!trust.granted(&device(3).endpoint_id, Grant::Control));
        trust.remove(&aid);
        assert!(!trust.granted(&aid, Grant::Control));
        assert!(trust.granted(&bid, Grant::Control));
        trust.sharing.control = false;
        assert!(!trust.granted(&bid, Grant::Control));
    }

    #[test]
    fn the_directory_holds_128_devices_and_replacing_one_does_not_grow_it() {
        let mut trust = Trust::default();
        for byte in 0u8..=127 {
            trust.pair(device(byte)).unwrap();
        }
        let mut extra = device(0);
        extra.endpoint_id[1] = 1;
        assert!(matches!(trust.pair(extra), Err(Error::TooManyDevices(128))));
        let mut replaced = device(0);
        replaced.relays = vec!["https://relay.example.com/".to_owned()];
        trust.pair(replaced).unwrap();
        let id = [0u8; 32];
        assert_eq!(
            trust.get(&id).unwrap().relays,
            vec!["https://relay.example.com/"]
        );
    }
}
