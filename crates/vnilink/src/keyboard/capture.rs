//! Which keys a controller takes from its own operating system while it
//! controls another computer: the shortcuts this system or its window
//! manager would act on here, such as switching applications. A platform hook
//! asks [`SystemKeys`] about every key it sees and sends the keys it takes to
//! the desktop as physical keys; it leaves the rest to the window, which
//! sends them with what they type.

use crate::input::usage;
use crate::OperatingSystem;

/// The keys taken so far and the modifiers held, as one hook sees them.
///
/// Every transition is shown to [`SystemKeys::take`], taken or not, so the
/// held modifiers stay true. A key whose press was taken has its release
/// taken too, whatever is held by then.
#[derive(Debug, Clone)]
pub struct SystemKeys {
    system: OperatingSystem,
    held: Vec<u16>,
    taken: Vec<u16>,
}

impl SystemKeys {
    /// The policy for the shortcuts of `system`, the controller's own.
    pub fn new(system: OperatingSystem) -> Self {
        Self {
            system,
            held: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// Whether this transition of the key `usage` is taken from this system.
    pub fn take(&mut self, usage: u16, down: bool) -> bool {
        if !down {
            self.held.retain(|held| *held != usage);
            let taken = self.taken.contains(&usage);
            self.taken.retain(|held| *held != usage);
            return taken;
        }
        if is_modifier(usage) && !self.held.contains(&usage) {
            self.held.push(usage);
        }
        let take = self.takes(usage);
        if take && !self.taken.contains(&usage) {
            self.taken.push(usage);
        }
        take
    }

    /// Keys whose press was taken and whose release has not come yet.
    pub fn taken(&self) -> &[u16] {
        &self.taken
    }

    fn held(&self, either: [u16; 2]) -> bool {
        self.held.iter().any(|held| either.contains(held))
    }

    fn takes(&self, key: u16) -> bool {
        let meta = self.held([usage::LEFT_META, usage::RIGHT_META]);
        let control = self.held([usage::LEFT_CONTROL, usage::RIGHT_CONTROL]);
        let alt = self.held([usage::LEFT_ALT, usage::RIGHT_ALT]);
        match self.system {
            // Command and Control shortcuts belong to the system, as with
            // Command-Tab and Control-arrows, or to this application's menus.
            // The modifiers themselves still reach the window.
            OperatingSystem::Macos => !is_modifier(key) && (meta || control),
            // The Windows or Super key, and whatever is pressed with it, opens
            // the system's menus here; Alt and Control with Tab, Escape, Space
            // and F4 switch, close or open windows here.
            OperatingSystem::Windows | OperatingSystem::Linux | OperatingSystem::Other => {
                matches!(key, usage::LEFT_META | usage::RIGHT_META)
                    || meta && !is_modifier(key)
                    || alt && matches!(key, usage::TAB | usage::ESCAPE | usage::SPACE | usage::F4)
                    || control && key == usage::ESCAPE
            }
        }
    }
}

fn is_modifier(key: u16) -> bool {
    (usage::LEFT_CONTROL..=usage::RIGHT_META).contains(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presses `keys` in order and lets them go in reverse, saying which were taken.
    fn chord(keys: &mut SystemKeys, chord: &[u16]) -> (Vec<bool>, Vec<bool>) {
        let pressed = chord.iter().map(|key| keys.take(*key, true)).collect();
        let released = chord
            .iter()
            .rev()
            .map(|key| keys.take(*key, false))
            .collect();
        (pressed, released)
    }

    #[test]
    fn a_pc_keeps_its_window_switching_and_start_menu_for_the_other_computer() {
        let mut keys = SystemKeys::new(OperatingSystem::Windows);
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_ALT, usage::TAB]),
            (vec![false, true], vec![true, false]),
            "Alt reaches the window; Tab is taken while Alt is held"
        );
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_META, usage::E]),
            (vec![true, true], vec![true, true]),
            "the Windows key and what is pressed with it are taken"
        );
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_CONTROL, usage::ESCAPE]),
            (vec![false, true], vec![true, false]),
            "Control-Escape would open Start here"
        );
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_CONTROL, usage::Z]),
            (vec![false, false], vec![false, false]),
            "Control-Z is the window's: it types nothing here"
        );
        assert!(keys.taken().is_empty());
    }

    #[test]
    fn a_mac_keeps_command_and_control_shortcuts_but_not_the_modifiers() {
        let mut keys = SystemKeys::new(OperatingSystem::Macos);
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_META, usage::TAB]),
            (vec![false, true], vec![true, false]),
            "Command reaches the window; Tab is taken while it is held"
        );
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_CONTROL, usage::RIGHT]),
            (vec![false, true], vec![true, false]),
            "Control-arrow would change Spaces here"
        );
        assert_eq!(
            chord(&mut keys, &[usage::LEFT_SHIFT, usage::A]),
            (vec![false, false], vec![false, false]),
            "typing stays the window's"
        );
    }

    #[test]
    fn a_taken_key_is_released_once_however_the_modifiers_end() {
        let mut keys = SystemKeys::new(OperatingSystem::Windows);
        assert!(!keys.take(usage::LEFT_ALT, true));
        assert!(keys.take(usage::TAB, true));
        assert!(keys.take(usage::TAB, true), "a held key repeats as taken");
        assert!(!keys.take(usage::LEFT_ALT, false));
        assert_eq!(keys.taken(), [usage::TAB]);
        assert!(
            keys.take(usage::TAB, false),
            "Tab went down taken, so it comes up taken"
        );
        assert!(
            !keys.take(usage::TAB, false),
            "a second release was never pressed"
        );
        assert!(
            !keys.take(usage::TAB, true),
            "without Alt, Tab is the window's again"
        );
    }

    #[test]
    fn native_keys_name_the_usages_this_policy_reads() {
        for key in [
            usage::TAB,
            usage::ESCAPE,
            usage::LEFT_META,
            usage::RIGHT_META,
            usage::LEFT_ALT,
            usage::Z,
        ] {
            let codes = crate::os_input::key_for_usage(key).expect("a mapped key");
            assert_eq!(
                crate::usage_for_native_key(crate::os_input::platform_code(codes)),
                Some(key),
                "the hook reads back the key {key:#x}"
            );
        }
    }
}
