//! The paired-computer list. Owns row data and selection, not widgets.

use vnilink::{DeviceInfo, Grants};

/// One sidebar row: this computer or a paired peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// Widget name suffix: `"self"` or `"peer-{endpoint_id}"`.
    pub name: String,
    pub title: String,
    pub subtitle: String,
}

pub fn self_row(hosting: bool) -> DeviceRow {
    DeviceRow {
        name: "self".to_string(),
        title: "This computer".to_string(),
        subtitle: if hosting {
            "Allowing control".to_string()
        } else {
            "Not allowing control".to_string()
        },
    }
}

pub fn peer_row(device: &DeviceInfo, status: &str) -> DeviceRow {
    DeviceRow {
        name: format!("peer-{}", device.endpoint_id),
        title: crate::presentation::device_name(device),
        subtitle: status.to_string(),
    }
}

/// Rows for the sidebar: this computer first, then paired peers.
pub fn rows(
    devices: &[DeviceInfo],
    hosting: bool,
    status: impl Fn(Grants) -> &'static str,
) -> Vec<DeviceRow> {
    std::iter::once(self_row(hosting))
        .chain(
            devices
                .iter()
                .map(|device| peer_row(device, status(device.grants))),
        )
        .collect()
}

/// Selection that survives a device list change. Drops the selection when
/// its computer is gone.
pub fn retain_selection(selected: &Option<String>, devices: &[DeviceInfo]) -> Option<String> {
    selected
        .clone()
        .filter(|id| devices.iter().any(|device| &device.endpoint_id == id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, control: bool) -> DeviceInfo {
        DeviceInfo {
            hostname: None,
            pet_name: None,
            endpoint_id: id.to_string(),
            relays: Vec::new(),
            direct: Vec::new(),
            grants: Grants {
                view: control,
                control,
                clipboard: false,
                transfer: false,
            },
        }
    }

    #[test]
    fn rows_lead_with_this_computer_and_track_selection() {
        let devices = vec![peer("other", true)];
        let rows = rows(&devices, false, |_| "Control allowed");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "self");
        assert_eq!(rows[0].subtitle, "Not allowing control");
        assert_eq!(rows[1].name, "peer-other");
        assert_eq!(
            retain_selection(&Some("other".to_string()), &devices),
            Some("other".to_string())
        );
        assert_eq!(retain_selection(&Some("gone".to_string()), &devices), None);
        assert_eq!(retain_selection(&None, &devices), None);
    }
}
