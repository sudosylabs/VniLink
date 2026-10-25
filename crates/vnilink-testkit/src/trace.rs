//! What the engines recorded while a test ran, printed when it fails.

use std::sync::{Mutex, MutexGuard};

/// The most events a failing test prints, its last ones.
const PRINTED: usize = 400;

/// Held by a test; when the test fails it prints the engines' events since
/// it started.
///
/// The log is one per process, so a test takes its turn: while one holds a
/// `Traced`, others in the process wait. Each failure then shows its own
/// test's events only, and timing contracts are not squeezed by siblings.
pub struct Traced {
    since: u64,
    _turn: MutexGuard<'static, ()>,
}

/// Starts recording for this test. Every VniLink event is kept, `trace`
/// level included, unless `VNILINK_LOG` chooses otherwise.
pub fn traced() -> Traced {
    static TURN: Mutex<()> = Mutex::new(());
    // A test that failed holding its turn leaves nothing to protect.
    let turn = TURN.lock().unwrap_or_else(|poison| poison.into_inner());
    vnilink_log::install();
    if vnilink_log::filter().source != vnilink_log::FilterSource::Environment {
        let _ = vnilink_log::set_filter(Some("warn,vnilink=trace"));
    }
    Traced {
        since: vnilink_log::now_ms(),
        _turn: turn,
    }
}

impl Traced {
    /// The events recorded since this test started, as JSON lines.
    pub fn events(&self) -> Vec<String> {
        vnilink_log::export()
            .lines()
            .filter(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|event| event.get("ms").and_then(serde_json::Value::as_u64))
                    .is_some_and(|ms| ms >= self.since)
            })
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Traced {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let events = self.events();
        let shown = &events[events.len().saturating_sub(PRINTED)..];
        eprintln!(
            "--- the engines recorded {} events during this test; the last {}:",
            events.len(),
            shown.len()
        );
        for event in shown {
            eprintln!("{event}");
        }
        eprintln!("---");
    }
}
