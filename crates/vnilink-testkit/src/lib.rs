//! VniLink's service test kit: engines on one computer, and for each
//! service a functional suite (`tests/<service>.rs`) and a performance
//! suite (`benches/<service>.rs`) that run alone.
//!
//! An [`Engine`] is a real `vnilink::Core` on loopback, with its own
//! identity, store and folders, that never reaches a relay. A [`Pair`] is a
//! host and a controller paired as the application pairs them. Each service
//! module adds what its suites share: [`files`] its trees and transfers,
//! [`audio`] stand-in sound sources, [`camera`] a stand-in camera,
//! [`desktop`] a scripted screen, a stand-in encoder and a recording
//! desktop driving the host's real picture pipeline, [`terminal`] the
//! platform's real shell, [`relay`] a relay on this computer, [`system`] an operating system that records the
//! actions asked of it.
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
mod engine;
pub mod files;
pub mod relay;
mod scratch;
pub mod system;
pub mod terminal;
mod trace;
mod wait;

pub use engine::{Engine, Pair, FILES};
pub use scratch::Scratch;
pub use trace::{traced, Traced};
pub use wait::{wait_for, DEFAULT_WAIT};

/// What a test returns: setup that fails with `?` fails the test with its
/// error, and asserts state the contract.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;
