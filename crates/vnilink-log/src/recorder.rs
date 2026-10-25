//! The flight recorder: recent events in bounded memory, so Export
//! Diagnostics can explain a session after it went wrong.
//!
//! Events that name a desktop service (the field `service`) are kept per
//! service, the most recent [`SERVICES`] services at most; the rest in one
//! process ring. Each ring holds at most [`RING_EVENTS`] events and
//! [`RING_BYTES`] bytes, dropping its oldest.

use std::collections::{HashMap, VecDeque};

/// Events a ring keeps, and the bytes they may take.
pub(crate) const RING_EVENTS: usize = 20_000;
pub(crate) const RING_BYTES: usize = 2 * 1024 * 1024;

/// Desktop services kept apart; an older one's ring makes room.
const SERVICES: usize = 16;

/// The most an export carries, newest kept; past it the export says what it
/// left out.
pub const MAX_EXPORT_BYTES: usize = 4 * 1024 * 1024;

/// An event as recorded: when, and its JSON line.
type Recorded = (u64, String);

#[derive(Default)]
pub(crate) struct Ring {
    pub(crate) events: VecDeque<Recorded>,
    pub(crate) bytes: usize,
    /// When it last took an event, to choose which service's ring goes.
    touched: u64,
}

impl Ring {
    pub(crate) fn push(&mut self, at: u64, line: String) {
        self.bytes += line.len();
        self.events.push_back((at, line));
        self.touched = at;
        while self.events.len() > RING_EVENTS || self.bytes > RING_BYTES {
            let Some((_, oldest)) = self.events.pop_front() else {
                break;
            };
            self.bytes -= oldest.len();
        }
    }
}

#[derive(Default)]
pub(crate) struct Recorder {
    process: Ring,
    pub(crate) services: HashMap<u64, Ring>,
}

impl Recorder {
    pub(crate) fn push(&mut self, service: Option<u64>, line: String, at: u64) {
        let Some(service) = service else {
            self.process.push(at, line);
            return;
        };
        if !self.services.contains_key(&service) && self.services.len() >= SERVICES {
            if let Some(oldest) = self
                .services
                .iter()
                .min_by_key(|(_, ring)| ring.touched)
                .map(|(id, _)| *id)
            {
                self.services.remove(&oldest);
            }
        }
        self.services.entry(service).or_default().push(at, line);
    }

    pub(crate) fn export(&self) -> String {
        let mut lines: Vec<&Recorded> = self.process.events.iter().collect();
        for ring in self.services.values() {
            lines.extend(ring.events.iter());
        }
        // Stable, so events of one millisecond keep their ring's order.
        lines.sort_by_key(|(at, _)| *at);
        let mut kept = Vec::new();
        let mut bytes = 0;
        for (_, line) in lines.iter().rev() {
            if bytes + line.len() + 1 > MAX_EXPORT_BYTES {
                break;
            }
            bytes += line.len() + 1;
            kept.push(line.as_str());
        }
        let left_out = lines.len() - kept.len();
        let mut out = String::with_capacity(bytes + 64);
        if left_out > 0 {
            out.push_str(&format!("{{\"truncated\":{left_out}}}\n"));
        }
        for line in kept.iter().rev() {
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}
