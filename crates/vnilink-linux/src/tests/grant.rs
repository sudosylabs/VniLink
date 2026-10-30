// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use crate::{GrantedScreen, ScreenGrant};

/// A directory of its own under the system's temporary one, removed after.
struct Scratch(std::path::PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vnilink-grant-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// No file is no grant; a kept token is the next start's, replaced whole,
/// in a file only this user can read.
#[test]
fn a_kept_token_is_offered_on_the_next_start() {
    let scratch = Scratch::new("kept");
    let grant = ScreenGrant::at(scratch.0.join("screen-grant"));
    assert_eq!(grant.token(), None);
    grant.keep("first-token").unwrap();
    assert_eq!(grant.token().as_deref(), Some("first-token"));
    grant.keep("second-token").unwrap();
    assert_eq!(grant.token().as_deref(), Some("second-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(scratch.0.join("screen-grant"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Only the token is left: no temporary file beside it.
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
}

/// What the portal never sends is not kept, and an empty file grants nothing.
#[test]
fn an_empty_or_malformed_token_is_no_grant() {
    let scratch = Scratch::new("empty");
    let path = scratch.0.join("screen-grant");
    let grant = ScreenGrant::at(&path);
    assert!(grant.keep("").is_err());
    assert!(grant.keep("two\nlines").is_err());
    assert_eq!(grant.token(), None);
    std::fs::write(&path, "\n").unwrap();
    assert_eq!(grant.token(), None);
}

/// What the grant shares is kept beside its token, privately, and read back
/// as kept; nothing kept, or anything malformed, is nothing remembered.
#[test]
fn the_shared_screen_is_remembered_beside_the_token() {
    let scratch = Scratch::new("screen");
    let grant = ScreenGrant::at(scratch.0.join("screen-grant"));
    assert_eq!(grant.screen(), None);
    let screen = GrantedScreen {
        width: 1920,
        height: 1080,
        separate_cursor: true,
        control: false,
    };
    grant.keep_screen(screen).unwrap();
    grant.keep("token").unwrap();
    assert_eq!(grant.screen(), Some(screen));
    assert_eq!(grant.token().as_deref(), Some("token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(scratch.0.join("screen-grant.screen"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    std::fs::write(scratch.0.join("screen-grant.screen"), "0 1080 1 0").unwrap();
    assert_eq!(grant.screen(), None);
    std::fs::write(scratch.0.join("screen-grant.screen"), "1920 1080 2 0").unwrap();
    assert_eq!(grant.screen(), None);
}
