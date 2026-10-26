//! Fixtures shared by the unit tests of more than one module.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Addresses a temporary fixture inside the whole-filesystem namespace.
///
/// Fixtures are ordinary folders, never substitute advertised roots. Only
/// paths below the fixture are used for writes.
#[derive(Clone)]
pub(crate) struct FilesystemLocation {
    pub(crate) root: u64,
    relative: String,
}

impl FilesystemLocation {
    pub(crate) fn new(roots: Vec<(u64, PathBuf)>, fixture: &Path) -> Self {
        let fixture = std::fs::canonicalize(fixture).expect("fixture exists");
        let (root, relative) = roots
            .into_iter()
            .find_map(|(id, path)| {
                fixture.strip_prefix(path).ok().map(|relative| {
                    let relative = relative
                        .components()
                        .map(|part| part.as_os_str().to_str().expect("UTF-8 fixture path"))
                        .collect::<Vec<_>>()
                        .join("/");
                    (id, relative)
                })
            })
            .expect("fixture belongs to an advertised filesystem root");
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
/// `host`: the files-only session that carries job control is admitted on
/// the Transfer grant.
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

/// Opens a presence link from `controller` to `host`, paired, and waits
/// until it is active.
pub(crate) fn connect_link(
    controller: &crate::core::Core,
    host: &crate::core::Core,
) -> crate::SessionInfo {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let session = controller.controller_session(pending.id).unwrap();
        if session.phase == crate::SessionPhase::Active {
            return session;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the link did not become active: {session:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
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
