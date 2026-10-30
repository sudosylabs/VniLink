// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The VniLink engine's log: what the crates record through `tracing`
//! (see `vnilink::trace`), filtered, written to the engine's own log file
//! and kept in a bounded flight recorder for Export Diagnostics.
//!
//! It stands on its own: the engine process, the test kit, examples and
//! command-line tools install it, and none needs the application. One
//! filter, in `tracing`'s directive syntax (`vnilink=info`,
//! `vnilink::files=debug,vnilink::camera=trace`, or `off`), chooses what
//! the log file and standard error take. It comes from [`LOG_ENV`] when
//! set, else from the application's setting ([`set_filter`]), else
//! [`DEFAULT_FILTER`], and changes while the engine runs.
//!
//! The flight recorder keeps VniLink's own events from debug level, plus
//! whatever more the filter asks for, and nothing when the filter is `off`.
//! Libraries' events (iroh's, which can name endpoints and addresses) reach
//! the log file at warning level and above only, and are never recorded for
//! export.
//!
//! Each event becomes one JSON line: `ms` (milliseconds since the log was
//! installed, never wall time), `level`, `target` and the event's fields.
//! The crates record no screen content, typed text, key identities, pointer
//! positions, clipboard or terminal content, secrets or absolute paths, so
//! neither does this.

mod file;
mod filter;
mod line;
mod recorder;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::FilterExt;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{reload, EnvFilter};

pub use filter::{FilterSource, FilterState, InvalidFilter, DEFAULT_FILTER, MAX_FILTER_BYTES};
pub use recorder::MAX_EXPORT_BYTES;

/// The filter that takes precedence over the application's setting.
pub const LOG_ENV: &str = "VNILINK_LOG";
/// A folder for log files in place of the one the engine chooses.
pub const LOG_DIR_ENV: &str = "VNILINK_LOG_DIR";
/// `1` also writes the filtered events, readably, to standard error.
pub const LOG_STDERR_ENV: &str = "VNILINK_LOG_STDERR";

type Reload = Box<dyn Fn(EnvFilter) -> bool + Send + Sync>;

struct Log {
    started: Instant,
    stderr: bool,
    file: Mutex<Option<file::LogFile>>,
    recorder: Mutex<recorder::Recorder>,
    filter: Mutex<filter::Filters>,
    /// The output's filter, and the recorder's two: VniLink's own events
    /// and, on top, a copy of the output's.
    reload: OnceLock<[Reload; 3]>,
}

fn log() -> &'static Log {
    static LOG: OnceLock<Log> = OnceLock::new();
    LOG.get_or_init(|| Log {
        started: Instant::now(),
        stderr: std::env::var_os(LOG_STDERR_ENV).is_some_and(|value| value == "1"),
        file: Mutex::new(None),
        recorder: Mutex::new(recorder::Recorder::default()),
        filter: Mutex::new(filter::Filters::from_environment(
            std::env::var(LOG_ENV).ok().as_deref(),
        )),
        reload: OnceLock::new(),
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn ours(target: &str) -> bool {
    target == "vnilink" || target.starts_with("vnilink::") || target.starts_with("vnilink_")
}

/// Keeps what a new span gives its events ([`line::SpanFields`]).
fn on_new_span<S>(attrs: &Attributes<'_>, id: &Id, ctx: &Context<'_, S>)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(span) = ctx.span(id) else {
        return;
    };
    let mut extensions = span.extensions_mut();
    if extensions.get_mut::<line::SpanFields>().is_none() {
        let mut fields = line::SpanFields::default();
        attrs.record(&mut line::SpanVisit {
            span: span.name(),
            fields: &mut fields,
        });
        extensions.insert(fields);
    }
}

/// Updates it when the span records a field again, as an engine's name.
fn on_record<S>(id: &Id, values: &Record<'_>, ctx: &Context<'_, S>)
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(span) = ctx.span(id) else {
        return;
    };
    let name = span.name();
    let mut extensions = span.extensions_mut();
    if let Some(fields) = extensions.get_mut::<line::SpanFields>() {
        values.record(&mut line::SpanVisit { span: name, fields });
    }
}

/// What the spans around `event` give it, the outermost first.
fn inherited<S>(
    event: &Event<'_>,
    ctx: &Context<'_, S>,
) -> serde_json::Map<String, serde_json::Value>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let mut fields = serde_json::Map::new();
    if let Some(scope) = ctx.event_scope(event) {
        for span in scope.from_root() {
            if let Some(given) = span.extensions().get::<line::SpanFields>() {
                fields.extend(given.0.clone());
            }
        }
    }
    fields
}

