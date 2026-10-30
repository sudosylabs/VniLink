// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The ScreenCaptureKit and VideoToolbox Swift bridges load
//! `@rpath/libswift_Concurrency.dylib`. Those crates' linker flags do not
//! apply to this crate's tests, so the rpath is set here.

use std::process::Command;

fn main() {
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    let Ok(output) = Command::new("xcode-select").arg("-p").output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let xcode_path = String::from_utf8_lossy(&output.stdout);
    let root = format!(
        "{}/Toolchains/XcodeDefault.xctoolchain/usr/lib",
        xcode_path.trim()
    );
    println!("cargo:rustc-link-arg=-Wl,-rpath,{root}/swift/macosx");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{root}/swift-5.5/macosx");
}
