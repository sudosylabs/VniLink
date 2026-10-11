//! Native pasteboard writes are acknowledged through the platform callback.
use crate::{error::map_error, FfiError, VnilinkCore};
use std::sync::Arc;

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

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiClipboardFileEntry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct FfiFileSelection {
    pub entries: Vec<FfiClipboardFileEntry>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiStoredFileSelection {
    pub revision: u64,
    pub selection: FfiFileSelection,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiPreparedFile {
    pub path: String,
    pub hash: Vec<u8>,
    pub size: u64,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct FfiPreparedPaste {
    pub files: Vec<FfiPreparedFile>,
}

fn to_selection(selection: FfiFileSelection) -> vnilink::FileSelection {
    vnilink::FileSelection {
        entries: selection
            .entries
            .into_iter()
            .map(|entry| vnilink::ClipboardFileEntry {
                path: entry.path,
                size: entry.size,
                dir: entry.dir,
            })
            .collect(),
    }
}

fn from_selection(selection: vnilink::FileSelection) -> FfiFileSelection {
    FfiFileSelection {
        entries: selection
            .entries
            .into_iter()
            .map(|entry| FfiClipboardFileEntry {
                path: entry.path,
                size: entry.size,
                dir: entry.dir,
            })
            .collect(),
    }
}

fn from_stored(stored: vnilink::StoredFileSelection) -> FfiStoredFileSelection {
    FfiStoredFileSelection {
        revision: stored.revision,
        selection: from_selection(stored.selection),
    }
}

fn from_prepared(prepared: vnilink::PreparedPaste) -> FfiPreparedPaste {
    FfiPreparedPaste {
        files: prepared
            .files
            .into_iter()
            .map(|file| FfiPreparedFile {
                path: file.path,
                hash: file.hash.to_vec(),
                size: file.size,
            })
            .collect(),
    }
}

/// Resolves offered selection paths to absolute source files, called on
/// preparation workers, never the UI thread. Resolve only paths the user
/// actually selected; canonicalize and refuse escapes. Do not call back
/// into Core. See `vnilink::HostClipboardFiles` for the contract.
#[uniffi::export(callback_interface)]
pub trait ClipboardFileSource: Send + Sync {
    fn resolve_source(&self, path: String) -> Result<String, FfiError>;
}

struct NativeFileSource(Box<dyn ClipboardFileSource>);
impl vnilink::HostClipboardFiles for NativeFileSource {
    fn resolve_source(&self, path: &str) -> Result<std::path::PathBuf, vnilink::Error> {
        self.0
            .resolve_source(path.to_owned())
            .map(std::path::PathBuf::from)
            .map_err(|_| vnilink::Error::Unsupported("paste source is unavailable"))
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
    pub fn install_clipboard_file_source(
        &self,
        source: Option<Box<dyn ClipboardFileSource>>,
    ) -> Result<(), FfiError> {
        self.inner
            .set_clipboard_file_source(source.map(|source| Arc::new(NativeFileSource(source)) as _))
            .map_err(map_error)
    }
    pub fn submit_clipboard_files(
        &self,
        session_id: u64,
        generation: u64,
        selection: FfiFileSelection,
    ) -> Result<(), FfiError> {
        self.inner
            .submit_clipboard_files(session_id, generation, &to_selection(selection))
            .map_err(map_error)
    }
    pub fn publish_clipboard_files(
        &self,
        session_id: u64,
        selection: FfiFileSelection,
    ) -> Result<(), FfiError> {
        self.inner
            .publish_clipboard_files(session_id, &to_selection(selection))
            .map_err(map_error)
    }
    pub fn clipboard_files(&self) -> Result<Option<FfiStoredFileSelection>, FfiError> {
        self.inner
            .clipboard_files()
            .map(|stored| stored.map(from_stored))
            .map_err(map_error)
    }
    pub fn session_clipboard_files(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<Option<FfiStoredFileSelection>, FfiError> {
        self.inner
            .session_clipboard_files(session_id, generation)
            .map(|stored| stored.map(from_stored))
            .map_err(map_error)
    }
    pub fn incoming_clipboard_files(
        &self,
        session_id: u64,
    ) -> Result<Option<FfiStoredFileSelection>, FfiError> {
        self.inner
            .incoming_clipboard_files(session_id)
            .map(|stored| stored.map(from_stored))
            .map_err(map_error)
    }
    /// Prepares a paste on a hosted session. Blocks until the source answers,
    /// the session ends, or the preparation budget runs out: call on a
    /// background thread.
    pub fn prepare_incoming_paste(
        &self,
        session_id: u64,
        revision: u64,
    ) -> Result<FfiPreparedPaste, FfiError> {
        self.inner
            .prepare_incoming_paste(session_id, revision)
            .map(from_prepared)
            .map_err(map_error)
    }
    /// Prepares a paste on a controller session. Same blocking contract as
    /// `prepare_incoming_paste`.
    pub fn prepare_outgoing_paste(
        &self,
        session_id: u64,
        generation: u64,
        revision: u64,
    ) -> Result<FfiPreparedPaste, FfiError> {
        self.inner
            .prepare_outgoing_paste(session_id, generation, revision)
            .map(from_prepared)
            .map_err(map_error)
    }
}
