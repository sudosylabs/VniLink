// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Fixtures shared by the unit tests of more than one module, for tests
//! that reach this crate's internals. A test that needs only the public
//! API belongs in the service's `vnilink-testkit` suite instead.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

// The kit's bounded waits, one copy shared by inclusion: the file uses only
// `std`, and this crate cannot depend on the kit, which enables this
// crate's `test-probes` feature for its own build.
// Wait on an event, a join handle or a channel wherever one exists.
#[path = "../../vnilink-testkit/src/wait.rs"]
mod wait;
pub(crate) use wait::{holds, wait_for, DEFAULT_WAIT};

/// An engine on loopback with relays off, for a module's test that needs a
/// whole engine and an internal the kit cannot reach.
pub(crate) fn loopback_core() -> crate::core::Core {
    let core = crate::core::Core::boot(
        crate::core::CoreConfig {
            listen: crate::core::Listen::Loopback,
            ..crate::core::CoreConfig::default()
        },
        iroh::RelayMode::Disabled,
    )
    .expect("a loopback engine");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("codec flags");
    core
}

/// A unique temporary directory, removed with everything in it on drop.
pub(crate) struct Scratch {
    pub(crate) path: PathBuf,
}

impl Scratch {
    pub(crate) fn tagged(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vnilink-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Addresses a temporary fixture on the shared disk as the engine names
/// it ([`crate::core::Core::locate`], [`crate::fs::RootSet::locate`]), never
/// a disk's root itself, so tests write only below their fixtures.
#[derive(Clone)]
pub(crate) struct FilesystemLocation {
    pub(crate) root: u64,
    relative: String,
}

impl FilesystemLocation {
    /// `fixture` on `core`'s shared disk.
    pub(crate) fn on(core: &crate::core::Core, fixture: &Path) -> Self {
        let (root, parent, name) = core
            .locate(fixture)
            .expect("the engine is running")
            .expect("the fixture is below a root of the shared disk");
        let relative = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        Self { root, relative }
    }

    /// `fixture` among `roots`, for tests without an engine.
    pub(crate) fn in_roots(roots: &crate::fs::RootSet, fixture: &Path) -> Self {
        let (root, relative) = roots
            .locate(fixture)
            .expect("the fixture is on the shared disk");
        assert!(
            !relative.is_empty(),
            "writes must stay inside a temporary fixture"
        );
        Self { root, relative }
    }

    pub(crate) fn path(&self, child: &str) -> String {
        if child.is_empty() {
            self.relative.clone()
        } else {
            format!("{}/{child}", self.relative)
        }
    }

    /// [`Self::path`] as a job names what it copies: the folder it is in
    /// and its name there.
    pub(crate) fn entry(&self, child: &str) -> (String, Vec<String>) {
        let path = self.path(child);
        match path.rsplit_once('/') {
            Some((folder, name)) => (folder.to_owned(), vec![name.to_owned()]),
            None => (String::new(), vec![path]),
        }
    }
}

/// Pairs both ways and grants `controller` view and control of `host`.
pub(crate) fn pair_for_control(host: &crate::core::Core, controller: &crate::core::Core) {
    // Negotiation needs an encoder on the host and a decoder on the
    // controller. Tests that cover the unsupported paths set their own flags.
    host.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller
        .set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    pair_granting(
        host,
        controller,
        crate::session::Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: false,
        },
    );
}

/// Pairs both ways and grants `controller` view, control and transfer on
/// `host`: the Files service that carries job control is admitted on the
/// Transfer grant.
pub(crate) fn pair_for_transfer(host: &crate::core::Core, controller: &crate::core::Core) {
    pair_granting(
        host,
        controller,
        crate::session::Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: true,
        },
    );
}

