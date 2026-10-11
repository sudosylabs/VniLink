//! User-facing sentences for core errors. Names the state, never secrets.

/// One-line sentence for a toast after a failed action.
pub fn say(error: &vnilink::Error) -> String {
    match error {
        vnilink::Error::PairingCheckMismatch => "Those digits do not match.".to_string(),
        vnilink::Error::InvalidPairingFile => "That file is not a pairing file.".to_string(),
        vnilink::Error::InvitationExpired => "That pairing file has expired.".to_string(),
        vnilink::Error::InvitationReplayed => "That pairing file was already used.".to_string(),
        vnilink::Error::NotPaired => "Save that computer before connecting.".to_string(),
        vnilink::Error::Timeout => "That computer did not answer.".to_string(),
        vnilink::Error::NotHosting => "Allow control is off.".to_string(),
        vnilink::Error::Unauthorized => "That computer did not allow control.".to_string(),
        vnilink::Error::Busy => "A connection is already in progress.".to_string(),
        vnilink::Error::Network(_) => "Could not reach that computer.".to_string(),
        vnilink::Error::Unsupported(reason) => {
            format!("That computer cannot share its desktop ({reason}).")
        }
        other => other.to_string(),
    }
}

/// Title and body for the startup failure page.
pub struct Notice {
    pub title: String,
    pub message: String,
}

pub fn open_notice(error: &vnilink::Error) -> Notice {
    let (title, message) = match error {
        vnilink::Error::StoreLocked => (
            "The keyring is locked",
            "Unlock your login keyring, then try again.",
        ),
        vnilink::Error::StoreUnavailable => (
            "The keyring is not available",
            "VniLink could not open the system keyring. Check that a Secret Service such as GNOME Keyring is running, then try again. Nothing was written to a file.",
        ),
        vnilink::Error::CorruptRecord => (
            "The keyring record is not valid",
            "The installation in the keyring is not valid. VniLink left it untouched.",
        ),
        other => {
            return Notice {
                title: "VniLink did not open".to_string(),
                message: other.to_string(),
            };
        }
    };
    Notice {
        title: title.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_name_the_state() {
        assert_eq!(
            say(&vnilink::Error::NotPaired),
            "Save that computer before connecting."
        );
        assert!(say(&vnilink::Error::Unsupported(
            "no H.264 encoder is available"
        ))
        .contains("cannot share its desktop"));
        let notice = open_notice(&vnilink::Error::StoreLocked);
        assert_eq!(notice.title, "The keyring is locked");
        let notice = open_notice(&vnilink::Error::Timeout);
        assert_eq!(notice.title, "VniLink did not open");
    }
}
