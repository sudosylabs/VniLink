use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::pairing::{self, PAIRING_FILE_EXTENSION};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

#[test]
fn file_digits_change_when_the_invitation_changes() {
    let endpoint = [7u8; 32];
    let nonce = [9u8; 16];
    let check = pairing::file_digits(&endpoint, &nonce, 50);
    assert_eq!(check, pairing::file_digits(&endpoint, &nonce, 50));
    assert_eq!(check.len(), 3);
    assert!(check.bytes().all(|byte| byte.is_ascii_digit()));
    assert_ne!(check, pairing::file_digits(&endpoint, &nonce, 51));
    assert_ne!(check, pairing::control_digits(&endpoint));
    assert_eq!(PAIRING_FILE_EXTENSION, "vnl");
}

#[test]
fn an_expired_file_is_rejected() {
    let host = start_direct();
    let controller = start_direct();
    let past = SystemTime::now() - Duration::from_secs(20 * 60);
    let file = host
        .export_pairing_file_at(Duration::from_secs(60), true, past)
        .unwrap();
    assert!(matches!(
        controller.open_pairing_file(&file.bytes),
        Err(Error::InvitationExpired)
    ));
}
