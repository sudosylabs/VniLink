use std::time::{Duration, Instant};

/// How long a wait lasts when a suite has no reason to choose.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// Asks `check` every few milliseconds until it gives a value, for at most
/// `within`; then fails naming `what` it waited for.
pub fn wait_for<T>(what: &str, within: Duration, mut check: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            started.elapsed() < within,
            "waited {within:?} for {what}, which did not happen"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
