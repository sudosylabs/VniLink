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

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use vnilink_media::Error;

/// Where one restore token is kept.
pub struct ScreenGrant {
    path: PathBuf,
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
        let failed = |_| Error::InvalidSetting("the screen sharing grant could not be kept");
        if token.is_empty() || token.contains('\n') {
            return Err(Error::InvalidSetting(
                "the portal's restore token is malformed",
            ));
        }
        let directory = self.path.parent().unwrap_or(Path::new("."));
        let temporary = directory.join(format!(
            ".{}.{}",
            self.path
                .file_name()
                .map_or("screen-grant".into(), |name| name.to_string_lossy()),
            std::process::id()
        ));
        let mut file = private_file(&temporary).map_err(failed)?;
        let written = file
            .write_all(token.as_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&temporary, &self.path));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.map_err(failed)
    }
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
