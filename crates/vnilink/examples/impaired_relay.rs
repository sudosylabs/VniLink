// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A local iroh relay for the impaired-network harness's relay mode.
//!
//! Serves plain HTTP on the given address with no QUIC address discovery
//! and no rate limits, the shape a core accepts as a loopback custom relay.
//! The harness starts it inside the host namespace and stops it with
//! SIGTERM; it prints one `listening <address>` line once it accepts
//! connections, then one `metrics <json>` line a second with the server's
//! cumulative counters.
//!
//! The server counts the datagram frames it took from clients
//! (`frames_recv`), wrote to clients (`frames_sent`) and dropped because the
//! recipient was gone or a write to it failed (`frames_dropped`). A frame
//! the recipient's send queue had no room for is counted by none of them:
//! `frames_recv - frames_sent - frames_dropped`, less at most one queue of
//! frames still waiting, is what the per-client queues discarded.
//! `disconnects` counts client connections that ended, including those the
//! server closed after a write took longer than its timeout.
//!
//! Run: `cargo run --release -p vnilink --example impaired_relay 127.0.0.2:3340`

use std::net::SocketAddr;
use std::time::Duration;

use iroh_relay::server::{RelayConfig, Server, ServerConfig};

#[tokio::main]
async fn main() {
    let address: SocketAddr = std::env::args()
        .nth(1)
        .expect("usage: impaired_relay <address:port>")
        .parse()
        .expect("a socket address");
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(address));
    let mut server = Server::spawn(config).await.expect("relay server");
    let bound = server.http_addr().expect("an HTTP listener");
    println!("listening {bound}");
    let metrics = server.metrics().server.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            let line = serde_json::json!({
                "bytes_recv": metrics.bytes_recv.get(),
                "bytes_sent": metrics.bytes_sent.get(),
                "frames_recv": metrics.send_packets_recv.get(),
                "frames_sent": metrics.send_packets_sent.get(),
                "frames_dropped": metrics.send_packets_dropped.get(),
                "accepts": metrics.accepts.get(),
                "disconnects": metrics.disconnects.get(),
            });
            println!("metrics {line}");
        }
    });
    match server.join().await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("relay server stopped: {error}"),
        Err(error) => panic!("relay server task failed: {error}"),
    }
}
