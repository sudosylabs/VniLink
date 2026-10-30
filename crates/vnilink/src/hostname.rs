// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! This installation's operating-system hostname for display only.
//!
//! The hostname is convenience metadata around the stable endpoint id,
//! the same split RustDesk uses: renaming never breaks trust. It is read
//! from the operating system, validated, and shown next to the key
//! fragment. It is never logged as an identity and never authorizes
//! anything.
//!
//! No new dependency and no unsafe code: the name comes from the
//! platform `hostname` program, which exists on Linux, macOS, and
//! Windows. Native apps may later supply their own value from Swift or
//! C# (`ProcessInfo.hostName`, `Dns.GetHostName`); this is the portable
//! fallback every platform shares.

/// Longest hostname kept, in bytes. Covers `HOST_NAME_MAX` (64) with
/// headroom, and always fits the one-byte wire length. Anything longer
/// falls back to the key fragment in the UI.
pub(crate) const MAX_HOSTNAME_BYTES: usize = 128;

/// Reads this machine's hostname. Returns `None` when the program is
/// missing, fails, or reports something unusable. Call it at pairing or
/// app start, not per frame: it spawns one short-lived process.
pub fn local_hostname() -> Option<String> {
    let output = std::process::Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    clean_hostname(&text)
}

/// Validates a hostname for display. Trims whitespace, rejects empties,
/// control characters, and overlong values. Anything else passes through
/// unchanged, including dotted and international names.
pub fn clean_hostname(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_HOSTNAME_BYTES {
        return None;
    }
    if trimmed.chars().any(|point| point.is_control()) {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaning_keeps_names_and_rejects_garbage() {
        assert_eq!(clean_hostname("  mac-mini  "), Some("mac-mini".to_string()));
        assert_eq!(
            clean_hostname("laptop.example.com"),
            Some("laptop.example.com".to_string())
        );
        assert_eq!(clean_hostname(""), None);
        assert_eq!(clean_hostname("   "), None);
        assert_eq!(clean_hostname("a\tb"), None);
        assert_eq!(clean_hostname(&"a".repeat(129)), None);
        assert_eq!(
            clean_hostname(&"a".repeat(128)).map(|name| name.len()),
            Some(128)
        );
        // Byte cap, not char cap: 65 two-byte names exceed 128 bytes.
        assert_eq!(clean_hostname(&"é".repeat(65)), None);
        assert_eq!(
            clean_hostname("mac-mini.local"),
            Some("mac-mini.local".to_string())
        );
    }

    #[test]
    fn reading_never_panics_and_stays_valid() {
        // Environment-dependent: a missing `hostname` program is fine.
        if let Some(name) = local_hostname() {
            assert_eq!(clean_hostname(&name), Some(name));
        }
    }
}
