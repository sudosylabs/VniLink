// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Decode synthetic cross-platform fixtures through the native GPU adapter.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink_media::VideoDecoder;
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("fixture directory required")?,
    );
    for width in [64, 1920] {
        let mut decoder = vnilink_linux::LinuxDecoder::new();
        for index in 0..3 {
            let bytes = std::fs::read(dir.join(format!("mac-high-{width}-{index}.h264")))?;
            let frame = vnilink::EncodedFrame::h264(index == 0, index * 33_333, bytes)?;
            let picture = decoder.decode(&frame)?;
            std::fs::write(
                dir.join(format!("decoded-{width}-{index}.bgra")),
                picture.pixels(),
            )?;
            println!(
                "{width} frame {index}: {}x{} decoded",
                picture.width(),
                picture.height()
            );
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Run this example on Linux with a VA-API render node.");
}
