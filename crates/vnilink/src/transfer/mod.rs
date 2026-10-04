//! One file for one control session.
//!
//! The bytes ride their own stream so a transfer does not wait behind keyboard
//! input or clipboard text. The host writes a file only when that controller's
//! transfer grant is on. The name has to stay inside the directory chosen for
//! this installation. This module does not log the name or the bytes.

mod frame;
mod path;
mod stream;

/// Largest file, in bytes. An empty file is allowed.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

pub(crate) const MAX_NAME_BYTES: usize = 1024;
pub(crate) const MAX_COMPONENT_BYTES: usize = 255;
pub(crate) const MAX_COMPONENTS: usize = 16;
pub(crate) const MAX_CHUNK: usize = 64 * 1024;

/// Tag plus one chunk. The outer frame length is four more bytes.
pub(crate) const MAX_FRAME: usize = 1 + MAX_CHUNK;
const UNSAFE_NAME: &str = "the file name is not a safe relative path";
const TOO_BIG: &str = "the file is larger than 8 MiB";
const SAVE_FAILED: &str = "the file could not be saved";
const REJECTED: &str = "the file was rejected";
const DIRECTORY_FAILED: &str = "the transfer directory could not be created";

#[cfg(test)]
pub(crate) use frame::{check_len, decode, encode, Frame};
pub(crate) use frame::{TransferRequest, TransferSender};
pub(crate) use path::{check_name, check_submission, checked_size, prepare_directory, write_file};
#[cfg(test)]
pub(crate) use stream::finish_decision;
pub(crate) use stream::{attach, exchange};
