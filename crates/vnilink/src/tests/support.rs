use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::input::{HostDesktop, InputEvent};
use crate::session::{Grants, SessionPhase, SessionRole};
use crate::video::HostCapture;

pub(crate) fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    // Test cores are capable machines; unsupported-path tests reset these.
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

pub(super) fn wait_closed(core: &Core, role: SessionRole) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = core.snapshot().unwrap();
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.phase == SessionPhase::Closed)
        {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("session did not close: {session:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) struct RecordingDesktop {
    pub(crate) events: Arc<Mutex<Vec<InputEvent>>>,
}

impl HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

pub(super) struct ShutdownCapture;

impl HostCapture for ShutdownCapture {
    fn monitor(&self) -> (u16, u16) {
        (1, 1)
    }
}

pub(crate) fn pair_for_control(host: &Core, controller: &Core) {
    // Negotiation needs an encoder on the host and a decoder on the
    // controller. Tests that cover the unsupported paths set their own flags.
    host.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller
        .set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
}
