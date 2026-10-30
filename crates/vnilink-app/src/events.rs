// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Polling subscriptions for foreign callers. Events and frames are pulled
//! on a timer, never pushed, so no callback ever crosses the FFI.

use crate::error::FfiError;
use crate::types::{self, FfiEncodedFrame, FfiEvent};

/// Bounded event queue polled from a background thread. A full queue drops
/// for this poller and bumps `lagged` instead of blocking the connection.
pub struct EventPoller {
    inner: std::sync::Mutex<vnilink::EventSubscription>,
}

impl EventPoller {
    pub(crate) fn new(inner: vnilink::EventSubscription) -> Self {
        Self {
            inner: std::sync::Mutex::new(inner),
        }
    }
}

impl EventPoller {
    /// Drains up to `max_events` without blocking.
    pub fn poll(&self, max_events: u32) -> Vec<FfiEvent> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut events = Vec::new();
        for _ in 0..max_events.max(1) {
            match inner.try_recv() {
                Some(event) => events.push(types::event(event)),
                None => break,
            }
        }
        events
    }

    pub fn lagged(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .lagged()
    }
}

/// One video subscription. Frames arrive in decode order; the app decodes
/// them with its platform decoder and paints the BGRA pictures.
pub struct VideoPoller {
    inner: vnilink::VideoSubscription,
}

impl VideoPoller {
    pub(crate) fn new(inner: vnilink::VideoSubscription) -> Self {
        Self { inner }
    }
}

impl VideoPoller {
    /// Waits up to `timeout_ms` for the next frame. Returns none on timeout
    /// so the caller can check its stop flag.
    pub fn recv_timeout(&self, timeout_ms: u64) -> Result<Option<FfiEncodedFrame>, FfiError> {
        if timeout_ms > 60_000 {
            return Err(FfiError::InvalidSetting {
                message: "video poll timeout is at most 60 seconds".to_string(),
            });
        }
        Ok(self
            .inner
            .recv_timeout(std::time::Duration::from_millis(timeout_ms))
            .map(Into::into))
    }

    pub fn try_recv(&self) -> Option<FfiEncodedFrame> {
        self.inner.try_recv().map(Into::into)
    }

    /// Calls `waker` each time a frame is queued; see
    /// [`vnilink::VideoSubscription::on_ready`].
    pub fn on_ready(&self, waker: impl Fn() + Send + Sync + 'static) {
        self.inner.on_ready(waker);
    }

    pub fn lagged(&self) -> u64 {
        self.inner.lagged()
    }
}
