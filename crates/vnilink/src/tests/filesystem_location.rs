//! Addresses temporary fixtures inside the actual whole-filesystem namespace.
//!
//! Fixtures are ordinary folders, never substitute advertised roots. Only
//! paths below the fixture are used for writes; drive/root discovery exercises
//! the same namespace as the application.

use std::path::{Path, PathBuf};

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
