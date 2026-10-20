//! Native pasteboard writes are acknowledged through the platform callback.
use crate::{error::map_error, FfiError, VnilinkCore};

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

#[derive(Clone, Debug)]
pub struct FfiClipboardFileEntry {
    pub path: String,
    pub size: u64,
    pub dir: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FfiFileSelection {
    pub entries: Vec<FfiClipboardFileEntry>,
}

#[derive(Clone, Debug)]
pub struct FfiStoredFileSelection {
    pub revision: u64,
    pub selection: FfiFileSelection,
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
}