/// The controller's Files on `host`: a presence link and a Files service
/// on it, opened as the Files tab opens them. The pipe keeps the service,
/// and so its jobs, alive: a test holds it for as long as it moves files.
pub(crate) fn open_files(
    controller: &crate::core::Core,
    host: &crate::core::Core,
) -> (crate::ServiceInfo, crate::RemoteFilesService) {
    let link = connect_link(controller, host);
    let owner = controller
        .attach_controller_service(link.id, link.generation, crate::ServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    (owner, pipe)
}

/// Opens a presence link from `controller` to `host`, paired, and waits
/// until it is active.
pub(crate) fn connect_link(
    controller: &crate::core::Core,
    host: &crate::core::Core,
) -> crate::SessionInfo {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait_active(controller, pending.id)
}

/// Waits, on the engine's own session events, until the controller session
/// `id` is active, and fails at once if it ends or fails instead.
pub(crate) fn wait_active(controller: &crate::core::Core, id: u64) -> crate::SessionInfo {
    wait_for_event(
        controller,
        "the link to become active",
        DEFAULT_WAIT,
        || {
            let session = controller.controller_session(id).unwrap();
            match session.phase {
                crate::SessionPhase::Active => Some(session),
                crate::SessionPhase::Closed | crate::SessionPhase::Failed(_) => {
                    panic!("the link ended before it was active: {session:?}")
                }
                _ => None,
            }
        },
    )
}

/// Returns what `ready` finds once it holds, re-evaluating it after each
/// event the engine publishes. The subscription opens before the first
/// check, so no change slips between the check and the wait.
pub(crate) fn wait_for_event<T>(
    core: &crate::core::Core,
    what: &str,
    within: std::time::Duration,
    mut ready: impl FnMut() -> Option<T>,
) -> T {
    let events = core.subscribe().expect("an event subscription");
    let deadline = std::time::Instant::now() + within;
    loop {
        if let Some(found) = ready() {
            return found;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!left.is_zero(), "timed out waiting for {what}");
        let _ = events.recv_timeout(left);
    }
}

fn pair_granting(
    host: &crate::core::Core,
    controller: &crate::core::Core,
    grants: crate::session::Grants,
) {
    host.start_hosting().unwrap();
    let token = host
        .create_invitation(std::time::Duration::from_secs(60))
        .unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(std::time::Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.set_desktop_sharing(grants).unwrap();
}

/// Two loopback engines paired for transfer both ways, with whole-filesystem
/// access and their own scratch trees: `root` is the filesystem fixture,
/// `blobs` the transfer directory.
pub(crate) fn transfer_pair(tag: &str) -> (crate::core::Core, crate::core::Core, Scratch, Scratch) {
    let host = transfer_core();
    let controller = transfer_core();
    let host_scratch = Scratch::tagged(&format!("{tag}-host"));
    let controller_scratch = Scratch::tagged(&format!("{tag}-controller"));
    for (core, scratch) in [(&host, &host_scratch), (&controller, &controller_scratch)] {
        std::fs::create_dir_all(scratch.join("root")).unwrap();
        core.set_transfer_directory(scratch.join("blobs")).unwrap();
        core.enable_filesystem_access().unwrap();
    }
    pair_for_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    (host, controller, host_scratch, controller_scratch)
}

fn transfer_core() -> crate::core::Core {
    let core = loopback_core();
    // These correctness fixtures may live on a small /dev/shm volume. Resource
    // admission has separate exact-boundary tests; don't require production
    // free-space headroom for these small temporary fixtures.
    core.set_file_resource_policy(crate::job::FileResourcePolicy {
        minimum_free_bytes: 0,
        ..Default::default()
    })
    .unwrap();
    core
}

/// A host desktop that records what it is asked to apply.
pub(crate) struct RecordingDesktop {
    pub(crate) events: std::sync::Arc<std::sync::Mutex<Vec<crate::input::InputEvent>>>,
}

impl crate::input::HostDesktop for RecordingDesktop {
    fn apply(&mut self, event: crate::input::InputEvent) -> Result<(), crate::error::Error> {
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
