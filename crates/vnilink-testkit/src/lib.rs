//! VniLink's service test kit: engines on one computer, and for each
//! service a functional suite (`tests/<service>.rs`) and a performance
//! suite (`benches/<service>.rs`) that run alone.
//!
//! An [`Engine`] is a real `vnilink::Core` on loopback, with its own
//! identity, store and folders, that never reaches a relay. It shares its
//! whole disk for Files, as the application does, and a test names every
//! path it hands an engine inside its own folders ([`files::Location`]),
//! so it writes nowhere else. A [`Pair`] is a
//! host and a controller paired as the application pairs them. Each service
//! module adds what its suites share: [`files`] its trees and transfers,
//! [`audio`] stand-in sound sources, [`camera`] a stand-in camera,
//! [`desktop`] a scripted screen, a stand-in encoder and recording or
//! scripted desktops driving the host's real picture pipeline, [`drops`]
//! seeded losses on a desktop's stream, [`terminal`] the
//! platform's real shell, [`relay`] a relay on this computer, [`system`]
//! operating systems that record the actions asked of them, [`power`] power
//! leases that count themselves.
//!
//! What every suite waits on lives once here too: [`session`] what a
//! session does, [`events`] what an engine publishes, [`store`]
//! installations whose store the test holds, and [`Traced`] what the
//! engines recorded.
//!
//! Every test starts with [`traced`]: when it fails, it prints the events
//! the engines recorded while it ran (`vnilink::trace`), so a failure
//! explains itself. Waits are bounded and say what they waited for
//! ([`wait_for`]). Performance suites write a [`bench::Report`] and compare
//! it with the previous run on this computer.
//!
//! Run them with `node build/run.ts test:service <service…>` and
//! `node build/run.ts bench:service <service…>`.

pub mod audio;
pub mod bench;
pub mod camera;
pub mod desktop;
pub mod drops;
mod engine;
pub mod events;
pub mod files;
pub mod peer;
pub mod power;
pub mod relay;
mod scratch;
pub mod session;
pub mod store;
pub mod system;
pub mod terminal;
mod trace;
mod wait;

pub use engine::{
    pair_cores, pair_cores_for_control, Engine, Pair, Reach, FILES, SEEN_AND_CONTROLLED,
};
pub use scratch::{PositionalFs, Scratch, ScratchVolume};
pub use trace::{traced, Story, Traced};
pub use wait::{holds, wait_for, DEFAULT_WAIT};

/// `mutex`'s guard, even after a thread panicked holding it: a test that
/// failed leaves its record readable to the one reporting it.
pub fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// What a test returns: setup that fails with `?` fails the test with its
/// error, and asserts state the contract.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;
