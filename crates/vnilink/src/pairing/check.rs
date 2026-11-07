// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Six-digit checks for a pairing file and for a control request.
//!
//! The file check is computed from the signed invitation, so the computer
//! that wrote the file and the computer that opens it show the same digits.
//! The person opening the file types the digits on the writer's screen.
//!
//! The control check is computed from the TLS session of the connection the
//! controller opened, and from both endpoint ids. The controller shows it
//! once connected; the host's owner types it to grant control. A different
//! connection, or a different pair of computers, produces different digits,
//! so a peer cannot choose its key or its nonce to match digits it has not
//! seen.

use sha2::{Digest, Sha256};

const FILE_DOMAIN: &[u8] = b"vnilink-pairing-file-v2";
const CONTROL_LABEL: &[u8] = b"vnilink-control-request-v2";

/// Digits typed for one check.
pub(crate) const CHECK_DIGITS: usize = 6;

/// Digits for one `.vnl` file. The host id, the invitation nonce, and the
/// expiry are fixed for that file, so both computers hash the same bytes.
pub(crate) fn file_digits(endpoint_id: &[u8; 32], nonce: &[u8; 16], expires_at_ms: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FILE_DOMAIN);
    hasher.update(endpoint_id);
    hasher.update([0]);
    hasher.update(nonce);
    hasher.update([0]);
    hasher.update(expires_at_ms.to_be_bytes());
    digits(&hasher.finalize())
}

/// Key material both ends of one connection derive for a control request.
///
/// `controller` and `host` are the endpoint ids, in that order on both sides.
pub(crate) fn control_secret(
    connection: &iroh::endpoint::Connection,
    controller: &[u8; 32],
    host: &[u8; 32],
) -> Option<[u8; 32]> {
    let mut context = [0u8; 64];
    context[..32].copy_from_slice(controller);
    context[32..].copy_from_slice(host);
    let mut secret = [0u8; 32];
    connection
        .export_keying_material(&mut secret, CONTROL_LABEL, &context)
        .ok()?;
    Some(secret)
}

/// Digits for a control request from the exported key material.
pub(crate) fn control_digits(secret: &[u8; 32]) -> String {
    digits(secret)
}

/// Whether a typed check equals the expected one, in time that does not
/// depend on how many leading digits agree.
pub(crate) fn same_check(expected: &str, typed: &str) -> bool {
    use subtle::ConstantTimeEq;
    expected.as_bytes().ct_eq(typed.as_bytes()).into()
}

fn digits(digest: &[u8]) -> String {
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{:0width$}", value % 1_000_000, width = CHECK_DIGITS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_digits_change_when_the_invitation_changes() {
        let endpoint = [7u8; 32];
        let nonce = [9u8; 16];
        let check = file_digits(&endpoint, &nonce, 50);
        assert_eq!(check, file_digits(&endpoint, &nonce, 50));
        assert_eq!(check.len(), CHECK_DIGITS);
        assert!(check.bytes().all(|byte| byte.is_ascii_digit()));
        assert_ne!(check, file_digits(&endpoint, &nonce, 51));
        assert_ne!(check, control_digits(&[7u8; 32]));
    }

    #[test]
    fn control_digits_follow_the_connection_not_the_peer() {
        let one = control_digits(&[1u8; 32]);
        let another = control_digits(&[2u8; 32]);
        assert_eq!(one.len(), CHECK_DIGITS);
        assert_ne!(one, another);
        assert_eq!(one, control_digits(&[1u8; 32]));
    }

    #[test]
    fn a_typed_check_must_equal_the_expected_one() {
        assert!(same_check("042042", "042042"));
        assert!(!same_check("042042", "042043"));
        assert!(!same_check("042042", "04204"));
        assert!(!same_check("042042", ""));
    }

    #[test]
    fn leading_zeros_are_kept() {
        let mut digest = [0u8; 32];
        digest[3] = 7;
        assert_eq!(digits(&digest), "000007");
    }
}
