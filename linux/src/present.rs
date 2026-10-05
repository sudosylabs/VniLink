//! Words the window shows. They name state, not screen contents or secrets.

use vnilink::{Grants, SessionInfo, SessionPhase, SessionRole};

pub fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= 16 {
        return id.to_string();
    }
    let head: String = chars.iter().take(8).collect();
    let tail: String = chars.iter().skip(chars.len() - 6).collect();
    format!("{head}…{tail}")
}

pub fn relay_summary(public_relay: bool, urls: &[String]) -> String {
    if public_relay {
        return "Public relays".to_string();
    }
    if urls.is_empty() {
        return "Custom relay".to_string();
    }
    urls.join(", ")
}

pub fn relay_description(public_relay: bool) -> &'static str {
    if public_relay {
        "Paired computers use these when a direct path is not open."
    } else {
        "Paired computers use only these. The public relays are not used."
    }
}

pub fn hosting_name(hosting: bool) -> &'static str {
    if hosting {
        "Allowing control"
    } else {
        "Not allowing control"
    }
}

pub fn hosting_subtitle(hosting: bool, file_ready: bool) -> &'static str {
    match (hosting, file_ready) {
        (false, _) => "Off. Creating a pairing file turns this on for 15 minutes.",
        (true, false) => "On. A new connection still needs a pairing file.",
        (true, true) => {
            "On. One computer can connect until the file expires, or until this is off."
        }
    }
}

pub fn peer_status(grants: Grants) -> &'static str {
    if grants.control {
        "Control allowed"
    } else {
        "Needs permission"
    }
}

pub fn peer_sentence(grants: Grants) -> &'static str {
    if grants.control {
        "Control is allowed. Connect when that computer is on. The clipboard and files stay off."
    } else {
        "Connect. This screen then shows three digits for the person at that computer to type."
    }
}

pub fn grant_summary(grants: Grants) -> String {
    let mut parts = Vec::new();
    if grants.control {
        parts.push("control");
    }
    if grants.clipboard {
        parts.push("clipboard");
    }
    if grants.transfer {
        parts.push("files");
    }
    match parts.as_slice() {
        [] => "No allows yet".to_string(),
        [one] => capitalize(one),
        [first, second] => format!("{} and {second}", capitalize(first)),
        many => format!("{}, {}, and {}", capitalize(many[0]), many[1], many[2]),
    }
}

pub fn grant_prose(grants: Grants) -> &'static str {
    match (grants.control, grants.clipboard, grants.transfer) {
        (false, false, false) => {
            "Nothing is allowed yet. Control, the clipboard, and files stay off."
        }
        (true, true, true) => "Control, the clipboard, and files are allowed.",
        (true, false, false) => "Control is allowed. The clipboard and files are not.",
        (false, true, false) => "The clipboard is allowed. Control and files are not.",
        (false, false, true) => "Files are allowed. Control and the clipboard are not.",
        (true, true, false) => "Control and the clipboard are allowed. Files are not.",
        (true, false, true) => "Control and files are allowed. The clipboard is not.",
        (false, true, true) => "The clipboard and files are allowed. Control is not.",
    }
}

pub fn allowed_name(allowed: bool) -> &'static str {
    if allowed {
        "Allowed"
    } else {
        "Not allowed"
    }
}

/// Explicit video state for this installation. Returns a sentence when video
/// cannot work, so the window names the missing codec instead of showing a
/// black screen. `capture` is whether a capture adapter is installed.
pub fn video_status(capture: bool, h264_encode: bool, h264_decode: bool) -> Option<&'static str> {
    if capture && !h264_encode {
        return Some("Video unavailable: no H.264 encoder on this computer.");
    }
    if !h264_decode {
        return Some("This computer cannot decode video: no H.264 decoder.");
    }
    None
}

