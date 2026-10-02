use std::collections::BTreeMap;
use std::net::SocketAddr;

use crate::error::Error;
use crate::session::Grants;

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

    pub(crate) fn revoke(&mut self, endpoint_id: &[u8; 32]) -> Result<(), Error> {
        let device = self.devices.get_mut(endpoint_id).ok_or(Error::NotPaired)?;
        device.grants = Grants::default();
        Ok(())
    }

    pub(crate) fn remove(&mut self, endpoint_id: &[u8; 32]) -> Option<Device> {
        self.devices.remove(endpoint_id)
    }

    pub(crate) fn get(&self, endpoint_id: &[u8; 32]) -> Option<&Device> {
        self.devices.get(endpoint_id)
    }

    pub(crate) fn control_granted(&self, endpoint_id: &[u8; 32]) -> bool {
        self.devices
            .get(endpoint_id)
            .is_some_and(|device| device.grants.control)
    }

    pub(crate) fn clipboard_granted(&self, endpoint_id: &[u8; 32]) -> bool {
        self.devices
            .get(endpoint_id)
            .is_some_and(|device| device.grants.clipboard)
    }

    pub(crate) fn transfer_granted(&self, endpoint_id: &[u8; 32]) -> bool {
        self.devices
            .get(endpoint_id)
            .is_some_and(|device| device.grants.transfer)
    }

    pub(crate) fn devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }
}
