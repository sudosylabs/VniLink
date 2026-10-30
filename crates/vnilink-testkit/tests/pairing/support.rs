// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What several pairing scenarios share: everything a host can share,
//! pairing through a pairing file, reading what one engine knows of
//! another, and a relay of the computer's own. Installations whose store
//! the test holds are the kit's ([`vnilink_testkit::store`]).

use std::time::Duration;

use vnilink::{Core, CustomRelay, DeviceInfo, Error, Grants, PairingFile, RelaySettings};

/// Everything a host can share.
pub const EVERYTHING: Grants = Grants {
    view: true,
    control: true,
    clipboard: true,
    transfer: true,
};

/// The controller opens `host`'s pairing file and confirms its digits: the
/// controller remembers the host, and the host may approve the controller
/// once when `allow_control`.
pub fn exchange(host: &Core, controller: &Core, allow_control: bool) -> PairingFile {
    let file = host
        .export_pairing_file(Duration::from_secs(60), allow_control)
        .expect("a pairing file");
    let preview = controller
        .open_pairing_file(&file.bytes)
        .expect("the file opens");
    controller
        .confirm_pairing(&preview, &file.check)
        .expect("the digits match");
    file
}

/// What `core` remembers of the peer `endpoint_id`.
pub fn device(core: &Core, endpoint_id: &str) -> DeviceInfo {
    core.snapshot()
        .expect("a snapshot")
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
        .expect("a paired device")
}

/// A relay of its own, with the credential `token`.
pub fn credentialed_relay(token: &str) -> Result<RelaySettings, Error> {
    let relay = CustomRelay::new("https://relay.example.com./")?.with_auth_token(token)?;
    RelaySettings::custom(vec![relay])
}
