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
            InputEvent::Pointer { .. }
            | InputEvent::Wheel { .. }
            | InputEvent::PreciseWheel { .. }
            | InputEvent::Motion { .. } => {}
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

    pub(crate) fn buttons_held(&self) -> bool {
        self.order
            .iter()
            .any(|held| matches!(held, Held::Button(_)))
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
    /// One display this stream will show. Ordered with input.
    Displays(Vec<u64>),
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
            Self::Displays(ids) => formatter.debug_tuple("Displays").field(ids).finish(),
        }
    }
}

/// Controller-side queue. Pointer samples replace the pending sample.
pub(crate) struct InputOutbox {
    queue: Mutex<VecDeque<Outbound>>,
    fault: Mutex<Option<InputFault>>,
    /// Last absolute pointer accepted into the queue. A later sample at the
    /// same point is not sent again. Cleared when pending input is dropped.
    last_pointer: Mutex<Option<(i32, i32, u64, u64)>>,
    notify: Notify,
}

impl InputOutbox {
    pub(crate) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            fault: Mutex::new(None),
            last_pointer: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    /// Pending item count for diagnostics. Payloads stay in the queue.
    pub(crate) fn depth(&self) -> usize {
        self.queue.lock().map(|queue| queue.len()).unwrap_or(0)
    }

    pub(crate) fn push(&self, event: InputEvent) -> Result<(), Error> {
        event.validate()?;
        self.push_item(Outbound::Event(event))
    }

    pub(crate) fn push_text(&self, text: String) -> Result<(), Error> {
        check_key_text(&text)?;
        self.push_item(Outbound::Text(text))
    }

    pub(crate) fn push_quality(&self, quality: Quality) -> Result<(), Error> {
        self.push_item(Outbound::Quality(quality))
    }

