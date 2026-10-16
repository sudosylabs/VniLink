//! Authenticated encryption for one installation record.
//!
//! The key stays in the platform credential store. This module never logs
//! key bytes, plaintext, or ciphertext.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use super::StoreFailure;

pub(crate) const KEY_LEN: usize = 32;
pub(crate) const NONCE_LEN: usize = 24;
pub(crate) const TAG_LEN: usize = 16;
pub(crate) const MAX_PLAINTEXT: usize = 1_048_576;
pub(crate) const MAX_CIPHERTEXT: usize = MAX_PLAINTEXT + TAG_LEN;

const KEY_ID_PREFIX: &[u8] = b"com.vnilink.storage/key-id\0";
const RECORD_PREFIX: &[u8] = b"com.vnilink.storage/installation-record\0";
const RECORD_VERSION: u64 = 1;

pub(crate) fn key_id(key: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(KEY_ID_PREFIX);
    hasher.update(key);
    let digest = hasher.finalize();
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&digest);
    out
}

pub(crate) fn random_nonce() -> Result<[u8; NONCE_LEN], StoreFailure> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| StoreFailure::Unavailable)?;
    Ok(nonce)
}

pub(crate) fn random_key() -> Result<zeroize::Zeroizing<[u8; KEY_LEN]>, StoreFailure> {
    let mut key = zeroize::Zeroizing::new([0u8; KEY_LEN]);
    getrandom::getrandom(key.as_mut()).map_err(|_| StoreFailure::Unavailable)?;
    Ok(key)
}

/// Encrypts `plaintext`. Each call uses a new nonce, including a retry of the
/// same bytes. The returned ciphertext includes the 16-byte tag.
pub(crate) fn encrypt(
    key: &[u8; KEY_LEN],
    id: &[u8; KEY_LEN],
    plaintext: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), StoreFailure> {
    if plaintext.is_empty() || plaintext.len() > MAX_PLAINTEXT {
        return Err(StoreFailure::Unavailable);
    }
    let nonce = random_nonce()?;
    let mut cipher_key = *key;
    let ciphertext = seal(&cipher_key, id, &nonce, plaintext);
    cipher_key.zeroize();
    let ciphertext = ciphertext?;
    if !(TAG_LEN + 1..=MAX_CIPHERTEXT).contains(&ciphertext.len()) {
        return Err(StoreFailure::Unavailable);
    }
    Ok((nonce, ciphertext))
}

pub(crate) fn decrypt(
    key: &[u8; KEY_LEN],
    id: &[u8; KEY_LEN],
    stored_id: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, StoreFailure> {
    if stored_id.len() != KEY_LEN || nonce.len() != NONCE_LEN {
        return Err(StoreFailure::Corrupt);
    }
    if !(TAG_LEN + 1..=MAX_CIPHERTEXT).contains(&ciphertext.len()) {
        return Err(StoreFailure::Corrupt);
    }
    if stored_id != id {
        return Err(StoreFailure::Corrupt);
    }
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(nonce);
    let mut cipher_key = *key;
    let plaintext = open(&cipher_key, id, &nonce_bytes, ciphertext);
    cipher_key.zeroize();
    let plaintext = plaintext?;
    if plaintext.is_empty() || plaintext.len() > MAX_PLAINTEXT {
        return Err(StoreFailure::Corrupt);
    }
    Ok(plaintext)
}

fn seal(
    key: &[u8; KEY_LEN],
    id: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> Result<Vec<u8>, StoreFailure> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| StoreFailure::Unavailable)?;
    let aad = record_aad(id);
    let nonce = nonce_value(nonce)?;
    cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| StoreFailure::Unavailable)
}

fn open(
    key: &[u8; KEY_LEN],
    id: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, StoreFailure> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| StoreFailure::Unavailable)?;
    let aad = record_aad(id);
    let nonce = nonce_value(nonce)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| StoreFailure::Corrupt)
}

fn nonce_value(nonce: &[u8; NONCE_LEN]) -> Result<XNonce, StoreFailure> {
    XNonce::try_from(nonce.as_slice()).map_err(|_| StoreFailure::Unavailable)
}

fn record_aad(id: &[u8; KEY_LEN]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(RECORD_PREFIX.len() + 8 + KEY_LEN);
    aad.extend_from_slice(RECORD_PREFIX);
    aad.extend_from_slice(&RECORD_VERSION.to_be_bytes());
    aad.extend_from_slice(id);
    aad
}

/// Compares equal-length secrets without branching on the first mismatch.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}
