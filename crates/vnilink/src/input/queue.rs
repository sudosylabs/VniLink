// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
    /// Drops queued input, keeping quality and display requests; returns
    /// how many inputs it dropped.
    pub(crate) fn clear_pending_input(&self) -> usize {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let before = queue.len();
        queue.retain(|item| matches!(item, Outbound::Quality(_) | Outbound::Displays(_)));
        let dropped = before - queue.len();
        drop(queue);
        *self
            .last_pointer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        dropped
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{usage, INPUT_QUEUE, MAX_KEY_TEXT};

    fn only_events(items: Vec<Outbound>) -> Vec<InputEvent> {
        items
            .into_iter()
            .map(|item| match item {
                Outbound::Event(event) => event,
                Outbound::Text(_)
                | Outbound::Chord(_)
                | Outbound::Quality(_)
                | Outbound::Displays(_) => {
                    panic!("a text item was queued with the keys")
                }
            })
            .collect()
    }

    #[test]
    fn pointer_samples_collapse_and_keys_do_not() {
        let outbox = InputOutbox::new();
        outbox.push(InputEvent::key_down(usage::A)).unwrap();
        outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
        outbox.push(InputEvent::pointer(8, 9, 1, 1)).unwrap();
        outbox.push(InputEvent::pointer(3, 4, 1, 1)).unwrap();
        outbox.push(InputEvent::key_up(usage::A)).unwrap();
        assert_eq!(
            only_events(outbox.drain()),
            vec![
                InputEvent::key_down(usage::A),
                InputEvent::pointer(3, 4, 1, 1),
                InputEvent::key_up(usage::A),
            ]
        );
    }

    #[test]
    fn ending_a_session_releases_held_keys_and_buttons_in_reverse() {
        let mut held = HeldInput::default();
        held.observe(InputEvent::key_down(usage::LEFT_SHIFT));
        held.observe(InputEvent::button_down(PointerButton::Left));
        held.observe(InputEvent::pointer(10, 10, 1, 1));
        held.observe(InputEvent::key_down(usage::LEFT_SHIFT));
        assert_eq!(
            held.release_events(),
            vec![
                InputEvent::button_up(PointerButton::Left),
                InputEvent::key_up(usage::LEFT_SHIFT),
            ]
        );
    }

    #[test]
    fn a_full_queue_keeps_key_transitions_and_folds_pointer_samples() {
        let outbox = InputOutbox::new();
        for usage_code in 0..INPUT_QUEUE as u16 {
            outbox.push(InputEvent::key_down(usage_code)).unwrap();
        }
        assert!(matches!(
            outbox.push(InputEvent::key_down(usage::A)),
            Err(Error::Busy)
        ));
        outbox.push(InputEvent::pointer(1, 2, 1, 1)).unwrap();
        let drained = only_events(outbox.drain());
        assert_eq!(drained.len(), INPUT_QUEUE);
        assert!(drained
            .iter()
            .all(|event| matches!(event, InputEvent::Key { .. })));

        let outbox = InputOutbox::new();
        for usage_code in 0..(INPUT_QUEUE as u16 - 1) {
            outbox.push(InputEvent::key_down(usage_code)).unwrap();
        }
        outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
        outbox.push(InputEvent::pointer(2, 2, 1, 1)).unwrap();
        let drained = only_events(outbox.drain());
        assert_eq!(drained.len(), INPUT_QUEUE);
        assert!(matches!(
            drained.last(),
            Some(InputEvent::Pointer { x: 2, y: 2, .. })
        ));
        assert!(!drained
            .iter()
            .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1, .. })));

        let outbox = InputOutbox::new();
        outbox.push(InputEvent::key_down(1)).unwrap();
        outbox.push(InputEvent::pointer(1, 1, 1, 1)).unwrap();
        for usage_code in 2..INPUT_QUEUE as u16 {
            outbox.push(InputEvent::key_down(usage_code)).unwrap();
        }
        outbox.push(InputEvent::pointer(7, 8, 1, 1)).unwrap();
        let drained = only_events(outbox.drain());
        assert_eq!(drained.len(), INPUT_QUEUE);
        assert!(matches!(drained[0], InputEvent::Key { usage: 1, .. }));
        assert!(matches!(
            drained.last(),
            Some(InputEvent::Pointer { x: 7, y: 8, .. })
        ));
        assert!(!drained
            .iter()
            .any(|event| matches!(event, InputEvent::Pointer { x: 1, y: 1, .. })));
    }

    #[test]
    fn the_first_fault_sticks_and_later_pushes_return_it() {
        let outbox = InputOutbox::new();
        outbox.fail(&Error::InputFailed(
            "the operating system rejected the event",
        ));
        outbox.fail(&Error::Unsupported("no desktop adapter is installed"));
        assert!(matches!(
            outbox.push(InputEvent::pointer(1, 1, 1, 1)),
            Err(Error::InputFailed(
                "the operating system rejected the event"
            ))
        ));
        assert!(outbox.drain().is_empty());

        let outbox = InputOutbox::new();
        outbox.fail(&Error::StaleSession);
        assert!(matches!(
            outbox.push(InputEvent::key_down(usage::A)),
            Err(Error::StaleSession)
        ));

        let outbox = InputOutbox::new();
        outbox.fail(&Error::Malformed);
        assert!(matches!(
            outbox.push(InputEvent::key_up(usage::A)),
            Err(Error::Malformed)
        ));

        let outbox = InputOutbox::new();
        outbox.fail(&Error::Busy);
        assert!(matches!(
            outbox.push(InputEvent::key_down(usage::A)),
            Err(Error::InputFailed("input was rejected"))
        ));
    }

    #[test]
    fn releasing_a_key_or_button_removes_it_from_the_held_set() {
        let mut held = HeldInput::default();
        held.observe(InputEvent::key_down(usage::A));
        held.observe(InputEvent::button_down(PointerButton::Left));
        held.observe(InputEvent::key_up(usage::A));
        held.observe(InputEvent::button_up(PointerButton::Left));
        held.observe(InputEvent::pointer(4, 5, 1, 1));
        held.observe(InputEvent::wheel(1, -1));
        assert!(held.release_events().is_empty());

        held.observe(InputEvent::key_down(usage::A));
        held.observe(InputEvent::key_down(usage::B));
        held.observe(InputEvent::button_down(PointerButton::Right));
        held.observe(InputEvent::key_up(usage::A));
        held.observe(InputEvent::button_down(PointerButton::Right));
        assert_eq!(
            held.release_events(),
            vec![
                InputEvent::button_up(PointerButton::Right),
                InputEvent::key_up(usage::B),
            ]
        );
    }

    #[test]
    fn finished_text_is_bounded_and_cleared_with_the_queue() {
        assert!(check_key_text("héllo").is_ok());
        assert!(check_key_text(&"a".repeat(MAX_KEY_TEXT)).is_ok());
        assert!(matches!(
            check_key_text(""),
            Err(Error::InvalidSetting("text entry is empty"))
        ));
        assert!(matches!(
            check_key_text(&"a".repeat(MAX_KEY_TEXT + 1)),
            Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
        ));
        assert!(matches!(
            check_key_text("a\0b"),
            Err(Error::InvalidSetting("text entry contains a null byte"))
        ));

        let outbox = InputOutbox::new();
        outbox.push(InputEvent::key_down(usage::A)).unwrap();
        outbox.push_text("héllo".to_owned()).unwrap();
        outbox.push(InputEvent::key_up(usage::A)).unwrap();
        let drained = outbox.drain();
        assert!(!format!("{drained:?}").contains("héllo"));
        assert_eq!(
            drained,
            vec![
                Outbound::Event(InputEvent::key_down(usage::A)),
                Outbound::Text("héllo".to_owned()),
                Outbound::Event(InputEvent::key_up(usage::A)),
            ]
        );

        let outbox = InputOutbox::new();
        outbox.push_text("gone".to_owned()).unwrap();
        outbox.clear();
        assert!(outbox.drain().is_empty());
        for _ in 0..INPUT_QUEUE {
            outbox.push(InputEvent::key_down(usage::A)).unwrap();
        }
        assert!(matches!(
            outbox.push_text("late".to_owned()),
            Err(Error::Busy)
        ));
    }

    #[test]
    fn selecting_a_display_allows_an_unchanged_pointer_retry_and_survives_focus_loss() {
        let outbox = crate::input::InputOutbox::new();
        let point = InputEvent::pointer(1, 2, 1, 1);
        outbox.push(point).unwrap();
        assert_eq!(outbox.drain(), vec![crate::input::Outbound::Event(point)]);
        outbox.push(point).unwrap();
        assert!(outbox.drain().is_empty());
        outbox.push_displays(vec![1]).unwrap();
        outbox.push(point).unwrap();
        assert_eq!(
            outbox.drain(),
            vec![
                crate::input::Outbound::Displays(vec![1]),
                crate::input::Outbound::Event(point)
            ]
        );
        outbox.push_displays(vec![1]).unwrap();
        outbox.push(InputEvent::key_down(usage::A)).unwrap();
        assert_eq!(
            outbox.clear_pending_input(),
            1,
            "the one queued key is dropped"
        );
        assert_eq!(
            outbox.drain(),
            vec![crate::input::Outbound::Displays(vec![1])]
        );
    }
}
