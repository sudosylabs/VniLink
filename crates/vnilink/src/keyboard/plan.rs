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
            // An uncommitted dead key must not be injected and then committed
            // again as text. Map mode still sends the physical key.
            if key.composing
                && mode != KeyboardMode::Map
                && key.text.is_empty()
                && !self.command_held()
            {
                return Ok(planned);
            }
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
            // AppKit/GTK deliver repeats as further downs. Keep one held
            // entry for cleanup, but do not swallow Backspace/navigation or
            // physical-key repeats. Modifier synchronization is idempotent.
            if !is_level(usage) && !is_command(usage) {
                planned.push(Planned::Event(self.key_event(usage, true)));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_navigation_keys_repeat_and_release_once() {
        let mut plan = KeyPlan::new();
        let press = LocalKey {
            usage: 42,
            down: true,
            text: "",
            composing: false,
        };
        for mode in [KeyboardMode::Character, KeyboardMode::Map] {
            plan.clear();
            for _ in 0..3 {
                assert_eq!(
                    events(&plan.apply(mode, false, press).unwrap()),
                    vec![InputEvent::key_down(42)]
                );
            }
            assert_eq!(
                events(&plan.apply(mode, false, up(42)).unwrap()),
                vec![InputEvent::key_up(42)]
            );
            assert!(!plan.is_held());
        }
    }

    fn down<'a>(usage: u16, text: &'a str) -> LocalKey<'a> {
        LocalKey::new(usage, true, text)
    }

    fn up(usage: u16) -> LocalKey<'static> {
        LocalKey::new(usage, false, "")
    }

    fn events(planned: &[Planned]) -> Vec<InputEvent> {
        planned
            .iter()
            .filter_map(|item| match item {
                Planned::Event(event) => Some(*event),
                Planned::Text(_) | Planned::Chord(_) => None,
            })
            .collect()
    }

    #[test]
    fn character_mode_inserts_at_from_either_chord() {
        let mut plan = KeyPlan::new();
        let mac = plan
            .apply(KeyboardMode::Character, false, down(usage::GRAVE, "@"))
            .unwrap();
        assert_eq!(mac, vec![Planned::Text("@".to_owned())]);
        assert!(plan
            .apply(KeyboardMode::Character, false, up(usage::GRAVE))
            .unwrap()
            .is_empty());

        plan.apply(KeyboardMode::Character, false, down(usage::RIGHT_ALT, ""))
            .unwrap();
        let pc = plan
            .apply(KeyboardMode::Character, false, down(usage::DIGIT_0, "@"))
            .unwrap();
        assert_eq!(pc, vec![Planned::Text("@".to_owned())]);
        assert!(events(
            &plan
                .apply(KeyboardMode::Character, false, up(usage::DIGIT_0))
                .unwrap()
        )
        .is_empty());
        assert!(events(
            &plan
                .apply(KeyboardMode::Character, false, up(usage::RIGHT_ALT))
                .unwrap()
        )
        .is_empty());
    }

    #[test]
    fn character_mode_inserts_the_french_m_and_keeps_shortcuts() {
        let mut plan = KeyPlan::new();
        let typed = plan
            .apply(KeyboardMode::Character, false, down(usage::SEMICOLON, "m"))
            .unwrap();
        assert_eq!(typed, vec![Planned::Text("m".to_owned())]);

        let mut plan = KeyPlan::new();
        let command = plan
            .apply(KeyboardMode::Character, true, down(usage::LEFT_META, ""))
            .unwrap();
        let letter = plan
            .apply(KeyboardMode::Character, true, down(usage::C, "c"))
            .unwrap();
        assert_eq!(
            events(&[command, letter].concat()),
            vec![
                InputEvent::key_down(usage::LEFT_CONTROL),
                InputEvent::key_down(usage::C),
            ]
        );
    }

    #[test]
    fn shift_is_a_key_for_selection_and_not_for_a_symbol() {
        let mut plan = KeyPlan::new();
        assert!(plan
            .apply(KeyboardMode::Character, false, down(usage::LEFT_SHIFT, ""))
            .unwrap()
            .is_empty());
        let symbol = plan
            .apply(KeyboardMode::Character, false, down(usage::GRAVE, "#"))
            .unwrap();
        assert_eq!(symbol, vec![Planned::Text("#".to_owned())]);
        let arrow = plan
            .apply(KeyboardMode::Character, false, down(usage::LEFT, ""))
            .unwrap();
        assert_eq!(
            arrow,
            vec![
                Planned::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
                Planned::Event(InputEvent::key_down(usage::LEFT)),
            ]
        );
    }

    #[test]
    fn map_sends_the_position_and_translate_sends_a_chord() {
        let mut plan = KeyPlan::new();
        let mapped = plan
            .apply(KeyboardMode::Map, false, down(usage::GRAVE, "@"))
            .unwrap();
        assert_eq!(
            mapped,
            vec![Planned::Event(InputEvent::key_down(usage::GRAVE))]
        );

        let mut plan = KeyPlan::new();
        let translated = plan
            .apply(KeyboardMode::Translate, false, down(usage::GRAVE, "@"))
            .unwrap();
        assert_eq!(translated, vec![Planned::Chord("@".to_owned())]);
        let shortcut = plan
            .apply(
                KeyboardMode::Translate,
                false,
                down(usage::LEFT_CONTROL, ""),
            )
            .unwrap();
        let letter = plan
            .apply(KeyboardMode::Translate, false, down(usage::C, "c"))
            .unwrap();
        assert_eq!(
            events(&[shortcut, letter].concat()),
            vec![
                InputEvent::key_down(usage::LEFT_CONTROL),
                InputEvent::key_down(usage::C),
            ]
        );
    }

    #[test]
    fn enter_and_a_missing_character_stay_physical() {
        let mut plan = KeyPlan::new();
        let entered = plan
            .apply(KeyboardMode::Character, false, down(usage::ENTER, "\n"))
            .unwrap();
        assert_eq!(
            entered,
            vec![Planned::Event(InputEvent::key_down(usage::ENTER))]
        );
        let bare = plan
            .apply(KeyboardMode::Character, false, down(usage::A, ""))
            .unwrap();
        assert_eq!(bare, vec![Planned::Event(InputEvent::key_down(usage::A))]);
    }

    #[test]
    fn uncommitted_dead_key_does_not_duplicate_the_final_composed_character() {
        for mode in [KeyboardMode::Character, KeyboardMode::Translate] {
            let mut plan = KeyPlan::new();
            assert!(plan
                .apply(mode, false, LocalKey::composing(usage::APOSTROPHE, true))
                .unwrap()
                .is_empty());
            assert!(plan
                .apply(mode, false, LocalKey::composing(usage::APOSTROPHE, false))
                .unwrap()
                .is_empty());
            let committed = plan
                .apply(mode, false, LocalKey::new(usage::E, true, "é"))
                .unwrap();
            assert_eq!(
                committed,
                vec![if mode == KeyboardMode::Character {
                    Planned::Text("é".into())
                } else {
                    Planned::Chord("é".into())
                }]
            );
            assert!(plan
                .apply(mode, false, LocalKey::new(usage::E, false, ""))
                .unwrap()
                .is_empty());
            assert!(!plan.is_held());
        }
        let mut plan = KeyPlan::new();
        assert_eq!(
            events(
                &plan
                    .apply(
                        KeyboardMode::Map,
                        false,
                        LocalKey::composing(usage::APOSTROPHE, true)
                    )
                    .unwrap()
            ),
            vec![InputEvent::key_down(usage::APOSTROPHE)]
        );
        assert_eq!(
            events(
                &plan
                    .apply(
                        KeyboardMode::Map,
                        false,
                        LocalKey::composing(usage::APOSTROPHE, false)
                    )
                    .unwrap()
            ),
            vec![InputEvent::key_up(usage::APOSTROPHE)]
        );
    }
}
