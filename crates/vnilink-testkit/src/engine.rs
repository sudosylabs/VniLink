// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{
    Core, Error, Grants, MemoryStore, RelaySettings, RemoteDesktopService, RemoteFilesService,
    ServiceInfo, ServiceKind, SessionInfo, SessionPhase,
};

use crate::{wait_for, Scratch, DEFAULT_WAIT};

/// Where an engine listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Loopback only, with no relay: nothing leaves this computer.
    Loopback,
    /// Every network this computer has and the public relays, as the
    /// application listens.
    Network,
}

/// A real engine on loopback, as the application runs one, with its own
/// identity in memory and its own folders: [`Self::folder`], the folder its
/// files live in, and `transfers`, where incoming bytes stage. It never
/// reaches a relay, so nothing leaves this computer.
///
/// It shares its whole disk for Files, as the application does: the disk
/// is the only thing Files shares. A test names paths only inside its own
/// folders, through [`Self::location`] or [`crate::files::Location`], so
/// nothing it writes lands outside them.
pub struct Engine {
    pub core: Arc<Core>,
    scratch: Scratch,
    name: String,
    /// Its installation: identity and records, kept for [`Self::restart`].
    store: MemoryStore,
    /// Whether it shares its whole disk from the start.
    disk: Disk,
}

/// Whether an engine shares its whole disk from the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disk {
    Shared,
    /// Shared only once something enables it, as the test agent does when
    /// its owner allows files.
    Kept,
}

impl Engine {
    /// `name` tags its folders, such as `host` or `controller`.
    pub fn start(name: &str) -> Self {
        crate::peer::refuse_peer();
        let store = MemoryStore::empty();
        // Named from its first record, as binding its endpoint.
        vnilink::trace::name_next_engine(name);
        let core = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
            .expect("a loopback engine");
        Self::prepare(name, core, store, Disk::Shared)
    }

    /// Shuts this engine down and opens its installation again, as quitting
    /// and starting the application does: the same identity, records and
    /// folders.
    pub fn restart(&mut self) {
        self.core.shutdown();
        vnilink::trace::name_next_engine(&self.name);
        let core = Core::open_direct(Arc::new(self.store.clone()), RelaySettings::public_n0())
            .expect("a loopback engine");
        configure(&self.name, &core, &self.scratch, self.disk);
        self.core = Arc::new(core);
    }

    /// An engine with no direct address, whose only way to its peers is the
    /// relay at `relay` ([`crate::relay::LocalRelay`]).
    pub fn relayed(name: &str, relay: &str) -> Self {
        crate::peer::refuse_peer();
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
        Self::prepare(name, core, store, Disk::Shared)
    }

    /// An engine another computer can reach, as the application opens one,
    /// or one on loopback only for `Reach::Loopback`: the test agent's and
    /// the suite's engines when they pair by file. Its identity lives in
    /// memory and ends with the process.
    pub fn reachable(name: &str, reach: Reach) -> Self {
        Self::open_reachable(name, reach, Disk::Shared)
    }

    /// As [`Self::reachable`], sharing no disk until something enables it:
    /// the test agent's engine, whose disk is readied only when its owner
    /// allows files and reached only while the files scenario runs.
    pub(crate) fn keeping_its_disk(name: &str, reach: Reach) -> Self {
        Self::open_reachable(name, reach, Disk::Kept)
    }

    fn open_reachable(name: &str, reach: Reach, disk: Disk) -> Self {
        if reach == Reach::Loopback {
            crate::peer::refuse_peer();
        }
        let store = MemoryStore::empty();
        vnilink::trace::name_next_engine(name);
        let core = match reach {
            Reach::Loopback => {
                Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
                    .expect("a loopback engine")
            }
            Reach::Network => Core::open(Arc::new(store.clone()), vnilink::OpenConfig::default())
                .expect("an engine on this computer's network"),
        };
        Self::prepare(name, core, store, disk)
    }

    fn prepare(name: &str, core: Core, store: MemoryStore, disk: Disk) -> Self {
        let scratch = Scratch::new(name);
        std::fs::create_dir_all(scratch.join("files")).expect("the engine's folder");
        configure(name, &core, &scratch, disk);
        Self {
            core: Arc::new(core),
            scratch,
            name: name.to_owned(),
            store,
            disk,
        }
    }

    /// The folder its files live in, inside its scratch folder.
    pub fn folder(&self) -> PathBuf {
        self.scratch.join("files")
    }

    /// [`Self::folder`] as the engine names it on its shared disk: every
    /// path a test hands the engine is made from this, so it stays inside
    /// the engine's own folder.
    pub fn location(&self) -> crate::files::Location {
        crate::files::Location::of(&self.core, &self.folder())
    }

    /// Its transfer folder: where incoming bytes stage and job records are
    /// kept.
    pub fn transfers(&self) -> PathBuf {
        self.scratch.join("transfers")
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
        pair_cores(&self.core, &viewer.core)
    }

