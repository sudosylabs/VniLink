//! Controller queue. Pointer samples replace the pending sample.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::error::Error;
use crate::protocol::Quality;

use super::events::{check_key_text, InputEvent, PointerButton};
use super::INPUT_QUEUE;

/// Keys and buttons this session has pressed and not yet released.
#[derive(Debug, Default)]
pub(crate) struct HeldInput {
    order: Vec<Held>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Key(u16),
    Button(PointerButton),
}

impl HeldInput {
    pub(crate) fn observe(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { usage, down: true } => self.push(Held::Key(usage)),
            InputEvent::Key { usage, down: false } => self.remove(Held::Key(usage)),
            InputEvent::Button { button, down: true } => self.push(Held::Button(button)),
            InputEvent::Button {
                button,
                down: false,
            } => self.remove(Held::Button(button)),
            InputEvent::Pointer { .. } | InputEvent::Wheel { .. } => {}
        }
    }

    pub(crate) fn release_events(&self) -> Vec<InputEvent> {
        self.order
            .iter()
            .rev()
            .map(|held| match *held {
                Held::Key(usage) => InputEvent::key_up(usage),
                Held::Button(button) => InputEvent::button_up(button),
            })
            .collect()
    }

    fn push(&mut self, held: Held) {
        if !self.order.contains(&held) {
            self.order.push(held);
        }
    }

    fn remove(&mut self, held: Held) {
        self.order.retain(|seen| *seen != held);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum InputFault {
    Unsupported(&'static str),
    Stale,
    Failed(&'static str),
    Malformed,
}

impl InputFault {
    pub(crate) fn from_error(error: &Error) -> Self {
        match error {
            Error::Unsupported(message) => Self::Unsupported(message),
            Error::StaleSession => Self::Stale,
            Error::InputFailed(message) => Self::Failed(message),
            Error::Malformed => Self::Malformed,
            _ => Self::Failed("input was rejected"),
        }
    }

    pub(crate) fn error(self) -> Error {
        match self {
            Self::Unsupported(message) => Error::Unsupported(message),
            Self::Stale => Error::StaleSession,
            Self::Failed(message) => Error::InputFailed(message),
            Self::Malformed => Error::Malformed,
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unsupported(message) | Self::Failed(message) => message,
            Self::Stale => "the input belongs to an old session",
            Self::Malformed => "the control message is malformed",
        }
    }
}

/// One queued control item. Text is ordered with keys and is not logged.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Outbound {
    Event(InputEvent),
    Text(String),
    /// Character the host must type with its own layout's chord.
    Chord(String),
    /// Encoder limits the host must apply. Ordered with input so the host
    /// applies it in the sequence the controller intended.
    Quality(Quality),
}

impl fmt::Debug for Outbound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Event(event) => formatter.debug_tuple("Event").field(event).finish(),
            Self::Text(text) => formatter
                .debug_struct("Text")
                .field("len", &text.len())
                .finish(),
            Self::Chord(text) => formatter
                .debug_struct("Chord")
                .field("len", &text.len())
                .finish(),
            Self::Quality(quality) => formatter.debug_tuple("Quality").field(quality).finish(),
        }
    }
}

/// Controller-side queue. Pointer samples replace the pending sample.
pub(crate) struct InputOutbox {
    queue: Mutex<VecDeque<Outbound>>,
    fault: Mutex<Option<InputFault>>,
    notify: Notify,
}

impl InputOutbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            fault: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    pub(crate) fn push(&self, event: InputEvent) -> Result<(), Error> {
        self.push_item(Outbound::Event(event))
    }

    pub(crate) fn push_text(&self, text: String) -> Result<(), Error> {
        check_key_text(&text)?;
        self.push_item(Outbound::Text(text))
    }

    pub(crate) fn push_quality(&self, quality: Quality) -> Result<(), Error> {
        self.push_item(Outbound::Quality(quality))
    }

    /// Queues every item, or none of them. A full queue returns [`Error::Busy`].
    pub(crate) fn push_batch(&self, items: &[Outbound]) -> Result<(), Error> {
        if items.is_empty() {
            return Ok(());
        }
        if let Some(fault) = self.fault() {
            return Err(fault.error());
        }
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if queue.len() + items.len() > INPUT_QUEUE {
            return Err(Error::Busy);
        }
        queue.extend(items.iter().cloned());
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    fn push_item(&self, item: Outbound) -> Result<(), Error> {
        if let Some(fault) = self.fault() {
            return Err(fault.error());
        }
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Outbound::Event(InputEvent::Pointer { .. }) = item {
            if let Some(last) = queue.back_mut() {
                if matches!(last, Outbound::Event(InputEvent::Pointer { .. })) {
                    *last = item;
                    self.notify.notify_one();
                    return Ok(());
                }
            }
        }
        if queue.len() >= INPUT_QUEUE {
            if matches!(item, Outbound::Event(InputEvent::Pointer { .. })) {
                if let Some(index) = queue.iter().position(|queued| {
                    matches!(queued, Outbound::Event(InputEvent::Pointer { .. }))
                }) {
                    queue.remove(index);
                } else {
                    return Ok(());
                }
            } else {
                return Err(Error::Busy);
            }
        }
        queue.push_back(item);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    /// Drops queued events and any stored fault. Used before a new generation.
    pub(crate) fn clear(&self) {
        self.queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
        *self
            .fault
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    pub(crate) fn drain(&self) -> Vec<Outbound> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        queue.drain(..).collect()
    }

    pub(crate) fn fail(&self, error: &Error) {
        let mut fault = self
            .fault
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if fault.is_none() {
            *fault = Some(InputFault::from_error(error));
        }
    }

    pub(crate) fn fault(&self) -> Option<InputFault> {
        *self
            .fault
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}
