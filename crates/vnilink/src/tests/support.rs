use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
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

pub(super) struct FixedDesktop {
    pub(super) width: u16,
    pub(super) height: u16,
    pub(crate) events: Arc<Mutex<Vec<InputEvent>>>,
    pub(super) fail_key_down: Option<u16>,
    pub(super) fail_release: bool,
}

impl HostDesktop for FixedDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::Key { down: false, .. } | InputEvent::Button { down: false, .. }
                if self.fail_release =>
            {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            InputEvent::Key { usage, down: true } if self.fail_key_down == Some(usage) => {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            _ => {}
        }
        self.events
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        (self.width, self.height)
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

pub(super) fn take_matching(
    subscription: &EventSubscription,
    mut pred: impl FnMut(&CoreEvent) -> bool,
) -> CoreEvent {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for an event");
        }
        if let Some(event) = subscription.recv_timeout(Duration::from_millis(20)) {
            if pred(&event) {
                return event;
            }
        }
    }
}

pub(super) fn boot_capacity(event_capacity: usize) -> Result<Core, Error> {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            event_capacity,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
}

/// Publish one display, wait until the controller has it, and select it.
pub(super) fn arm_display(host: &Core, controller: &Core, session_id: u64, generation: u64) {
    let display = crate::DisplayInfo {
        id: 1,
        origin_x: 0,
        origin_y: 0,
        width: 1920,
        height: 1080,
        rotation_deg: 0,
        scale_percent: 100,
        revision: 1,
    };
    host.set_topology(1, &[display]).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let ready = controller
            .session_topology(session_id, generation)
            .ok()
            .is_some_and(|displays| displays.iter().any(|item| item.id == 1));
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the display topology did not arrive"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    controller
        .select_displays(session_id, generation, &[1])
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let selected = host.video_targets().ok().is_some_and(|targets| {
            targets.iter().any(|target| {
                !target.service
                    && host
                        .incoming_display_selection(target.id, target.generation)
                        .ok()
                        .as_deref()
                        == Some(&[1][..])
            })
        });
        if selected {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the display was not selected"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn wait_applied(events: &Mutex<Vec<InputEvent>>, expected: &[InputEvent]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
        if got.len() >= expected.len() {
            assert_eq!(got.as_slice(), expected);
            return;
        }
        drop(got);
        if std::time::Instant::now() > deadline {
            let got = events.lock().unwrap_or_else(|poison| poison.into_inner());
            panic!("applied events did not settle: {got:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
