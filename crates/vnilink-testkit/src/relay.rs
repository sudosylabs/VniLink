// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A relay on this computer: engines started with
//! [`crate::Engine::relayed`] have no direct address, so everything between
//! them crosses it, as between two computers that cannot reach each other
//! directly.

use std::net::SocketAddr;
use std::time::Duration;

use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{tcp::OwnedReadHalf, tcp::OwnedWriteHalf, TcpListener, TcpStream};

/// A relay serving plain HTTP on loopback, stopped when dropped.
pub struct LocalRelay {
    url: String,
    server: Option<Server>,
    runtime: tokio::runtime::Runtime,
}

impl LocalRelay {
    pub fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime for the relay");
        let server = runtime.block_on(async {
            let mut config = ServerConfig::default();
            config.relay = Some(RelayConfig::new(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                0,
            ))));
            Server::spawn(config).await.expect("a relay server")
        });
        let address = server.http_addr().expect("the relay's address");
        Self {
            url: format!("http://{address}"),
            server: Some(server),
            runtime,
        }
    }

    /// The URL engines use to reach it.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A proxy in front of this relay that holds every byte `one_way`
    /// before passing it on, in each direction. A transfer between two
    /// engines crosses it twice each way, so their round trip time is
    /// four times `one_way`.
    pub fn delayed(&self, one_way: Duration) -> Delayed {
        let target = self
            .url
            .strip_prefix("http://")
            .and_then(|address| address.parse::<SocketAddr>().ok())
            .expect("the relay's address");
        let listener = self
            .runtime
            .block_on(TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))))
            .expect("a port for the proxy");
        let url = format!("http://{}", listener.local_addr().expect("its address"));
        let accepting = self.runtime.spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(relay) = TcpStream::connect(target).await else {
                    continue;
                };
                let (client_read, client_write) = client.into_split();
                let (relay_read, relay_write) = relay.into_split();
                tokio::spawn(pass(client_read, relay_write, one_way));
                tokio::spawn(pass(relay_read, client_write, one_way));
            }
        });
        Delayed { url, accepting }
    }

    /// Bytes the relay has carried between its clients so far.
    pub fn carried(&self) -> u64 {
        self.server
            .as_ref()
            .map_or(0, |server| server.metrics().server.bytes_sent.get())
    }
}

/// A relay's address as engines reach it through a delaying proxy
/// ([`LocalRelay::delayed`]), stopped when dropped.
pub struct Delayed {
    url: String,
    accepting: tokio::task::JoinHandle<()>,
}

impl Delayed {
    /// The URL engines use to reach the relay through the proxy.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for Delayed {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

/// Copies `from` to `to`, each read written `delay` after it was read, in
/// order and without limiting the rate.
async fn pass(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, delay: Duration) {
    let (held, mut due) = tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let writing = tokio::spawn(async move {
        while let Some((at, bytes)) = due.recv().await {
            tokio::time::sleep_until(at).await;
            if to.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let mut buffer = vec![0u8; 64 * 1024];
    while let Ok(read) = from.read(&mut buffer).await {
        if read == 0
            || held
                .send((tokio::time::Instant::now() + delay, buffer[..read].to_vec()))
                .is_err()
        {
            break;
        }
    }
    drop(held);
    let _ = writing.await;
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = self.runtime.block_on(server.shutdown());
        }
    }
}
