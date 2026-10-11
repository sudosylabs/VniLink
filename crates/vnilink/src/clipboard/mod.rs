//! Clipboard text for one control session.
//!
//! The text rides its own stream so a paste does not wait behind keyboard
//! input, and keyboard input does not wait behind a paste. The host applies
//! it only when that controller's clipboard grant is on. This module does not
//! read or write the operating-system clipboard, and it does not log the text.

use crate::error::Error;

/// Largest clipboard paste, in UTF-8 bytes. Empty text clears the clipboard.
pub const MAX_CLIPBOARD_TEXT: usize = 256 * 1024;

mod files;
mod frame;
mod stream;

pub(crate) use files::{decode_ready, encode_ready, PasteReadyOutcome};
pub use files::{
    ClipboardFileEntry, FileSelection, PreparedFile, PreparedPaste, StoredFileSelection,
    MAX_CLIPBOARD_FILES, MAX_CLIPBOARD_FILES_BYTES,
};
pub(crate) use frame::{check_len, decode, encode, Frame};
pub(crate) use frame::{check_text, ClipboardRequest, ClipboardSender};
pub(crate) use frame::{status_code, status_result};
pub(crate) use stream::{attach, exchange, ClipboardPayload};

/// Applies text on the controlled computer.
///
/// The platform owns the operating-system clipboard. This method must not
/// call back into [`crate::Core`]. The text is private: do not log it.
pub trait HostClipboard: Send {
    fn set_text(&mut self, text: &str) -> Result<(), Error>;
}

/// Resolves offered selection paths to absolute source files for paste
/// preparation. The platform owns the file-manager selection scope: resolve
/// only paths the user actually selected, and refuse anything else.
/// Canonicalize the result and refuse symlinks escaping the selection; the
/// core hashes whatever this returns. Shared across concurrent
/// preparations; use interior mutability for caches. Must not call back
/// into [`crate::Core`]. Resolved paths are private.
pub trait HostClipboardFiles: Send + Sync {
    fn resolve_source(&self, path: &str) -> Result<std::path::PathBuf, Error>;
}
