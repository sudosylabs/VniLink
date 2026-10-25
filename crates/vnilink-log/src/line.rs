//! One event as a log line.

use std::time::Instant;

use serde_json::{Map, Value};
use tracing::field::{Field, Visit};
use tracing::Event;

/// The keys every line has.
const RESERVED: [&str; 3] = ["ms", "level", "target"];

pub(crate) struct Line {
    ms: u64,
    level: &'static str,
    target: String,
    fields: Map<String, Value>,
    service: Option<u64>,
}

impl Visit for Line {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "service" {
            self.service = Some(value);
        }
        self.fields.insert(field.name().into(), value.into());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name().into(), value.into());
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name().into(), value.into());
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.insert(field.name().into(), value.into());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().into(), value.into());
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().into(), format!("{value:?}").into());
    }
}

impl Line {
    pub(crate) fn of(event: &Event<'_>, started: Instant) -> Self {
        let metadata = event.metadata();
        let mut line = Self {
            ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            level: metadata.level().as_str(),
            target: metadata.target().to_owned(),
            fields: Map::new(),
            service: None,
        };
        event.record(&mut line);
        line
    }

    pub(crate) fn ms(&self) -> u64 {
        self.ms
    }

    /// The desktop service the event names, as its field `service`.
    pub(crate) fn service(&self) -> Option<u64> {
        self.service
    }

    /// `{"ms":…,"level":…,"target":…,<fields>}`. A field named like one of
    /// the line's own keys is kept as `field.<name>`, so it never replaces
    /// when the event happened.
    pub(crate) fn json(&self) -> String {
        let mut object = Map::with_capacity(self.fields.len() + 3);
        for (name, value) in &self.fields {
            let name = if RESERVED.contains(&name.as_str()) {
                format!("field.{name}")
            } else {
                name.clone()
            };
            object.insert(name, value.clone());
        }
        object.insert("ms".into(), self.ms.into());
        object.insert("level".into(), self.level.into());
        object.insert("target".into(), self.target.as_str().into());
        Value::Object(object).to_string()
    }

    /// `ms LEVEL target: message name=value…`, for a terminal.
    pub(crate) fn readable(&self) -> String {
        let mut text = format!("{:>9} {:<5} {}:", self.ms, self.level, self.target);
        if let Some(Value::String(message)) = self.fields.get("message") {
            text.push(' ');
            text.push_str(message);
        }
        for (name, value) in &self.fields {
            if name != "message" {
                text.push_str(&format!(" {name}={value}"));
            }
        }
        text
    }
}
