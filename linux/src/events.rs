//! Live UI state beside the snapshot. Owns the pending control request,
//! the connecting flag, and the toast queue — everything `paint` needs
//! beyond the snapshot itself.

use vnilink::ControlRequest;

/// Ephemeral state the snapshot does not carry.
#[derive(Debug, Default)]
pub struct LiveState {
    request: Option<ControlRequest>,
    connecting: bool,
    toasts: Vec<String>,
}

impl LiveState {
    pub fn set_request(&mut self, request: Option<ControlRequest>) {
        self.request = request;
    }

    pub fn request(&self) -> Option<&ControlRequest> {
        self.request.as_ref()
    }

    pub fn take_request(&mut self) -> Option<ControlRequest> {
        self.request.take()
    }

    pub fn set_connecting(&mut self, connecting: bool) {
        self.connecting = connecting;
    }

    pub fn connecting(&self) -> bool {
        self.connecting
    }

    pub fn toast(&mut self, message: String) {
        self.toasts.push(message);
    }

    pub fn drain_toasts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.toasts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_state_holds_request_connecting_and_toasts() {
        let mut state = LiveState::default();
        assert!(!state.connecting());
        state.set_connecting(true);
        assert!(state.connecting());
        state.toast("one".to_string());
        state.toast("two".to_string());
        assert_eq!(state.drain_toasts(), vec!["one".to_string(), "two".to_string()]);
        assert!(state.drain_toasts().is_empty());
        state.set_request(Some(ControlRequest {
            endpoint_id: "other".to_string(),
            check: "847".to_string(),
        }));
        assert_eq!(state.request().unwrap().check, "847");
        assert!(state.take_request().is_some());
        assert!(state.request().is_none());
    }
}
