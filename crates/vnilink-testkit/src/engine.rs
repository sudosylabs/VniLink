use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use vnilink::{Core, Grants, MemoryStore, RelaySettings};

use crate::Scratch;

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
        let scratch = Scratch::new(name);
        let core = Core::open_direct(Arc::new(MemoryStore::empty()), RelaySettings::public_n0())
            .expect("a loopback engine");
        // A session needs codecs to open, whatever it carries.
        core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
            .expect("codecs");
        std::fs::create_dir_all(scratch.join("root")).expect("a file root");
        core.set_transfer_directory(scratch.join("transfers"))
            .expect("a transfer folder");
        core.set_file_roots(vec![scratch.join("root")])
            .expect("a file root");
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
        let pair = Self {
            host: Engine::start("host"),
            controller: Engine::start("controller"),
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

    /// Both sides may move files, as the Files tab needs.
    pub fn for_files() -> Self {
        let transfer = Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        };
        // A session opens under view and control.
        Self::new(
            Grants {
                view: true,
                control: true,
                ..transfer
            },
            transfer,
        )
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
