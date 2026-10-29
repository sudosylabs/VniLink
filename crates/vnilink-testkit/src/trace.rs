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

    /// The events whose message is `message` and whose every field in
    /// `fields` is the text given, as `("engine", "host")` for one engine's.
    pub fn named_with(&self, message: &str, fields: &[(&str, &str)]) -> Vec<serde_json::Value> {
        self.named(message)
            .into_iter()
            .filter(|event| {
                fields.iter().all(|(field, value)| match &event[*field] {
                    serde_json::Value::String(text) => text == value,
                    other => other.to_string() == *value,
                })
            })
            .collect()
    }

    /// As [`Self::wait_for_records`], counting only the records
    /// [`Self::named_with`] picks: for a record one engine makes at a moment
    /// of its own.
    pub fn wait_for_records_with(&self, message: &str, fields: &[(&str, &str)], count: usize) {
        crate::wait_for(
            &format!("{count} \"{message}\" records with {fields:?}"),
            crate::DEFAULT_WAIT,
            || (self.named_with(message, fields).len() >= count).then_some(()),
        );
    }
}

impl Traced {
    /// Fails if any event recorded during this test holds one of `content`:
    /// `what`, as \"terminal content\", is the person's, never the record's.
    pub fn assert_unrecorded(&self, what: &str, content: &[&str]) {
        for event in self.events() {
            for text in content {
                assert!(
                    !event.contains(text),
                    "{what} is never recorded, yet {text:?} was: {event}"
                );
            }
        }
    }

    /// How `engine` recorded each hosted service it served ending, by
    /// service id: the phase it ended in, whichever path recorded it. A
    /// connection's close and a serve task's own end race to record a lost
    /// parent, under different messages and the same cause.
    pub fn hosted_ends(&self, engine: &str) -> Vec<(u64, String)> {
        let mut ends: Vec<(u64, String)> = self
            .events()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|event| event["engine"] == engine)
            .filter_map(|event| {
                let phase = match event["message"].as_str()? {
                    "service phase" if event["target"] == "vnilink::service" => {
                        event["to"].as_str()?
                    }
                    "service closed" | "service ended without closing" => {
                        event["phase"].as_str()?
                    }
                    _ => return None,
                };
                (phase.starts_with("closed") || phase.starts_with("failed"))
                    .then(|| (event["service"].as_u64(), phase.to_owned()))
                    .and_then(|(service, phase)| Some((service?, phase)))
            })
            .collect();
        ends.sort();
        ends
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
    /// and times (`…_ms`, `…_us`). [`Traced::tell`] chooses otherwise.
    pub fn story(&self, areas: &[&str]) -> String {
        self.tell(areas).to_string()
    }

    /// [`Story::detailed`]: what a service records of every event it
    /// handles, as each key applied.
    pub fn detailed_story(&self, areas: &[&str]) -> String {
        self.tell(areas).detailed().to_string()
    }

    /// [`Story::concurrent`]: for work a service does at once.
    pub fn concurrent_story(&self, areas: &[&str]) -> String {
        self.tell(areas).concurrent().to_string()
    }

    /// [`Story::of`]: one engine's side.
    pub fn engine_story(&self, engine: &str, areas: &[&str]) -> String {
        self.tell(areas).of(engine).to_string()
    }

    /// The story of `areas`, to tell as [`Traced::story`] does or as its
    /// options choose.
    pub fn tell<'a>(&'a self, areas: &[&str]) -> Story<'a> {
        Story {
            traced: self,
            areas: areas.iter().map(|area| (*area).to_owned()).collect(),
            detailed: false,
            concurrent: false,
            engine: None,
            messages: None,
            masks: Vec::new(),
        }
    }
}

/// A story of some areas and how to tell it: made by [`Traced::tell`],
/// told by `to_string`.
pub struct Story<'a> {
    traced: &'a Traced,
    areas: Vec<String>,
    detailed: bool,
    concurrent: bool,
    engine: Option<String>,
    messages: Option<Vec<String>>,
    masks: Vec<Mask<'a>>,
}

/// Fields whose values are shown as `…` on the events `when` picks.
struct Mask<'a> {
    fields: Vec<String>,
    when: Box<dyn Fn(&serde_json::Value) -> bool + 'a>,
}

impl<'a> Story<'a> {
    /// With the `trace` records too.
    pub fn detailed(mut self) -> Self {
        self.detailed = true;
        self
    }

    /// For work a service does at once, as files sent side by side: a run
    /// of events with the same message is sorted, since their order differs
    /// between runs and is not part of the story.
    pub fn concurrent(mut self) -> Self {
        self.concurrent = true;
        self
    }

