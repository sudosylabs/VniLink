use crate::error::Error;

mod events;
mod map;
mod queue;
pub mod usage;

pub(crate) use events::{check_key_text, key_text_problem, swap_command_key};
pub use events::{InputEvent, PointerButton};
pub use map::map_point;
pub(crate) use queue::{HeldInput, InputFault, InputOutbox, Outbound};

/// Finished text on the control stream, in bytes. This is a short phrase,
/// not a clipboard paste. The control frame also carries the tag and the
/// session ids, and that frame stays within 256 bytes.
pub const MAX_KEY_TEXT: usize = 128;

/// How many input events may wait to be written. Pointer movement collapses
/// instead of filling this queue. Key and button transitions do not.
pub(crate) const INPUT_QUEUE: usize = 64;

/// Applies authorized input on the host. The native app can supply its own
/// implementation. [`crate::Core::enable_os_input`] installs one that injects
/// into the operating system.
pub trait HostDesktop: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error>;

    /// Types finished characters. This is not a key position and not a
    /// clipboard paste. The default refuses that string and leaves key
    /// injection available. Returning [`Error::Unsupported`] makes the host
    /// stop applying later keys for the session.
    fn type_text(&mut self, text: &str) -> Result<(), Error> {
        let _ = text;
        Err(Error::InputFailed("text entry is not available"))
    }

    /// Presses the host layout's chord for these characters. A character the
    /// layout does not contain is refused. This is not a clipboard paste.
    /// The default refuses the string and leaves later keys available.
    fn type_chord(&mut self, text: &str) -> Result<(), Error> {
        let _ = text;
        Err(Error::InputFailed("translated keys are not available"))
    }

    /// Pixel size of the one monitor this session controls. `(0, 0)` if unknown.
    fn monitor(&self) -> (u16, u16);
}
