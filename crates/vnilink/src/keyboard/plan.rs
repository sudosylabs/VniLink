//! Which host action one local key becomes.

use crate::error::Error;
use crate::input::{self, usage, InputEvent, Outbound};

use super::{KeyboardMode, LocalKey};

/// One host action decided from a local key. Text is not logged by the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Planned {
    Event(InputEvent),
    Text(String),
    Chord(String),
}

impl Planned {
    /// The queue item this plan becomes. The map lives beside the plan so the
    /// controller only learns one vocabulary for both.
    pub(crate) fn outbound(self) -> Outbound {
        match self {
            Self::Event(event) => Outbound::Event(event),
            Self::Text(text) => Outbound::Text(text),
            Self::Chord(text) => Outbound::Chord(text),
        }
    }
}

/// Keys this controller is holding, and which of them were sent as positions.
#[derive(Clone)]
pub(crate) struct KeyPlan {
    swap: bool,
    local: Vec<u16>,
    sent: Vec<u16>,
    generation: u64,
}

impl KeyPlan {
    pub(crate) fn new() -> Self {
        Self {
            swap: false,
            local: Vec::new(),
            sent: Vec::new(),
            generation: 0,
        }
    }

    pub(crate) fn is_held(&self) -> bool {
        !self.local.is_empty() || !self.sent.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.local.clear();
        self.sent.clear();
    }

    /// Drops held state when the controlling session changes. The old host
    /// releases its own keys. This does not send those releases again.
    pub(crate) fn reset_if_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }

    pub(crate) fn apply(
        &mut self,
        mode: KeyboardMode,
        swap: bool,
        key: LocalKey<'_>,
    ) -> Result<Vec<Planned>, Error> {
        if !key.text.is_empty() {
            if let Some(message) = input::key_text_problem(key.text) {
                return Err(Error::InvalidSetting(message));
            }
        }
        self.swap = swap;
        let mut planned = Vec::new();
        if key.down {
            self.track_local(true, key.usage);
            if self.sends_character(mode, key.usage, key.text) {
                self.release_levels(&mut planned);
                planned.push(if mode == KeyboardMode::Translate {
                    Planned::Chord(key.text.to_owned())
                } else {
                    Planned::Text(key.text.to_owned())
                });
            } else if mode != KeyboardMode::Map && is_level(key.usage) && !self.command_held() {
                // Shift and Alt stay on the controller until a shortcut needs them.
            } else {
                if mode != KeyboardMode::Map {
                    self.sync_levels(&mut planned);
                }
                self.press(key.usage, &mut planned);
            }
        } else {
            self.track_local(false, key.usage);
            self.release(key.usage, &mut planned);
        }
        Ok(planned)
    }

    fn sends_character(&self, mode: KeyboardMode, usage: u16, text: &str) -> bool {
        mode != KeyboardMode::Map
            && !is_level(usage)
            && !is_command(usage)
            && !self.command_held()
            && is_typed_text(text)
    }

    fn command_held(&self) -> bool {
        self.local.iter().copied().any(is_command)
    }

    fn sync_levels(&mut self, planned: &mut Vec<Planned>) {
        for usage in [
            usage::LEFT_SHIFT,
            usage::RIGHT_SHIFT,
            usage::LEFT_ALT,
            usage::RIGHT_ALT,
        ] {
            let down = self.local.contains(&usage);
            let sent = self.sent.contains(&usage);
            if down && !sent {
                self.press(usage, planned);
            } else if !down && sent {
                self.release(usage, planned);
            }
        }
    }

    fn release_levels(&mut self, planned: &mut Vec<Planned>) {
        for usage in [
            usage::RIGHT_ALT,
            usage::LEFT_ALT,
            usage::RIGHT_SHIFT,
            usage::LEFT_SHIFT,
        ] {
            self.release(usage, planned);
        }
    }

    fn press(&mut self, usage: u16, planned: &mut Vec<Planned>) {
        if self.sent.contains(&usage) {
            return;
        }
        self.sent.push(usage);
        planned.push(Planned::Event(self.key_event(usage, true)));
    }

    fn release(&mut self, usage: u16, planned: &mut Vec<Planned>) {
        if !self.sent.contains(&usage) {
            return;
        }
        self.sent.retain(|held| *held != usage);
        planned.push(Planned::Event(self.key_event(usage, false)));
    }

    /// The physical key event with the Control/Command swap applied.
    fn key_event(&self, usage: u16, down: bool) -> InputEvent {
        let event = InputEvent::Key { usage, down };
        if self.swap {
            input::swap_command_key(event)
        } else {
            event
        }
    }

    /// Records a locally held usage for later release.
    fn track_local(&mut self, down: bool, usage: u16) {
        if down {
            if !self.local.contains(&usage) {
                self.local.push(usage);
            }
        } else {
            self.local.retain(|held| *held != usage);
        }
    }
}

fn is_command(usage: u16) -> bool {
    matches!(
        usage,
        usage::LEFT_CONTROL | usage::RIGHT_CONTROL | usage::LEFT_META | usage::RIGHT_META
    )
}

fn is_level(usage: u16) -> bool {
    matches!(
        usage,
        usage::LEFT_SHIFT | usage::RIGHT_SHIFT | usage::LEFT_ALT | usage::RIGHT_ALT
    )
}

fn is_typed_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| !character.is_control())
}