    /// As `engine` alone told it, for a scenario whose other engine records
    /// at moments of its own, as a sender mirroring its receiver when it
    /// next asks.
    pub fn of(mut self, engine: &str) -> Self {
        self.engine = Some(engine.to_owned());
        self
    }

    /// Only the events whose message is one of `messages`, as a job's
    /// states without the files that moved between them.
    pub fn only(mut self, messages: &[&str]) -> Self {
        self.messages = Some(
            messages
                .iter()
                .map(|message| (*message).to_owned())
                .collect(),
        );
        self
    }

    /// Shows `fields` as `…` on the events `when` picks, where their values
    /// differ between runs, as how far a cancelled file got; bound those
    /// values in an assert instead.
    pub fn masking(
        mut self,
        fields: &[&str],
        when: impl Fn(&serde_json::Value) -> bool + 'a,
    ) -> Self {
        self.masks.push(Mask {
            fields: fields.iter().map(|field| (*field).to_owned()).collect(),
            when: Box::new(when),
        });
        self
    }

    /// The story's line for `event`, and the part that starts it, or
    /// `None` when the story leaves it out.
    fn line(&self, event: &serde_json::Value) -> Option<(String, String)> {
        let area = event["target"]
            .as_str()
            .and_then(|target| target.strip_prefix("vnilink::"))
            .filter(|area| self.areas.iter().any(|wanted| wanted == area))?;
        let mark = match event["level"].as_str().unwrap_or_default() {
            "WARN" => "warn ",
            "ERROR" => "error ",
            "INFO" | "DEBUG" => "",
            "TRACE" if self.detailed => "",
            _ => return None,
        };
        let message = event["message"].as_str().unwrap_or_default();
        if message == "ignored"
            || self
                .messages
                .as_ref()
                .is_some_and(|messages| !messages.iter().any(|wanted| wanted == message))
        {
            return None;
        }
        let masked: Vec<&str> = self
            .masks
            .iter()
            .filter(|mask| (mask.when)(event))
            .flat_map(|mask| mask.fields.iter().map(String::as_str))
            .collect();
        let said = format!("{mark}{area}: {message}");
        let mut line = said.clone();
        for (key, value) in event.as_object().into_iter().flatten() {
            if UNSTORIED.contains(&key.as_str()) || key.ends_with("_ms") || key.ends_with("_us") {
                continue;
            }
            match value {
                _ if masked.contains(&key.as_str()) => line.push_str(&format!(" {key}=…")),
                serde_json::Value::String(text) => line.push_str(&format!(" {key}={text}")),
                other => line.push_str(&format!(" {key}={other}")),
            }
        }
        Some((said, line))
    }
}

impl std::fmt::Display for Story<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Each engine's lines, each with the part that starts it.
        let mut engines: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for event in self
            .traced
            .events()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        {
            let engine = event["engine"].as_str().unwrap_or("?").to_owned();
            if self.engine.as_ref().is_some_and(|wanted| *wanted != engine) {
                continue;
            }
            let Some(told) = self.line(&event) else {
                continue;
            };
            match engines.iter_mut().find(|(name, _)| *name == engine) {
                Some((_, lines)) => lines.push(told),
                None => engines.push((engine, vec![told])),
            }
        }
        engines.sort_by(|a, b| a.0.cmp(&b.0));
        for (engine, mut lines) in engines {
            if self.concurrent {
                for run in lines.chunk_by_mut(|a, b| a.0 == b.0) {
                    run.sort();
                }
            }
            writeln!(formatter, "{engine}:")?;
            for (_, line) in lines {
                writeln!(formatter, "  {line}")?;
            }
        }
        Ok(())
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

/// Asserts the story `tell` gives is `expected`, once it is whole: the
/// engines record on threads of their own, so what a test waited for can
/// be told before the records after it are. It asks again until the story
/// has as many lines as `expected`, for at most [`crate::DEFAULT_WAIT`];
/// a story that differs is then shown as it stands.
#[track_caller]
pub fn assert_story(expected: expect_test::Expect, mut tell: impl FnMut() -> String) {
    let lines = |text: &str| text.lines().filter(|line| !line.trim().is_empty()).count();
    let wanted = lines(expected.data());
    let started = std::time::Instant::now();
    let mut story = tell();
    while lines(&story) < wanted && started.elapsed() < crate::DEFAULT_WAIT {
        std::thread::sleep(std::time::Duration::from_millis(1));
        story = tell();
    }
    expected.assert_eq(&story);
}
