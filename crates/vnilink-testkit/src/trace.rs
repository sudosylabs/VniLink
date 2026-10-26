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

impl Traced {
    /// The events recorded since this test started whose message is
    /// `message`, as JSON objects, for a suite to assert what a service
    /// records.
    pub fn named(&self, message: &str) -> Vec<serde_json::Value> {
        self.events()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| {
                event.get("message").and_then(serde_json::Value::as_str) == Some(message)
            })
            .collect()
    }
}

impl Traced {
    /// Waits until `count` events whose message is `message` are recorded.
    /// A service records an event just after the work it describes, so a
    /// test that saw the work waits for its record before stating the story.
    pub fn wait_for_records(&self, message: &str, count: usize) {
        crate::wait_for(
            &format!("{count} \"{message}\" records"),
            crate::DEFAULT_WAIT,
            || (self.named(message).len() >= count).then_some(()),
        );
    }
}

/// Fields a story leaves out: they differ from run to run, as times and
/// peers' fingerprints do, or every line has them.
const UNSTORIED: &[&str] = &["ms", "level", "target", "message", "engine", "peer"];

impl Traced {
    /// What the engines recorded about `areas` (`"clipboard"` for
    /// `vnilink::clipboard`) during this test, for a suite to state with
    /// `expect![…]`: each engine's events in the order it recorded them, as
    /// `area: message key=value…`, warnings and errors marked. Ignored
    /// failures are left out, since teardown races change them and their
    /// counts are in the diagnostic snapshot, and so are `trace` records
    /// and times (`…_ms`, `…_us`).
    pub fn story(&self, areas: &[&str]) -> String {
        self.told(areas, false, false)
    }

    /// [`Traced::story`] with the `trace` records too: what a service
    /// records of every event it handles, as each key applied.
    pub fn detailed_story(&self, areas: &[&str]) -> String {
        self.told(areas, true, false)
    }

    /// [`Traced::story`] for work a service does at once, as files sent
    /// side by side: a run of events with the same message is sorted, since
    /// their order differs between runs and is not part of the story.
    pub fn concurrent_story(&self, areas: &[&str]) -> String {
        self.told(areas, false, true)
    }

    fn told(&self, areas: &[&str], detailed: bool, concurrent: bool) -> String {
        // Each engine's lines, each with the message that starts it.
        let mut engines: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for event in self
            .events()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        {
            let Some(area) = event["target"]
                .as_str()
                .and_then(|target| target.strip_prefix("vnilink::"))
                .filter(|area| areas.contains(area))
            else {
                continue;
            };
            let level = event["level"].as_str().unwrap_or_default();
            let mark = match level {
                "WARN" => "warn ",
                "ERROR" => "error ",
                "INFO" | "DEBUG" => "",
                "TRACE" if detailed => "",
                _ => continue,
            };
            if event["message"] == "ignored" {
                continue;
            }
            let said = format!(
                "{mark}{area}: {}",
                event["message"].as_str().unwrap_or_default()
            );
            let mut line = said.clone();
            if let Some(fields) = event.as_object() {
                for (key, value) in fields {
                    if UNSTORIED.contains(&key.as_str())
                        || key.ends_with("_ms")
                        || key.ends_with("_us")
                    {
                        continue;
                    }
                    match value {
                        serde_json::Value::String(text) => line.push_str(&format!(" {key}={text}")),
                        other => line.push_str(&format!(" {key}={other}")),
                    }
                }
            }
            let engine = event["engine"].as_str().unwrap_or("?").to_owned();
            match engines.iter_mut().find(|(name, _)| *name == engine) {
                Some((_, lines)) => lines.push((said, line)),
                None => engines.push((engine, vec![(said, line)])),
            }
        }
        engines.sort_by(|a, b| a.0.cmp(&b.0));
        let mut story = String::new();
        for (engine, mut lines) in engines {
            if concurrent {
                for run in lines.chunk_by_mut(|a, b| a.0 == b.0) {
                    run.sort();
                }
            }
            story.push_str(&engine);
            story.push_str(":\n");
            for (_, line) in lines {
                story.push_str("  ");
                story.push_str(&line);
                story.push('\n');
            }
        }
        story
    }
}

impl Traced {
    /// The events recorded during this test that do not say which engine
    /// recorded them, the log's own aside.
    pub fn unnamed(&self) -> Vec<String> {
        self.events()
            .into_iter()
            .filter(|line| {
                serde_json::from_str::<serde_json::Value>(line).is_ok_and(|event| {
                    event.get("engine").is_none() && event["target"] != "vnilink::log"
                })
            })
            .collect()
    }
}

impl Drop for Traced {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            // Every record says which engine kept it (`vnilink::trace`).
            let unnamed = self.unnamed();
            assert!(
                unnamed.is_empty(),
                "{} events do not say which engine recorded them:\n{}",
                unnamed.len(),
                unnamed.join("\n")
            );
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