pub fn phase_name(phase: &SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Connecting => "Connecting",
        SessionPhase::Authenticated => "Authenticated",
        SessionPhase::Authorized => "Authorized",
        SessionPhase::Negotiating => "Negotiating",
        SessionPhase::Active => "Active",
        SessionPhase::Reconnecting => "Reconnecting",
        SessionPhase::Stopping => "Stopping",
        SessionPhase::Closed => "Closed",
        SessionPhase::Failed(_) => "Failed",
    }
}

pub fn role_name(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Host => "Hosting a session",
        SessionRole::Controller => "Connected to a computer",
    }
}

pub fn session_banner(session: &SessionInfo) -> String {
    let who = short_id(&session.peer_id);
    match (&session.role, &session.phase) {
        (SessionRole::Host, SessionPhase::Active) => {
            format!("{who} is connected to this desktop")
        }
        (SessionRole::Controller, SessionPhase::Active) => format!("Connected to {who}"),
        (_, phase) => format!("{who} · {}", phase_name(phase)),
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(role: SessionRole, phase: SessionPhase) -> SessionInfo {
        SessionInfo {
            id: 1,
            generation: 1,
            role,
            phase,
            peer_id: "abcdefghijklmnopqrstuvwxyz012345".to_string(),
            quality: None,
            monitor_width: 0,
            monitor_height: 0,
        }
    }

    #[test]
    fn summaries_name_the_relay_the_allows_and_the_session() {
        assert_eq!(relay_summary(true, &[]), "Public relays");
        assert_eq!(relay_summary(false, &[]), "Custom relay");
        assert_eq!(
            relay_summary(false, &["relay.example".to_string()]),
            "relay.example"
        );
        assert!(relay_description(true).contains("direct path"));
        assert!(relay_description(false).contains("public relays"));
        assert_eq!(grant_summary(Grants::default()), "No allows yet");
        assert_eq!(
            grant_summary(Grants {
                control: true,
                clipboard: false,
                transfer: true,
            }),
            "Control and files"
        );
        assert_eq!(
            grant_summary(Grants {
                control: true,
                clipboard: true,
                transfer: true,
            }),
            "Control, clipboard, and files"
        );
        assert_eq!(
            grant_prose(Grants::default()),
            "Nothing is allowed yet. Control, the clipboard, and files stay off."
        );
        assert_eq!(short_id("other-computer"), "other-computer");
        assert_eq!(
            short_id("abcdefghijklmnopqrstuvwxyz012345"),
            "abcdefgh…012345"
        );
        let banner = session_banner(&session(SessionRole::Host, SessionPhase::Active));
        assert_eq!(banner, "abcdefgh…012345 is connected to this desktop");
        let failed = session_banner(&session(
            SessionRole::Controller,
            SessionPhase::Failed("hidden reason".to_string()),
        ));
        assert!(failed.contains("Failed"));
        assert!(!failed.contains("hidden"));
        assert_eq!(phase_name(&SessionPhase::Failed("hidden".into())), "Failed");
        assert_eq!(hosting_name(true), "Allowing control");
        assert_eq!(peer_status(Grants::default()), "Needs permission");
        assert_eq!(
            peer_status(Grants {
                control: true,
                clipboard: false,
                transfer: false,
            }),
            "Control allowed"
        );
        assert!(peer_sentence(Grants::default()).contains("three digits"));
        assert_eq!(
            hosting_subtitle(false, false),
            "Off. Creating a pairing file turns this on for 15 minutes."
        );
        assert!(hosting_subtitle(true, false).contains("pairing file"));
        assert!(hosting_subtitle(true, true).contains("expires"));
        let _ = role_name(SessionRole::Host);
    }

    #[test]
    fn video_status_names_the_missing_codec() {
        assert_eq!(video_status(false, false, true), None);
        assert_eq!(video_status(true, true, true), None);
        assert!(video_status(true, false, true)
            .unwrap()
            .contains("no H.264 encoder"));
        assert!(video_status(false, false, false)
            .unwrap()
            .contains("no H.264 decoder"));
    }
}
