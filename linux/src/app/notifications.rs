//! Toasts. The only way the app reports a failed action.

use crate::error::say;

/// Shows a failed action as a toast.
pub(crate) fn failed(toasts: &adw::ToastOverlay, error: &vnilink::Error) {
    toasts.add_toast(adw::Toast::new(&say(error)));
}

/// Shows a plain message as a toast.
pub(crate) fn note(toasts: &adw::ToastOverlay, message: &str) {
    toasts.add_toast(adw::Toast::new(message));
}
