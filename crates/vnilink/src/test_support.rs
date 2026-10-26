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
