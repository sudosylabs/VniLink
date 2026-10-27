//! What several files modules share: a sender's serve parked with its source
//! open. Paths inside an engine's folder are the kit's
//! ([`vnilink_testkit::files::Location`]).

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use vnilink::Core;
use vnilink_testkit::lock;

/// The serve to park: its engine ([`Core::engine_key`]) and job.
static TARGET: Mutex<Option<(usize, u64)>> = Mutex::new(None);
/// Set when the target serve opened its source, and woken.
static PARKED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// Set when the test lets the target serve go on, and woken.
static RELEASED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Called by every serve once it opened its verified source: parks the
/// target's until released, and lets every other pass.
fn serve_opened(engine: usize, job: u64) {
    if *lock(&TARGET) != Some((engine, job)) {
        return;
    }
    *lock(&PARKED.0) = true;
    PARKED.1.notify_all();
    let released = lock(&RELEASED.0);
    let _released = RELEASED
        .1
        .wait_while(released, |released| !*released)
        .unwrap_or_else(|poison| poison.into_inner());
}

/// A sender's serve of one job parked once it opened its source, until
/// [`Self::release`]. The probe is one per process, which a test's turn
/// ([`vnilink_testkit::traced`]) holds; dropping this disarms it and lets
/// any parked serve go.
pub struct ServeProbe;

impl ServeProbe {
    /// Parks `sender`'s serves of its job `job`.
    pub fn arm(sender: &Core, job: u64) -> Self {
        *lock(&TARGET) = Some((sender.engine_key(), job));
        *lock(&PARKED.0) = false;
        *lock(&RELEASED.0) = false;
        Core::set_serve_source_probe(Some(serve_opened));
        Self
    }

    /// Whether the serve opened its source and parked, within thirty
    /// seconds. A caller releases it before asserting.
    pub fn parked(&self) -> bool {
        let parked = lock(&PARKED.0);
        let (parked, _) = PARKED
            .1
            .wait_timeout_while(parked, Duration::from_secs(30), |parked| !*parked)
            .unwrap_or_else(|poison| poison.into_inner());
        *parked
    }

    /// Waits for the serve to park; fails, having released it, if it never
    /// does.
    pub fn wait_until_parked(&self) {
        if !self.parked() {
            self.release();
            panic!("the sender's serve never opened its source");
        }
    }

    pub fn release(&self) {
        *lock(&RELEASED.0) = true;
        RELEASED.1.notify_all();
    }
}

impl Drop for ServeProbe {
    fn drop(&mut self) {
        Core::set_serve_source_probe(None);
        *lock(&TARGET) = None;
        self.release();
    }
}
