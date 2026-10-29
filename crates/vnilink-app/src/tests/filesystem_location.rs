//! Addresses temporary fixtures on the shared disk as the engine names them
//! ([`vnilink::Core::locate`]), never a disk's root itself, so tests write
//! only below their fixtures.

use std::path::Path;

#[derive(Clone)]
pub(crate) struct FilesystemLocation {
    pub(crate) root: u64,
    relative: String,
}

impl FilesystemLocation {
    /// `fixture` on `core`'s shared disk.
    pub(crate) fn on(core: &vnilink::Core, fixture: &Path) -> Self {
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

    pub(crate) fn path(&self, child: &str) -> String {
        if child.is_empty() {
            self.relative.clone()
        } else {
            format!("{}/{child}", self.relative)
        }
    }
}
