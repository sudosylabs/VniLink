//! Native pasteboard writes are acknowledged through the platform callback.
use crate::{error::map_error, FfiError, VnilinkCore};

#[uniffi::export(callback_interface)]
pub trait ClipboardWriter: Send + Sync {
    /// Called on a core worker, never the UI thread. Do not call back into Core.
    fn set_text(&self, text: String) -> Result<(), FfiError>;
}

struct NativeClipboard(Box<dyn ClipboardWriter>);
impl vnilink::HostClipboard for NativeClipboard {
    fn set_text(&mut self, text: &str) -> Result<(), vnilink::Error> {
        self.0.set_text(text.to_owned()).map_err(|_| {
            vnilink::Error::Unsupported("the operating system could not update the clipboard")
        })
    }
}

#[uniffi::export]
impl VnilinkCore {
    pub fn install_clipboard(&self, writer: Box<dyn ClipboardWriter>) -> Result<(), FfiError> {
        self.inner
            .set_clipboard(Box::new(NativeClipboard(writer)))
            .map_err(map_error)
    }
    pub fn publish_clipboard(&self, session_id: u64, text: String) -> Result<(), FfiError> {
        self.inner
            .publish_clipboard(session_id, &text)
            .map_err(map_error)
    }
}
