// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Running counts of file transfer, for diagnostics: bytes and files each
//! way, segment checks, and how often transfers resumed, retried, stalled
//! or were refused. Counts only, never file names or contents.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::diagnostics::TransferSummary;

use super::wire::{STATUS_BUSY, STATUS_CHANGED};

#[derive(Default)]
pub(crate) struct TransferCounters {
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    files_served: AtomicU64,
    files_fetched: AtomicU64,
    files_resumed: AtomicU64,
    segments_checked: AtomicU64,
    segments_mismatched: AtomicU64,
    retries: AtomicU64,
    stalls: AtomicU64,
    refused_denied: AtomicU64,
    refused_changed: AtomicU64,
    refused_busy: AtomicU64,
}

fn add(counter: &AtomicU64, value: u64) {
    counter.fetch_add(value, Ordering::Relaxed);
}

fn read(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

impl TransferCounters {
    pub(crate) fn sent(&self, bytes: usize) {
        add(&self.bytes_sent, bytes as u64);
    }

    pub(crate) fn received(&self, bytes: usize) {
        add(&self.bytes_received, bytes as u64);
    }

    /// A file was sent to its end.
    pub(crate) fn served(&self) {
        add(&self.files_served, 1);
    }

    /// A file arrived whole; `resumed` when it continued an earlier try.
    pub(crate) fn fetched(&self, resumed: bool) {
        add(&self.files_fetched, 1);
        if resumed {
            add(&self.files_resumed, 1);
        }
    }

    /// A segment arrived and was checked against its value.
    pub(crate) fn checked(&self, matched: bool) {
        add(
            if matched {
                &self.segments_checked
            } else {
                &self.segments_mismatched
            },
            1,
        );
    }

    pub(crate) fn retried(&self) {
        add(&self.retries, 1);
    }

    pub(crate) fn stalled(&self) {
        add(&self.stalls, 1);
    }

    /// A request was answered with refusal `status`.
    pub(crate) fn refused(&self, status: u8) {
        add(
            match status {
                STATUS_CHANGED => &self.refused_changed,
                STATUS_BUSY => &self.refused_busy,
                _ => &self.refused_denied,
            },
            1,
        );
    }

    pub(crate) fn summary(&self) -> TransferSummary {
        TransferSummary {
            bytes_sent: read(&self.bytes_sent),
            bytes_received: read(&self.bytes_received),
            files_served: read(&self.files_served),
            files_fetched: read(&self.files_fetched),
            files_resumed: read(&self.files_resumed),
            segments_checked: read(&self.segments_checked),
            segments_mismatched: read(&self.segments_mismatched),
            retries: read(&self.retries),
            stalls: read(&self.stalls),
            refused_denied: read(&self.refused_denied),
            refused_changed: read(&self.refused_changed),
            refused_busy: read(&self.refused_busy),
        }
    }
}
