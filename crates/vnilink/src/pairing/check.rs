//! Three-digit checks for a pairing file and for a control request.
//!
//! The file check is computed from the signed invitation, so the computer
//! that wrote the file and the computer that opens it show the same digits.
//! The person opening the file types the digits on the writer's screen.
//!
//! The control check is computed from the controller's endpoint id alone.
//! The controller can show it before Connect. The host shows the same digits
//! when that controller's connection arrives, and types those digits to grant
//! control. A different computer produces different digits.

use sha2::{Digest, Sha256};

const FILE_DOMAIN: &[u8] = b"vnilink-pairing-file-v1";
const CONTROL_DOMAIN: &[u8] = b"vnilink-control-request-v1";

/// Digits for one `.vnl` file. The host id, the invitation nonce, and the
/// expiry are fixed for that file, so both computers hash the same bytes.
/// The integer is the first four digest bytes, big-endian, modulo 1000.
pub(crate) fn file_digits(endpoint_id: &[u8; 32], nonce: &[u8; 16], expires_at_ms: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FILE_DOMAIN);
    hasher.update(endpoint_id);
    hasher.update([0]);
    hasher.update(nonce);
    hasher.update([0]);
    hasher.update(expires_at_ms.to_be_bytes());
    three_digits(&hasher.finalize())
}

/// Digits for the controller that is asking to connect. Both sides hash the
/// controller's endpoint id and nothing else.
pub(crate) fn control_digits(endpoint_id: &[u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CONTROL_DOMAIN);
    hasher.update(endpoint_id);
    three_digits(&hasher.finalize())
}

fn three_digits(digest: &[u8]) -> String {
    let value = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    format!("{:03}", value % 1000)
}
