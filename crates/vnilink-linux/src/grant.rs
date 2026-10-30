// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The desktop portal's grant to share this computer's screen, kept between
//! starts.
//!
//! Asked to keep a grant until it is revoked, the RemoteDesktop and
//! ScreenCast portals answer a start with a restore token. Offered on the
//! next start, it shares the same screen without asking anyone again, and
//! each start answers with the token for the next. The first start, and
//! any after the grant is revoked in the system's privacy settings, ask.
//!
//! The token stands for that consent. For an application that is not
//! sandboxed the portal cannot tell which program offers it, so it is kept
//! in a file only this user can read, beside the engine's other private
//! state, and never logged.
// trace: records every ignored failure

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use vnilink_media::Error;

/// Where one restore token is kept.
pub struct ScreenGrant {
    path: PathBuf,
}

/// What a kept grant shares, learned when it was last opened, so a host can
/// offer the screen without opening the portal, and without the compositor
/// announcing a share no one is watching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantedScreen {
    pub width: u16,
    pub height: u16,
    /// The compositor reports the pointer apart from the picture.
    pub separate_cursor: bool,
    /// Remote interaction was allowed with the screen.
    pub control: bool,
}

impl ScreenGrant {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The token from the last start, if any. A missing, unreadable or
    /// empty file is no grant: the portal then asks.
    pub fn token(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_owned())
    }

    /// Keeps the token the portal answered a start with, replacing the last
    /// whole: readers see the old token or the new one, never part of one.
    pub fn keep(&self, token: &str) -> Result<(), Error> {
        if token.is_empty() || token.contains('\n') {
            return Err(Error::InvalidSetting(
                "the portal's restore token is malformed",
            ));
        }
        replace_private(&self.path, token)
    }

    /// The screen this grant shared when last opened, if kept.
    pub fn screen(&self) -> Option<GrantedScreen> {
        let text = std::fs::read_to_string(self.screen_path()).ok()?;
        let mut fields = text.split_whitespace();
        let mut next = || fields.next()?.parse::<u16>().ok();
        let (width, height, cursor, control) = (next()?, next()?, next()?, next()?);
        (width > 0 && height > 0 && cursor <= 1 && control <= 1).then_some(GrantedScreen {
            width,
            height,
            separate_cursor: cursor == 1,
            control: control == 1,
        })
    }

    /// Keeps what the grant shares, beside its token.
    pub fn keep_screen(&self, screen: GrantedScreen) -> Result<(), Error> {
        replace_private(
            &self.screen_path(),
            &format!(
                "{} {} {} {}",
                screen.width,
                screen.height,
                u8::from(screen.separate_cursor),
                u8::from(screen.control)
            ),
        )
    }

    fn screen_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".screen");
        self.path.with_file_name(name)
    }
}

/// Replaces `path` whole with `contents`, readable by this user only:
/// readers see the old contents or the new, never part of either.
fn replace_private(path: &Path, contents: &str) -> Result<(), Error> {
    let failed = |_| Error::InvalidSetting("the screen sharing grant could not be kept");
    let directory = path.parent().unwrap_or(Path::new("."));
    let temporary = directory.join(format!(
        ".{}.{}",
        path.file_name()
            .map_or("screen-grant".into(), |name| name.to_string_lossy()),
        std::process::id()
    ));
    let mut file = private_file(&temporary).map_err(failed)?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        vnilink::ignored!(
            vnilink::trace::CAPTURE,
            "a temporary grant file left behind is replaced next time",
            std::fs::remove_file(&temporary)
        );
    }
    written.map_err(failed)
}

/// A new file that only this user can read or write.
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

impl fmt::Debug for ScreenGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScreenGrant")
            .field("path", &self.path)
            .finish()
    }
}