    /// As [`Self::pair_with`], this engine sharing itself to be seen and
    /// controlled ([`SEEN_AND_CONTROLLED`]): a [`Pair`] for several engines.
    pub fn pair_for_control(&self, viewer: &Engine) -> Result<(), Error> {
        pair_cores_for_control(&self.core, &viewer.core)
    }

    /// This engine's services connection to `host`, as the application
    /// opens one, once `host` admitted it; the refusal when it was refused.
    pub fn connect_services(&self, host: &Engine) -> Result<SessionInfo, Error> {
        self.connect_services_to(&host.endpoint())
    }

    /// As [`Self::connect_services`], to the host whose endpoint is
    /// `endpoint`, for a host the kit did not start.
    pub fn connect_services_to(&self, endpoint: &str) -> Result<SessionInfo, Error> {
        let pending = self.core.begin_connect_services(endpoint)?;
        self.settled(pending.id, "the services connection to be admitted")
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
        match self.core.open_files_service(owner.id, owner.generation) {
            Ok(pipe) => Ok((owner, pipe)),
            Err(error) => {
                // A refused service leaves no record, as the Files tab
                // leaves none: the next attempt is not busy with it.
                self.core
                    .close_controller_service(owner.id, owner.generation)?;
                self.core.forget_controller_service(owner.id)?;
                Err(error)
            }
        }
    }

    /// This engine's Files on `host`, as the Files tab reaches a computer:
    /// a services connection and a Files service on it, once `host`
    /// admitted both; the refusal when it refused either.
    pub fn files_on(&self, host: &Engine) -> Result<Files, Error> {
        self.files_at(&host.endpoint())
    }

    /// As [`Self::files_on`], on the host whose endpoint is `endpoint`,
    /// for a host the kit did not start.
    pub fn files_at(&self, endpoint: &str) -> Result<Files, Error> {
        let connection = self.connect_services_to(endpoint)?;
        let (service, pipe) = self.open_files(&connection)?;
        Ok(Files {
            connection,
            service,
            pipe,
        })
    }
}

/// A controller's Files on a host, as the Files tab holds them: the
/// services connection, the Files service on it, and the pipe its listings
/// and changes go through. Its jobs ride the service, so it lives as long
/// as a test moves files; dropping it closes the service, and the
/// connection stays.
pub struct Files {
    pub connection: SessionInfo,
    pub service: ServiceInfo,
    pub pipe: RemoteFilesService,
}

impl Files {
    /// Closes the service, as closing the Files tab does: the connection
    /// stays.
    pub fn close(&self) {
        self.pipe.close();
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
/// transfer folder under `scratch`, and its whole disk shared unless
/// `disk` keeps it.
fn configure(name: &str, core: &Core, scratch: &Scratch, disk: Disk) {
    // Every event this engine records says it is `name`, and other
    // engines know its computer by that name, wherever the suite runs.
    core.set_trace_name(name);
    core.set_hostname(name).expect("a computer name");
    // A session needs codecs to open, whatever it carries.
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
        .expect("codecs");
    core.set_transfer_directory(scratch.join("transfers"))
        .expect("a transfer folder");
    if disk == Disk::Shared {
        core.enable_filesystem_access()
            .expect("the whole disk shared");
    }
    // A suite's files are small next to the free space the product keeps
    // in reserve; admission has its own tests in `vnilink`.
    core.set_file_resource_policy(vnilink::job::FileResourcePolicy {
        minimum_free_bytes: 0,
        ..Default::default()
    })
    .expect("a resource policy");
}

/// `host` hosts, and it and `viewer` trust each other, as two computers do
/// once paired: [`Engine::pair_with`] for engines the kit did not start.
pub fn pair_cores(host: &Core, viewer: &Core) -> Result<(), Error> {
    let invitation = Duration::from_secs(60);
    host.start_hosting()?;
    viewer.accept_invitation(&host.create_invitation(invitation)?)?;
    host.accept_invitation(&viewer.create_invitation(invitation)?)?;
    Ok(())
}

/// As [`pair_cores`], `host` sharing itself to be seen and controlled.
pub fn pair_cores_for_control(host: &Core, viewer: &Core) -> Result<(), Error> {
    pair_cores(host, viewer)?;
    host.set_desktop_sharing(SEEN_AND_CONTROLLED)
}

/// What a host shares once paired for control: it may be seen and
/// controlled, and no more.
pub const SEEN_AND_CONTROLLED: Grants = Grants {
    view: true,
    control: true,
    clipboard: false,
    transfer: false,
};

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

    /// As [`Self::new`], the host sharing no disk, as an engine is before
    /// its owner enables filesystem access: a host that offers no Files.
    pub fn host_keeping_its_disk(host: Grants, controller: Grants) -> Self {
        Self::paired(
            Engine::keeping_its_disk("host", Reach::Loopback),
            Engine::start("controller"),
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

    /// The controller's Files on the host, as the Files tab reaches it,
    /// once the host admitted them.
    pub fn open_files(&self) -> Files {
        self.controller
            .files_on(&self.host)
            .expect("the host's Files")
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
        Self::new(
            SEEN_AND_CONTROLLED,
            Grants {
                view: false,
                control: false,
                clipboard: false,
                transfer: false,
            },
        )
    }
}
