use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{Core, Grants, MemoryStore, RelaySettings, ServiceInfo, ServiceKind, SessionInfo};

use crate::{wait_for, Scratch, DEFAULT_WAIT};

/// A real engine on loopback, as the application runs one, with its own
/// identity in memory and its own folders: `root`, the one folder it may
/// read and write for files, and `transfers`, where incoming bytes stage.
/// It never reaches a relay, so nothing leaves this computer.
pub struct Engine {
    pub core: Arc<Core>,
    scratch: Scratch,
}

impl Engine {
    /// `name` tags its folders, such as `host` or `controller`.
    pub fn start(name: &str) -> Self {
        let core = Core::open_direct(Arc::new(MemoryStore::empty()), RelaySettings::public_n0())
            .expect("a loopback engine");
        Self::prepare(name, core)
    }

    /// An engine with no direct address, whose only way to its peers is the
    /// relay at `relay` ([`crate::relay::LocalRelay`]).
    pub fn relayed(name: &str, relay: &str) -> Self {
        let settings =
            RelaySettings::custom(vec![vnilink::CustomRelay::new(relay).expect("a relay URL")])
                .expect("relay settings");
        let core = Core::open(
            Arc::new(MemoryStore::empty()),
            vnilink::OpenConfig {
                listen: vnilink::Listen::RelayOnly,
                event_capacity: 64,
                initial_relay: settings,
            },
        )
        .expect("a relayed engine");
        Self::prepare(name, core)
    }

    fn prepare(name: &str, core: Core) -> Self {
        // Every event this engine records says it is `name`, and other
        // engines know its computer by that name, wherever the suite runs.
        core.set_trace_name(name);
        core.set_hostname(name).expect("a computer name");
        let scratch = Scratch::new(name);
        // A session needs codecs to open, whatever it carries.
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
            .expect("codecs");
        std::fs::create_dir_all(scratch.join("root")).expect("a file root");
        core.set_transfer_directory(scratch.join("transfers"))
            .expect("a transfer folder");
        // Windows refuses rooted Files until its operations are handle-relative;
        // the other services still run there, and the Files suites fail plainly.
        match core.set_file_roots(vec![scratch.join("root")]) {
            Err(vnilink::Error::Unsupported(_)) if cfg!(windows) => {}
            result => result.expect("a file root"),
        }
        // A suite's files are small next to the free space the product keeps
        // in reserve; admission has its own tests in `vnilink`.
        core.set_file_resource_policy(vnilink::job::FileResourcePolicy {
            minimum_free_bytes: 0,
            ..Default::default()
        })
        .expect("a resource policy");
        Self {
            core: Arc::new(core),
            scratch,
        }
    }

    /// The folder its files live in.
    pub fn root(&self) -> PathBuf {
        self.scratch.join("root")
    }

    /// The id of [`Self::root`] among its file roots.
    pub fn root_id(&self) -> u64 {
        let root = self.root();
        self.core
            .file_roots()
            .expect("file roots")
            .into_iter()
            .find_map(|(id, path)| (path == root).then_some(id))
            .expect("the engine's root is one of its file roots")
    }

    pub fn endpoint(&self) -> String {
        self.core.endpoint_id()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

/// What the host and the controller grant so files move both ways; a
/// session opens under view and control.
pub const FILES: (Grants, Grants) = (
    Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    },
    Grants {
        view: false,
        control: false,
        clipboard: false,
        transfer: true,
    },
);

/// A host and a controller that trust each other, as two computers do once
/// paired, the host serving.
pub struct Pair {
    pub host: Engine,
    pub controller: Engine,
}

impl Pair {
    /// Paired both ways; the host shares what `host` grants and the
    /// controller what `controller` grants.
    pub fn new(host: Grants, controller: Grants) -> Self {
        Self::paired(
            Engine::start("host"),
            Engine::start("controller"),
            host,
            controller,
        )
    }

    fn paired(
        host_engine: Engine,
        controller_engine: Engine,
        host: Grants,
        controller: Grants,
    ) -> Self {
        let pair = Self {
            host: host_engine,
            controller: controller_engine,
        };
        let invitation = Duration::from_secs(60);
        pair.host.core.start_hosting().expect("hosting");
        let token = pair
            .host
            .core
            .create_invitation(invitation)
            .expect("an invitation");
        pair.controller
            .core
            .accept_invitation(&token)
            .expect("pairing");
        let back = pair
            .controller
            .core
            .create_invitation(invitation)
            .expect("an invitation");
        pair.host.core.accept_invitation(&back).expect("pairing");
        pair.host.core.set_desktop_sharing(host).expect("sharing");
        pair.controller
            .core
            .set_desktop_sharing(controller)
            .expect("sharing");
        pair
    }

    /// As [`Self::new`], each engine reaching the other only through the
    /// relay at `relay`.
    pub fn relayed(relay: &str, host: Grants, controller: Grants) -> Self {
        Self::paired(
            Engine::relayed("host", relay),
            Engine::relayed("controller", relay),
            host,
            controller,
        )
    }

    /// Both sides may move files, as the Files tab needs.
    pub fn for_files() -> Self {
        let (host, controller) = FILES;
        Self::new(host, controller)
    }

    /// The host shares `kinds` as services, besides being seen and
    /// controlled, under which a services connection opens.
    pub fn for_services(kinds: &[ServiceKind]) -> Self {
        let pair = Self::for_desktop();
        for kind in kinds {
            pair.host
                .core
                .set_service_sharing(*kind, true)
                .expect("service sharing");
        }
        pair
    }

    /// The controller's services connection to the host, as the
    /// application opens one, once the host admitted it.
    pub fn connect_services(&self) -> SessionInfo {
        let controller = &self.controller.core;
        let pending = controller
            .begin_connect_services(&self.host.endpoint())
            .expect("a connection");
        wait_for("the connection to be admitted", DEFAULT_WAIT, || {
            let session = controller.controller_session(pending.id).ok()?;
            (session.phase == vnilink::SessionPhase::Active).then_some(session)
        })
    }

    /// A service of `kind` on `connection`.
    pub fn attach(&self, connection: &SessionInfo, kind: ServiceKind) -> ServiceInfo {
        self.controller
            .core
            .attach_controller_service(connection.id, connection.generation, kind)
            .expect("a service")
    }

    /// The host may be seen and controlled.
    pub fn for_desktop() -> Self {
        let none = Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: false,
        };
        Self::new(
            Grants {
                view: true,
                control: true,
                ..none
            },
            none,
        )
    }
}
