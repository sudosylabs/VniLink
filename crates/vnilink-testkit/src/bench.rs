//! A performance suite's results: written as JSON to
//! `.build/bench/<service>/<run>.json` (or under `VNILINK_BENCH_DIR`), and
//! compared with the previous run on this computer, so "slower than
//! usual" is a number. A run records the platform and build profile, never
//! the computer's name or its user's.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// A change in this direction past [`REGRESSION`] is reported as a regression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
}

/// How far a measurement may move the wrong way before it is called a
/// regression: loopback runs on a shared computer vary by about this much.
pub const REGRESSION: f64 = 0.15;

/// The folder runs go in when `VNILINK_BENCH_DIR` does not say.
const DEFAULT_FOLDER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.build/bench");

struct Measurement {
    name: String,
    unit: &'static str,
    value: f64,
    better: Better,
}

/// One run of one service's performance suite.
pub struct Report {
    service: String,
    measurements: Vec<Measurement>,
}

impl Report {
    pub fn new(service: &str) -> Self {
        Self {
            service: service.to_owned(),
            measurements: Vec::new(),
        }
    }

    /// Records one result and prints it.
    pub fn record(&mut self, name: &str, value: f64, unit: &'static str, better: Better) {
        println!("{name}: {value:.2} {unit}");
        self.measurements.push(Measurement {
            name: name.to_owned(),
            unit,
            value,
            better,
        });
    }

    fn folder(&self) -> PathBuf {
        std::env::var_os("VNILINK_BENCH_DIR")
            .filter(|folder| !folder.is_empty())
            .map_or_else(|| PathBuf::from(DEFAULT_FOLDER), PathBuf::from)
            .join(&self.service)
    }

    /// Writes this run, compares it with the previous one, and returns the
    /// measurements that regressed.
    pub fn finish(self) -> Vec<String> {
        let folder = self.folder();
        std::fs::create_dir_all(&folder).expect("the bench folder");
        let folder = std::fs::canonicalize(&folder).expect("the bench folder");
        let previous = latest_run(&folder);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let run = json!({
            "service": self.service,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "measurements": self.measurements.iter().map(|measurement| json!({
                "name": measurement.name,
                "unit": measurement.unit,
                "value": measurement.value,
                "better": if measurement.better == Better::Higher { "higher" } else { "lower" },
            })).collect::<Vec<_>>(),
        });
        let path = folder.join(format!("{now}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&run).expect("JSON"))
            .expect("the run's results");
        println!("results: {}", path.display());
        let Some((name, previous)) = previous else {
            println!("no previous run to compare with");
            return Vec::new();
        };
        if previous.get("profile") != run.get("profile") {
            println!("the previous run ({name}) was another build profile; not compared");
            return Vec::new();
        }
        println!("compared with {name}:");
        let mut regressed = Vec::new();
        for measurement in &self.measurements {
            let Some(before) = previous_value(&previous, &measurement.name) else {
                continue;
            };
            if before == 0.0 {
                continue;
            }
            let change = (measurement.value - before) / before;
            let worse = match measurement.better {
                Better::Higher => -change,
                Better::Lower => change,
            };
            let verdict = if worse > REGRESSION {
                regressed.push(measurement.name.clone());
                "REGRESSED"
            } else if worse < -REGRESSION {
                "improved"
            } else {
                "unchanged"
            };
            println!(
                "  {}: {before:.2} -> {:.2} {} ({:+.0}%) {verdict}",
                measurement.name,
                measurement.value,
                measurement.unit,
                change * 100.0
            );
        }
        regressed
    }
}

/// The newest run in `folder`, by its name, which is its start time.
fn latest_run(folder: &std::path::Path) -> Option<(String, Value)> {
    let mut runs: Vec<(u128, PathBuf)> = std::fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let stamp = path.file_stem()?.to_str()?.parse().ok()?;
            Some((stamp, path))
        })
        .collect();
    runs.sort();
    let (_, path) = runs.pop()?;
    let run = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    Some((path.file_name()?.to_string_lossy().into_owned(), run))
}

fn previous_value(run: &Value, name: &str) -> Option<f64> {
    run.get("measurements")?
        .as_array()?
        .iter()
        .find(|measurement| measurement.get("name").and_then(Value::as_str) == Some(name))?
        .get("value")?
        .as_f64()
}

/// The median of `samples`; zero for none.
pub fn median(samples: &mut [Duration]) -> Duration {
    samples.sort();
    samples.get(samples.len() / 2).copied().unwrap_or_default()
}

/// The `percent` percentile of `samples`; zero for none.
pub fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
    samples.sort();
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples[(samples.len() * percent / 100).min(samples.len() - 1)]
}

/// Times `work`.
pub fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = work();
    (value, started.elapsed())
}
