use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;

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