/// The log file and standard error.
struct Output;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Output {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        on_new_span(attrs, id, &ctx);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        on_record(id, values, &ctx);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if !ours(metadata.target()) && *metadata.level() > Level::WARN {
            return;
        }
        let log = log();
        let mut file = lock(&log.file);
        if file.is_none() && !log.stderr {
            return;
        }
        let line = line::Line::of(event, log.started, inherited(event, &ctx));
        if let Some(file) = file.as_mut() {
            file.write(&line.json());
        }
        drop(file);
        if log.stderr {
            eprintln!("{}", line.readable());
        }
    }
}

/// The flight recorder.
struct Recording;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Recording {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        on_new_span(attrs, id, &ctx);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        on_record(id, values, &ctx);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !ours(event.metadata().target()) {
            return;
        }
        let log = log();
        let line = line::Line::of(event, log.started, inherited(event, &ctx));
        lock(&log.recorder).push(line.service(), line.json());
    }
}

/// Starts the log for this process; later calls do nothing. Another
/// subscriber already installed keeps its place, and this one then sees
/// nothing.
pub fn install() {
    let log = log();
    log.reload.get_or_init(|| {
        let filters = lock(&log.filter);
        let (output, output_handle) = reload::Layer::new(filters.output());
        let (recorded, recorded_handle) = reload::Layer::new(filters.recorded());
        let (also, also_handle) = reload::Layer::new(filters.output());
        let rejected = filters.rejected_environment();
        drop(filters);
        let subscriber = tracing_subscriber::registry()
            .with(Output.with_filter(output))
            .with(Recording.with_filter(recorded.or(also)));
        let installed = tracing::subscriber::set_global_default(subscriber).is_ok();
        if installed && rejected {
            tracing::warn!(
                target: "vnilink::log",
                "the filter in {LOG_ENV} is not valid; the default applies"
            );
        }
        [
            Box::new(move |filter| output_handle.reload(filter).is_ok()) as Reload,
            Box::new(move |filter| recorded_handle.reload(filter).is_ok()),
            Box::new(move |filter| also_handle.reload(filter).is_ok()),
        ]
    });
}

/// Applies the application's filter setting; `None` or empty asks for the
/// default. While [`LOG_ENV`] is set it wins, and the setting waits.
pub fn set_filter(setting: Option<&str>) -> Result<FilterState, InvalidFilter> {
    let log = log();
    let mut filters = lock(&log.filter);
    filters.set(setting)?;
    if let Some([output, recorded, also]) = log.reload.get() {
        output(filters.output());
        recorded(filters.recorded());
        also(filters.output());
    }
    let state = filters.state();
    drop(filters);
    tracing::info!(
        target: "vnilink::log",
        filter = state.directives.as_str(),
        source = state.source.as_str(),
        "filter applied"
    );
    Ok(state)
}

/// The filter in force and where it came from.
pub fn filter() -> FilterState {
    lock(&log().filter).state()
}

/// Writes the filtered events to `engine.log` in a new folder for this run
/// under `root`, or under [`LOG_DIR_ENV`] when set, keeping the most recent
/// runs. Once per process: later calls return the file already open.
pub fn write_to(root: &Path) -> io::Result<PathBuf> {
    let log = log();
    let mut file = lock(&log.file);
    if let Some(open) = file.as_ref() {
        return Ok(open.path().to_path_buf());
    }
    let root = std::env::var_os(LOG_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map_or_else(|| root.to_path_buf(), PathBuf::from);
    let opened = file::LogFile::open_run(&root)?;
    let path = opened.path().to_path_buf();
    *file = Some(opened);
    Ok(path)
}

/// The log file this run writes, if any.
pub fn log_file() -> Option<PathBuf> {
    lock(&log().file)
        .as_ref()
        .map(|file| file.path().to_path_buf())
}

/// The end of this run's log file: at most `max_bytes`, starting on a whole
/// line. Empty when there is no log file.
pub fn tail(max_bytes: usize) -> io::Result<String> {
    match log_file() {
        Some(path) => file::tail(&path, max_bytes),
        None => Ok(String::new()),
    }
}

/// The log's clock: milliseconds since it was installed, as each event's
/// `ms` says, so a caller can take the events after a moment of its own.
pub fn now_ms() -> u64 {
    u64::try_from(log().started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Everything the flight recorder holds, as JSON lines in time order: at
/// most [`MAX_EXPORT_BYTES`], the newest kept, with a first line saying how
/// many older events were left out when any were.
pub fn export() -> String {
    lock(&log().recorder).export()
}

#[cfg(test)]
mod tests;
