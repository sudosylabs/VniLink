use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{
    Core, Error, Grants, MemoryStore, RelaySettings, RemoteDesktopService, RemoteFilesService,
    ServiceInfo, ServiceKind, SessionInfo, SessionPhase,
};

use crate::{wait_for, Scratch, DEFAULT_WAIT};

/// A real engine on loopback, as the application runs one, with its own
/// identity in memory and its own folders: `root`, the one folder it may
/// read and write for files, and `transfers`, where incoming bytes stage.
/// It never reaches a relay, so nothing leaves this computer.
pub struct Engine {
    pub core: Arc<Core>,
    scratch: Scratch,
    name: String,
    /// Its installation: identity and records, kept for [`Self::restart`].
    store: MemoryStore,
}

impl Engine {
    /// `name` tags its folders, such as `host` or `controller`.
    pub fn start(name: &str) -> Self {
        let store = MemoryStore::empty();
        // Named from its first record, as binding its endpoint.
        vnilink::trace::name_next_engine(name);
        let core = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
            .expect("a loopback engine");
        Self::prepare(name, core, store)
    }

    /// Shuts this engine down and opens its installation again, as quitting
    /// and starting the application does: the same identity, records and
    /// folders.
    pub fn restart(&mut self) {
        self.core.shutdown();
        vnilink::trace::name_next_engine(&self.name);
        let core = Core::open_direct(Arc::new(self.store.clone()), RelaySettings::public_n0())
            .expect("a loopback engine");
        configure(&self.name, &core, &self.scratch);
        self.core = Arc::new(core);
    }

    /// An engine with no direct address, whose only way to its peers is the
    /// relay at `relay` ([`crate::relay::LocalRelay`]).
    pub fn relayed(name: &str, relay: &str) -> Self {
        let settings =
            RelaySettings::custom(vec![vnilink::CustomRelay::new(relay).expect("a relay URL")])
                .expect("relay settings");
        let store = MemoryStore::empty();
        // Named from its first record, as binding its endpoint.
        vnilink::trace::name_next_engine(name);
        let core = Core::open(
            Arc::new(store.clone()),
            vnilink::OpenConfig {
                listen: vnilink::Listen::RelayOnly,
                event_capacity: 64,
                initial_relay: settings,
            },
        )
        .expect("a relayed engine");
        Self::prepare(name, core, store)
    }

    fn prepare(name: &str, core: Core, store: MemoryStore) -> Self {
        let scratch = Scratch::new(name);
        std::fs::create_dir_all(scratch.join("root")).expect("a file root");
        configure(name, &core, &scratch);
        Self {
            core: Arc::new(core),
            scratch,
            name: name.to_owned(),
            store,
        }
    }

    /// The folder its files live in.
    pub fn root(&self) -> PathBuf {
        self.scratch.join("root")
    }