    pub(crate) fn push_displays(&self, ids: Vec<u64>) -> Result<(), Error> {
        self.push_item(Outbound::Displays(ids))
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
        if items
            .iter()
            .any(|item| matches!(item, Outbound::Event(InputEvent::Motion { .. })))
        {
            // A batch admits atomically: any motion in it moves the pointer
            // exactly like an item push does.
            *self
                .last_pointer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = None;
        }
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
        if let Some(point) = absolute_point(&item) {
            let mut last = self
                .last_pointer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if *last == Some(point) {
                return Ok(());
            }
            if let Some(back) = queue.back_mut() {
                if matches!(back, Outbound::Event(InputEvent::Pointer { .. })) {
                    *back = item;
                    *last = Some(point);
                    drop(last);
                    drop(queue);
                    self.notify.notify_one();
                    return Ok(());
                }
            }
            if queue.len() >= INPUT_QUEUE {
                if let Some(index) = queue.iter().position(|queued| {
                    matches!(queued, Outbound::Event(InputEvent::Pointer { .. }))
                }) {
                    queue.remove(index);
                } else {
                    return Ok(());
                }
            }
            queue.push_back(item);
            *last = Some(point);
            drop(last);
            drop(queue);
            self.notify.notify_one();
            return Ok(());
        }
        if fold_motion(&mut queue, &item) {
            // A folded motion still moves the pointer: the remembered
            // absolute position no longer describes it. A folded wheel
            // scrolls in place and invalidates nothing.
            if matches!(item, Outbound::Event(InputEvent::Motion { .. })) {
                *self
                    .last_pointer
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = None;
            }
            drop(queue);
            self.notify.notify_one();
            return Ok(());
        }
        if queue.len() >= INPUT_QUEUE {
            // Relative motion is a sample. A wheel that cannot fold stays
            // ordered with keys, so a full queue reports Busy instead of
            // discarding the scroll or a key. A dropped motion moved
            // nothing, so the remembered position stands.
            if matches!(item, Outbound::Event(InputEvent::Motion { .. })) {
                return Ok(());
            }
            return Err(Error::Busy);
        }
        if matches!(item, Outbound::Displays(_)) {
            // A previously refused point becomes meaningful after selecting a
            // source. It must be sent again even at unchanged coordinates.
            *self
                .last_pointer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = None;
        }
        if matches!(item, Outbound::Event(InputEvent::Motion { .. })) {
            // Admitted relative motion invalidates the remembered absolute
            // position: the next absolute pointer restores a position
            // rather than repeating it, so it must not be suppressed as a
            // duplicate. Only interchangeable motion folds.
            *self
                .last_pointer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = None;
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
        *self
            .last_pointer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
    }

    /// Focus loss discards pending input without discarding encoder settings
    /// or clearing a fault from an ended owner.
    pub(crate) fn clear_pending_input(&self) {
        self.queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .retain(|item| matches!(item, Outbound::Quality(_) | Outbound::Displays(_)));
        *self
            .last_pointer
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

fn absolute_point(item: &Outbound) -> Option<(i32, i32, u64, u64)> {
    match item {
        Outbound::Event(InputEvent::Pointer {
            x,
            y,
            display,
            revision,
        }) => Some((*x, *y, *display, *revision)),
        _ => None,
    }
}

/// Folds this sample into the pending sample of the same kind. `true` means
/// the queue already accounts for it, including a pair that cancelled to zero.
fn fold_motion(queue: &mut VecDeque<Outbound>, item: &Outbound) -> bool {
    let replacement = match (queue.back(), item) {
        (
            Some(Outbound::Event(InputEvent::Motion { dx: ox, dy: oy })),
            Outbound::Event(InputEvent::Motion { dx, dy }),
        ) => {
            let nx = ox.saturating_add(*dx);
            let ny = oy.saturating_add(*dy);
            Some(if nx == 0 && ny == 0 {
                None
            } else {
                Some(InputEvent::motion(nx, ny))
            })
        }
        (
            Some(Outbound::Event(InputEvent::Wheel { dx: ox, dy: oy })),
            Outbound::Event(InputEvent::Wheel { dx, dy }),
        ) => Some(signed_pair(
            i32::from(*ox).saturating_add(i32::from(*dx)),
            i32::from(*oy).saturating_add(i32::from(*dy)),
            |dx, dy| {
                InputEvent::wheel(
                    i16::try_from(dx).unwrap_or(if dx < 0 { i16::MIN } else { i16::MAX }),
                    i16::try_from(dy).unwrap_or(if dy < 0 { i16::MIN } else { i16::MAX }),
                )
            },
        )),
        (
            Some(Outbound::Event(InputEvent::PreciseWheel { dx: ox, dy: oy })),
            Outbound::Event(InputEvent::PreciseWheel { dx, dy }),
        ) => {
            let limit = super::MAX_PRECISE_WHEEL;
            Some(signed_pair(
                ox.saturating_add(*dx).clamp(-limit, limit),
                oy.saturating_add(*dy).clamp(-limit, limit),
                InputEvent::precise_wheel,
            ))
        }
        _ => None,
    };
    let Some(replacement) = replacement else {
        return false;
    };
    match replacement {
        None => {
            queue.pop_back();
        }
        Some(event) => {
            *queue
                .back_mut()
                .expect("the folded sample is still pending") = Outbound::Event(event);
        }
    }
    true
}

fn signed_pair(dx: i32, dy: i32, event: impl Fn(i32, i32) -> InputEvent) -> Option<InputEvent> {
    if dx == 0 && dy == 0 {
        None
    } else {
        Some(event(dx, dy))
    }
}

#[cfg(test)]
mod review_motion_tests {
    use super::*;
    #[test]
    fn review_absolute_pointer_after_relative_motion_is_not_suppressed() {
        let queue = InputOutbox::new();
        let pointer = InputEvent::pointer(100, 100, 1, 1);
        queue.push(pointer).unwrap();
        queue.drain();
        queue.push(InputEvent::Motion { dx: 10, dy: 0 }).unwrap();
        queue.push(pointer).unwrap();
        assert_eq!(
            queue.drain().len(),
            2,
            "absolute reposition suppressed after relative motion"
        );
    }
}

#[cfg(test)]
mod motion_invalidation_tests {
    use super::*;
    #[test]
    fn folded_queued_and_batch_motion_reposition_but_dropped_motion_does_not() {
        let pointer = InputEvent::pointer(100, 100, 1, 1);
        // Folded motion still moves the pointer.
        let queue = InputOutbox::new();
        queue.push(pointer).unwrap();
        queue.drain();
        queue.push(InputEvent::Motion { dx: 10, dy: 0 }).unwrap();
        queue.push(InputEvent::Motion { dx: 5, dy: 0 }).unwrap();
        queue.push(pointer).unwrap();
        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(
            matches!(drained[1], Outbound::Event(InputEvent::Pointer { .. })),
            "folded motion suppressed the reposition: {drained:?}"
        );
        // Batch motion admits atomically and invalidates identically.
        let queue = InputOutbox::new();
        queue.push(pointer).unwrap();
        queue.drain();
        queue
            .push_batch(&[Outbound::Event(InputEvent::Motion { dx: 1, dy: 1 })])
            .unwrap();
        queue.push(pointer).unwrap();
        assert_eq!(queue.drain().len(), 2);
        // A wheel scrolls in place: no invalidation, still suppressed.
        let queue = InputOutbox::new();
        queue.push(pointer).unwrap();
        queue.drain();
        queue.push(InputEvent::wheel(0, 3)).unwrap();
        queue.push(pointer).unwrap();
        assert_eq!(queue.drain().len(), 1);
        // Dropped motion on a full queue moved nothing: still suppressed.
        let queue = InputOutbox::new();
        queue.push(pointer).unwrap();
        queue.drain();
        for _ in 0..INPUT_QUEUE {
            queue
                .push(InputEvent::key_down(crate::input::usage::B))
                .unwrap();
        }
        queue.push(InputEvent::Motion { dx: 10, dy: 0 }).unwrap();
        queue.push(pointer).unwrap();
        assert_eq!(queue.drain().len(), INPUT_QUEUE);
    }
}
