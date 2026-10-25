use std::time::{Duration, Instant};

/// How long a wait lasts when a suite has no reason to choose.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(10);

/// Asks `check` until it gives a value, for at most `within`; then fails
/// naming `what` it waited for. It looks again after a tenth of the time it
/// has waited, between 50 µs and a millisecond, so a bench measuring
/// through it adds about a tenth to what it measures.
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
        let pause =
            (started.elapsed() / 10).clamp(Duration::from_micros(50), Duration::from_millis(1));
        std::thread::sleep(pause);
    }
}