    /// Its transfer folder: where incoming bytes stage and job records are
    /// kept.
    pub fn transfers(&self) -> PathBuf {
        self.scratch.join("transfers")
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

/// Connecting: what one engine does to reach another, as the application
/// does it.
impl Engine {
    /// This engine hosts, and it and `viewer` trust each other, as two
    /// computers do once paired. After [`Self::restart`] it tells `viewer`
    /// where it now listens too.
    pub fn pair_with(&self, viewer: &Engine) -> Result<(), Error> {
        let invitation = Duration::from_secs(60);
        self.core.start_hosting()?;
        viewer
            .core
            .accept_invitation(&self.core.create_invitation(invitation)?)?;
        self.core
            .accept_invitation(&viewer.core.create_invitation(invitation)?)?;
        Ok(())
    }

    /// This engine's services connection to `host`, as the application
    /// opens one, once `host` admitted it; the refusal when it was refused.
    pub fn connect_services(&self, host: &Engine) -> Result<SessionInfo, Error> {
        let pending = self.core.begin_connect_services(&host.endpoint())?;
        self.settled(pending.id, "the services connection to be admitted")
    }

    /// This engine's control session to `host`, the one a desktop opens,
    /// once active; the refusal when it was refused.
    pub fn connect_control(&self, host: &Engine) -> Result<SessionInfo, Error> {
        let pending = self.core.begin_connect(&host.endpoint())?;
        self.settled(pending.id, "the control session to open")
    }

    fn settled(&self, session: u64, what: &str) -> Result<SessionInfo, Error> {
        wait_for(what, DEFAULT_WAIT, || {
            match self.core.controller_session(session) {
                Ok(session) if session.phase == SessionPhase::Active => Some(Ok(session)),
                Ok(session)
                    if matches!(
                        session.phase,
                        SessionPhase::Failed(_) | SessionPhase::Closed
                    ) =>
                {
                    Some(Err(Error::Unauthorized))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
    }

    /// A Desktop service on `connection`, opened as the application opens
    /// one.
    pub fn open_desktop(&self, connection: &SessionInfo) -> Result<RemoteDesktopService, Error> {
        let owner = self.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Desktop,
        )?;
        self.core.open_desktop_service(owner.id, owner.generation)
    }

    /// A Files service on `connection` and its pipe, opened as the Files tab
    /// opens one.
    pub fn open_files(
        &self,
        connection: &SessionInfo,
    ) -> Result<(ServiceInfo, RemoteFilesService), Error> {
        let owner = self.core.attach_controller_service(
            connection.id,
            connection.generation,
            ServiceKind::Files,
        )?;
        let pipe = self.core.open_files_service(owner.id, owner.generation)?;
        Ok((owner, pipe))
    }
}

/// Serving: what a host engine serves its viewers.
impl Engine {
    /// The services of `kind` this engine serves now; every service, for
    /// `None`.
    pub fn hosted(&self, kind: Option<ServiceKind>) -> Vec<ServiceInfo> {
        self.core
            .hosted_services()
            .expect("the services this engine serves")
            .into_iter()
            .filter(|service| kind.is_none_or(|kind| service.kind == kind))
            .collect()
    }

    /// The service of `kind` this engine serves `viewer` now, if any.
    pub fn hosted_for(&self, kind: ServiceKind, viewer: &Engine) -> Option<ServiceInfo> {
        let peer = viewer.endpoint();
        self.hosted(Some(kind))
            .into_iter()
            .find(|service| service.peer_id == peer)
    }

    /// The service of `kind` this engine serves `viewer`, once it serves it.
    pub fn serving(&self, kind: ServiceKind, viewer: &Engine) -> ServiceInfo {
        let peer = viewer.endpoint();
        wait_for(
            &format!("the host to serve the viewer's {kind:?}"),
            DEFAULT_WAIT,
            || {
                self.hosted(Some(kind))
                    .into_iter()
                    .find(|service| service.peer_id == peer)
            },
        )
    }

    /// Waits until this engine serves no service of `kind`, or none at all
    /// for `None`: its viewers' closes have reached it.
    pub fn wait_released(&self, kind: Option<ServiceKind>) {
        wait_for(
            &format!("the host to release its {kind:?} services"),
            DEFAULT_WAIT,
            || self.hosted(kind).is_empty().then_some(()),
        );
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

/// Settles an engine as the suites run one: named `name`, with codecs, its
/// transfer folder and its one file root under `scratch`.
fn configure(name: &str, core: &Core, scratch: &Scratch) {
    // Every event this engine records says it is `name`, and other
    // engines know its computer by that name, wherever the suite runs.
    core.set_trace_name(name);
    core.set_hostname(name).expect("a computer name");
    // A session needs codecs to open, whatever it carries.
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
        .expect("codecs");
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
        pair.host.pair_with(&pair.controller).expect("pairing");
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
        self.controller
            .connect_services(&self.host)
            .expect("an admitted connection")
    }

    /// The controller's control session to the host, once active.
    pub fn connect_control(&self) -> SessionInfo {
        self.controller
            .connect_control(&self.host)
            .expect("an active control session")
    }

    /// Another viewer named `name`, paired with the host as the controller
    /// is; it shares nothing of its own.
    pub fn add_viewer(&self, name: &str) -> Engine {
        let viewer = Engine::start(name);
        self.host.pair_with(&viewer).expect("pairing");
        viewer
    }

    /// After either engine restarted, the host hosts again and each tells
    /// the other where it now listens.
    pub fn reintroduce(&self) {
        self.host.pair_with(&self.controller).expect("pairing");
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
