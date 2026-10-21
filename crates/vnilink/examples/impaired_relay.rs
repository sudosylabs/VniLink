//! A local iroh relay for the impaired-network harness's relay mode.
//!
//! Serves plain HTTP on the given address with no QUIC address discovery
//! and no rate limits, the shape a core accepts as a loopback custom relay.
//! The harness starts it inside the host namespace and stops it with
//! SIGTERM; it prints one `listening <address>` line once it accepts
//! connections.
//!
//! Run: `cargo run --release -p vnilink --example impaired_relay 127.0.0.2:3340`

use std::net::SocketAddr;

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
    match server.join().await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("relay server stopped: {error}"),
        Err(error) => panic!("relay server task failed: {error}"),
    }
}
