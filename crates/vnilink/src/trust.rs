use std::collections::BTreeMap;
use std::net::SocketAddr;

use crate::error::Error;
use crate::session::{Grant, Grants};

pub(crate) const MAX_DEVICES: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct Device {
    pub endpoint_id: [u8; 32],
    pub display_id: String,
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
    pub grants: Grants,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Trust {
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

    pub(crate) fn authorize(
        &mut self,
        endpoint_id: &[u8; 32],
        grants: Grants,
    ) -> Result<(), Error> {
        let device = self.devices.get_mut(endpoint_id).ok_or(Error::NotPaired)?;
        device.grants = grants;
        Ok(())
    }

    pub(crate) fn remove(&mut self, endpoint_id: &[u8; 32]) -> Option<Device> {
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
        let Some(device) = self.devices.get(endpoint_id) else {
            return false;
        };
        match grant {
            Grant::Control => device.grants.control,
            Grant::Clipboard => device.grants.clipboard,
            Grant::Transfer => device.grants.transfer,
        }
    }

    pub(crate) fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }
}
