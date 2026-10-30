// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Diagnostic: open each camera as a host does and say why one does not
//! start, the step that failed included.
//!
//! `cargo run -p vnilink-linux --release --example camera_probe -- [width height fps]`

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;
    use vnilink::camera::{CameraApproval, CameraConfig, HostCameraFactory};

    let numbers: Vec<u16> = std::env::args()
        .skip(1)
        .map(|text| text.parse())
        .collect::<Result<_, _>>()?;
    let (width, height, fps) = match numbers[..] {
        [width, height, fps] => (width, height, fps),
        _ => (1280, 720, 30),
    };
    let factory = vnilink_linux::LinuxCameraFactory::new(CameraApproval::All)?;
    let devices = factory.devices()?;
    println!("{} camera(s)", devices.len());
    for device in devices {
        let config = CameraConfig {
            device_id: device.id.clone(),
            width,
            height,
            fps,
        };
        let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
        match factory.open(config, startup) {
            Ok(_backend) => println!("{}: started", device.name),
            Err(error) => println!("{}: did not start: {error:?}", device.name),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("camera_probe opens Linux cameras");
    std::process::exit(2);
}
