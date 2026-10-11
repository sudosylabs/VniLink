//! Pairing flow state. Owns which step the UI shows, not the widgets.
//!
//! Two directions share this: this computer offering a file (digits shown
//! here, typed on the other screen) and a file from another computer opened
//! here (digits typed here). A control request is a third: digits shown on
//! the controller, typed here.

/// Where the pairing UI stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PairingFlow {
    /// Nothing in progress.
    #[default]
    Idle,
    /// This computer created a file; these digits are on our screen.
    Offer { check: String },
    /// A file from another computer is open; these digits came with it and
    /// the typed entry must match the other screen.
    FileOpened { endpoint_id: String, check: String },
    /// Connected and waiting: these digits are on our screen for the host
    /// to type.
    Waiting { check: String },
}

impl PairingFlow {
    pub fn offer(&mut self, check: String) {
        *self = Self::Offer { check };
    }

    pub fn open_file(&mut self, endpoint_id: String, check: String) {
        *self = Self::FileOpened { endpoint_id, check };
    }

    pub fn wait(&mut self, check: String) {
        *self = Self::Waiting { check };
    }

    /// Selecting a computer abandons any open file, matching the old
    /// behavior of clearing the file view on row selection.
    pub fn select_computer(&mut self) {
        if matches!(self, Self::FileOpened { .. }) {
            *self = Self::Idle;
        }
    }

    pub fn clear_waiting(&mut self) {
        if matches!(self, Self::Waiting { .. }) {
            *self = Self::Idle;
        }
    }

    pub fn clear_file(&mut self) {
        if matches!(self, Self::FileOpened { .. }) {
            *self = Self::Idle;
        }
    }

    pub fn offer_check(&self) -> Option<&str> {
        match self {
            Self::Offer { check } => Some(check),
            _ => None,
        }
    }

    pub fn file(&self) -> Option<(&str, &str)> {
        match self {
            Self::FileOpened { endpoint_id, check } => Some((endpoint_id, check)),
            _ => None,
        }
    }

    pub fn waiting(&self) -> Option<&str> {
        match self {
            Self::Waiting { check } => Some(check),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_steps_replace_each_other() {
        let mut flow = PairingFlow::default();
        assert_eq!(flow, PairingFlow::Idle);
        flow.offer("042".to_string());
        assert_eq!(flow.offer_check(), Some("042"));
        flow.open_file("other".to_string(), "847".to_string());
        assert_eq!(flow.file(), Some(("other", "847")));
        assert_eq!(flow.offer_check(), None);
        flow.select_computer();
        assert_eq!(flow, PairingFlow::Idle);
        flow.wait("133".to_string());
        assert_eq!(flow.waiting(), Some("133"));
        flow.clear_file();
        assert_eq!(flow.waiting(), Some("133"));
        flow.select_computer();
        assert_eq!(flow.waiting(), Some("133"));
        flow.clear_waiting();
        assert_eq!(flow, PairingFlow::Idle);
    }
}
