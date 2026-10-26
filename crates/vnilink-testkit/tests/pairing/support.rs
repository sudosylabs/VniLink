//! What several pairing scenarios share: an installation whose store the
//! test holds, pairing raw engines as the application pairs them, and
//! reading what one engine knows of another.

use std::sync::Arc;
use std::time::Duration;

use vnilink::{
    ControlRequest, Core, CoreEvent, DeviceInfo, EventSubscription, Grants, MemoryStore,
    PairingFile, RelaySettings, SessionPhase, SessionRole,
};
use vnilink_testkit::wait_for;

/// What a host shares once paired for control: it may be seen and
/// controlled.
pub const SEEN_AND_CONTROLLED: Grants = Grants {
    view: true,
    control: true,
    clipboard: false,
    transfer: false,
};

/// Everything a host can share.
pub const EVERYTHING: Grants = Grants {
    view: true,
    control: true,
    clipboard: true,
    transfer: true,
};

/// An engine whose installation store the test holds, to make its next save
/// fail or to reopen it with another relay. A kit [`vnilink_testkit::Engine`]
/// keeps its store to itself; this is that engine with the store in hand,
/// set up as the kit sets up its own.
pub struct Installed {
    pub core: Core,
    pub store: MemoryStore,
    name: String,
}

impl Installed {
    /// A new installation named `name`, on the public relays it never
    /// reaches.
    pub fn start(name: &str) -> Self {
        Self::start_with(name, RelaySettings::public_n0())
    }

    /// A new installation named `name` whose first relay setting is `relay`.
    pub fn start_with(name: &str, relay: RelaySettings) -> Self {
        let store = MemoryStore::empty();
        let core = open(name, &store, relay);
        Self {
            core,
            store,
            name: name.to_owned(),
        }
    }

    /// Shuts this engine down and opens its installation again, as quitting
    /// and starting the application does.
    pub fn restart(&mut self) {
        self.restart_with(RelaySettings::public_n0());
    }

    /// As [`Self::restart`], offering `relay` as the first relay setting,
    /// which a saved one overrides.
    pub fn restart_with(&mut self, relay: RelaySettings) {
        self.core.shutdown();
        self.core = open(&self.name, &self.store, relay);
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

/// Opens the installation in `store` as the kit opens an engine: on
/// loopback, named `name` in records and to its peers, with codecs.
fn open(name: &str, store: &MemoryStore, relay: RelaySettings) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), relay).expect("a loopback engine");
    core.set_trace_name(name);
    core.set_hostname(name).expect("a computer name");
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
        .expect("codecs");
    core
}

/// `host` and `controller` trust each other as two paired computers do,
/// the host serving and sharing itself to be seen and controlled.
pub fn pair_for_control(host: &Core, controller: &Core) {
    let invitation = Duration::from_secs(60);
    host.start_hosting().expect("hosting");
    controller
        .accept_invitation(&host.create_invitation(invitation).expect("an invitation"))
        .expect("the host's invitation");
    host.accept_invitation(
        &controller
            .create_invitation(invitation)
            .expect("an invitation"),
    )
    .expect("the controller's invitation");
    host.set_desktop_sharing(SEEN_AND_CONTROLLED)
        .expect("sharing");
}

/// The controller opens `host`'s pairing file and confirms its digits: the
/// controller remembers the host, and the host may approve the controller
/// once when `allow_control`.
pub fn exchange(host: &Core, controller: &Core, allow_control: bool) -> PairingFile {
    let file = host
        .export_pairing_file(Duration::from_secs(60), allow_control)
        .expect("a pairing file");
    let preview = controller
        .open_pairing_file(&file.bytes)
        .expect("the file opens");
    controller
        .confirm_pairing(&preview, &file.check)
        .expect("the digits match");
    file
}

/// What `core` remembers of the peer `endpoint_id`.
pub fn device(core: &Core, endpoint_id: &str) -> DeviceInfo {
    core.snapshot()
        .expect("a snapshot")
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
        .expect("a paired device")
}

/// The request the host asks its owner to approve, once it asks, within
/// `within`.
pub fn wait_request(host: &Core, within: Duration) -> ControlRequest {
    wait_for("the host to ask its owner", within, || {
        host.control_request().ok().flatten()
    })
}

/// Waits until `core`'s session in `role` is closed.
pub fn wait_closed(core: &Core, role: SessionRole, within: Duration) {
    wait_for(&format!("the {role:?} session to close"), within, || {
        let snapshot = core.snapshot().ok()?;
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        (session?.phase == SessionPhase::Closed).then_some(())
    });
}

/// The events `events` holds now, taken.
pub fn drain(events: &EventSubscription) -> Vec<CoreEvent> {
    std::iter::from_fn(|| events.try_recv()).collect()
}
