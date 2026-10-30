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

use iroh_relay::server::{RelayConfig, Server, ServerConfig};

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

    /// Bytes the relay has carried between its clients so far.
    pub fn carried(&self) -> u64 {
        self.server
            .as_ref()
            .map_or(0, |server| server.metrics().server.bytes_sent.get())
    }
}

impl Drop for LocalRelay {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let _ = self.runtime.block_on(server.shutdown());
        }
    }
}
