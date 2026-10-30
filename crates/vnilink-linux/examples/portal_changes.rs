// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Diagnostic: how often the ScreenCast portal delivers a picture of the
//! chosen monitor, and how often its pixels changed, while the screen is
//! meant to be still. A delivery advances the host's attestation, so one
//! without a change keeps the host from seeing a still screen.
//!
//! `cargo run -p vnilink-linux --release --example portal_changes -- [seconds] [grant file]`

/// Milliseconds since 1970, to line changes up with other logs.
#[cfg(target_os = "linux")]
fn epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |age| age.as_millis())
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    let seconds: u64 = std::env::args()
        .nth(1)
        .map_or(Ok(20), |text| text.parse())?;
    let grant = std::env::args().nth(2).map(vnilink_linux::ScreenGrant::at);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut session = vnilink_linux::PortalSession::new();
    let remote = runtime.block_on(vnilink_linux::open_monitor(&mut session, grant.as_ref()))?;
    let node = session
        .pipewire_node()
        .ok_or("the portal started no stream")?;
    let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
    let capture = vnilink_linux::PortalCapture::open(remote, node, startup)?;
    // The first picture is a change; count from a quiet start.
    let quiet = Instant::now() + Duration::from_secs(2);
    while Instant::now() < quiet {
        let _ = capture.poll_frame(0)?;
        std::thread::sleep(Duration::from_millis(16));
    }
    let (mut deliveries, mut changed) = (0u32, 0u32);
    let mut gaps = Vec::new();
    let mut attested = capture.attested_at();
    let mut last_delivery = Instant::now();
    let mut last_rows: Option<Vec<Vec<u8>>> = None;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        let frame = capture.poll_frame(0)?;
        let now = capture.attested_at();
        if now != attested {
            attested = now;
            deliveries += 1;
            gaps.push(last_delivery.elapsed().as_millis());
            last_delivery = Instant::now();
            if let Some(frame) = frame {
                let rows: Vec<Vec<u8>> =
                    (0..frame.height()).map(|y| frame.row(y).to_vec()).collect();
                if let Some(last) = &last_rows {
                    // The rows and columns (in pixels) that differ: where on
                    // the screen the change is.
                    let differ: Vec<usize> =
                        (0..rows.len()).filter(|&y| rows[y] != last[y]).collect();
                    if let (Some(&top), Some(&bottom)) = (differ.first(), differ.last()) {
                        changed += 1;
                        let columns = differ.iter().flat_map(|&y| {
                            rows[y]
                                .chunks(4)
                                .zip(last[y].chunks(4))
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .map(|(x, _)| x)
                        });
                        let (left, right) =
                            columns.fold((usize::MAX, 0), |(l, r), x| (l.min(x), r.max(x)));
                        println!(
                            "  {} ms: rows {top}..={bottom}, columns {left}..={right}",
                            epoch_ms()
                        );
                    } else {
                        println!("  {} ms: identical", epoch_ms());
                    }
                }
                last_rows = Some(rows);
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    gaps.sort_unstable();
    let median = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
    println!(
        "over {seconds} s: {deliveries} deliveries, {changed} with changed pixels, \
         {} identical; median gap {median} ms",
        deliveries - changed.min(deliveries)
    );
    drop(capture);
    drop(runtime);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("portal_changes reads a Linux display");
    std::process::exit(2);
}
