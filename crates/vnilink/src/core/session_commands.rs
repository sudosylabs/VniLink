//! What a desktop session's controller asks its host besides pictures: the
//! quality it wants and the display it shows, sent in order on the
//! session's control stream. A session carries no input; a Desktop
//! service's own stream does (`desktop_stream`).
use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::Notify;

use crate::error::Error;
use crate::protocol::Quality;

/// The most commands a session holds unsent.
const BOUND: usize = crate::input::INPUT_QUEUE;

/// One command a controller asks of its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SessionCommand {
    Quality(Quality),
    Displays(Vec<u64>),
}

/// Why a session takes no more commands: its desktop ended, or its host
/// refused one. Kept apart from the error, which is not `Clone`.
#[derive(Debug, Clone, Copy)]
enum CommandFault {
    Unsupported(&'static str),
    Stale,
    NoSession,
    /// Any other refusal: the session is ending.
    Refused,
}

impl CommandFault {
    fn of(error: &Error) -> Self {
        match error {
            Error::Unsupported(reason) => Self::Unsupported(reason),
            Error::StaleSession => Self::Stale,
            Error::NoSession => Self::NoSession,
            _ => Self::Refused,
        }
    }

    fn error(self) -> Error {
        match self {
            Self::Unsupported(reason) => Error::Unsupported(reason),
            Self::Stale => Error::StaleSession,
            Self::NoSession => Error::NoSession,
            Self::Refused => Error::Unauthorized,
        }
    }
}

/// A session's unsent commands. Once the host refuses one, or the desktop
/// ends, it takes no more until cleared for a new generation.
pub(super) struct SessionCommands {
    queue: Mutex<VecDeque<SessionCommand>>,
    fault: Mutex<Option<CommandFault>>,
    notify: Notify,
}

impl SessionCommands {
    pub(super) fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            fault: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    /// Queues `command`; a full queue is [`Error::Busy`], and a refused
    /// session gives the refusal again.
    pub(super) fn push(&self, command: SessionCommand) -> Result<(), Error> {
        if let Some(fault) = *super::lock(&self.fault) {
            return Err(fault.error());
        }
        let mut queue = super::lock(&self.queue);
        if queue.len() >= BOUND {
            return Err(Error::Busy);
        }
        queue.push_back(command);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }

    pub(super) fn drain(&self) -> Vec<SessionCommand> {
        super::lock(&self.queue).drain(..).collect()
    }

    /// Drops what is queued and any refusal, for a new generation.
    pub(super) fn clear(&self) {
        super::lock(&self.queue).clear();
        *super::lock(&self.fault) = None;
    }

    /// Refuses later commands with `error`; the first refusal stands.
    pub(super) fn fail(&self, error: &Error) {
        let mut fault = super::lock(&self.fault);
        if fault.is_none() {
            *fault = Some(CommandFault::of(error));
        }
    }

    pub(super) fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_go_in_order_until_the_bound_and_a_refusal_stops_them() {
        let commands = SessionCommands::new();
        commands
            .push(SessionCommand::Displays(vec![1]))
            .expect("a display selection queues");
        commands
            .push(SessionCommand::Quality(crate::session::controller_offer()))
            .expect("a quality queues");
        assert_eq!(
            commands.drain(),
            vec![
                SessionCommand::Displays(vec![1]),
                SessionCommand::Quality(crate::session::controller_offer())
            ],
            "commands are sent in the order asked"
        );
        for _ in 0..BOUND {
            commands
                .push(SessionCommand::Displays(vec![2]))
                .expect("within the bound");
        }
        assert!(
            matches!(
                commands.push(SessionCommand::Displays(vec![3])),
                Err(Error::Busy)
            ),
            "a full queue is busy"
        );
        commands.fail(&Error::Unsupported("the Desktop service has ended"));
        commands.clear();
        commands.fail(&Error::Unsupported("the Desktop service has ended"));
        assert!(
            matches!(
                commands.push(SessionCommand::Displays(vec![1])),
                Err(Error::Unsupported("the Desktop service has ended"))
            ),
            "a refused session takes no more commands"
        );
        commands.clear();
        assert!(
            commands.drain().is_empty(),
            "clearing dropped what was queued"
        );
        commands
            .push(SessionCommand::Displays(vec![1]))
            .expect("a new generation takes commands again");
    }
}
