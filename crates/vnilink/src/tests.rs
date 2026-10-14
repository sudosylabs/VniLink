#[path = "tests/admission.rs"]
mod admission_tests;
#[path = "tests/apply.rs"]
mod apply_tests;
#[path = "tests/availability.rs"]
mod availability_tests;
#[path = "tests/blob.rs"]
mod blob_tests;
#[path = "tests/clipboard_files.rs"]
mod clipboard_files_tests;
#[path = "tests/clipboard.rs"]
mod clipboard_tests;
#[path = "tests/datagram.rs"]
mod datagram_tests;
#[path = "tests/desktop_clipboard.rs"]
mod desktop_clipboard_tests;
#[path = "tests/diagnostics.rs"]
mod diagnostics_tests;
#[path = "tests/error.rs"]
mod error_tests;
#[path = "tests/events.rs"]
mod events_tests;
#[path = "tests/filesystem_location.rs"]
pub(crate) mod filesystem_location;
#[path = "tests/fs.rs"]
mod fs_tests;
#[path = "tests/hosting.rs"]
mod hosting_tests;
#[path = "tests/hostname.rs"]
mod hostname_tests;
#[path = "tests/identity.rs"]
mod identity_tests;
#[path = "tests/input.rs"]
mod input_tests;
#[path = "tests/installation.rs"]
mod installation_tests;
#[path = "tests/invitation.rs"]
mod invitation_tests;
#[path = "tests/job.rs"]
mod job_tests;
#[path = "tests/keyboard_mode.rs"]
mod keyboard_mode_tests;
#[path = "tests/keyboard_support.rs"]
mod keyboard_support;
#[path = "tests/keyboard.rs"]
mod keyboard_tests;
#[path = "tests/keyboard_text.rs"]
mod keyboard_text_tests;
#[path = "tests/link.rs"]
mod link_tests;
#[path = "tests/multi_session.rs"]
mod multi_session_tests;
#[path = "tests/os_input.rs"]
mod os_input_tests;
#[path = "tests/pairing.rs"]
mod pairing_tests;
#[path = "tests/persistence.rs"]
mod persistence_tests;
#[path = "tests/protocol.rs"]
mod protocol_tests;
#[path = "tests/reconnect.rs"]
mod reconnect_tests;
#[path = "tests/relay.rs"]
mod relay_tests;
#[path = "tests/session.rs"]
mod session_tests;
#[path = "tests/sidestream.rs"]
mod sidestream_tests;
#[path = "tests/soak.rs"]
mod soak_tests;
#[path = "tests/support.rs"]
pub(crate) mod support;
#[path = "tests/system.rs"]
mod system_tests;
#[path = "tests/topology.rs"]
mod topology_tests;
#[path = "tests/trust.rs"]
mod trust_tests;
#[path = "tests/video_caps.rs"]
mod video_caps_tests;
#[path = "tests/video.rs"]
mod video_tests;

#[path = "tests/services.rs"]
mod service_tests;

#[path = "tests/desktop_services.rs"]
mod desktop_service_tests;

#[cfg(unix)]
#[path = "tests/terminal.rs"]
mod terminal_tests;

#[path = "tests/audio.rs"]
mod audio_tests;
#[path = "tests/camera.rs"]
mod camera_tests;

#[path = "tests/power.rs"]
mod power_tests;
