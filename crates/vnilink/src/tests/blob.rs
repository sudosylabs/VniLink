//! Stage A spike: authenticated `iroh-blobs` transfers above the legacy cap.
//!
//! Two paired loopback peers exchange one file larger than 8 MiB through the
//! peer-isolated blob provider. An unauthorized peer, an unoffered hash and a
//! revoked grant are all denied through the real data path.

use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::session::Grants;

fn start_direct() -> Core {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core")
}

struct Scratch {
    path: std::path::PathBuf,
}

impl Scratch {
    fn tagged(tag: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vnilink-blob-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    fn blobs(&self) -> std::path::PathBuf {
        self.path.join("blobs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn paired_with_dirs(
    host: &Core,
    controller: &Core,
    transfer: bool,
    tag: &str,
) -> (Scratch, Scratch) {
    let host_scratch = Scratch::tagged(&format!("{tag}-host"));
    let controller_scratch = Scratch::tagged(&format!("{tag}-controller"));
    host.set_blob_directory(host_scratch.blobs()).unwrap();
    controller
        .set_blob_directory(controller_scratch.blobs())
        .unwrap();
    pair(host, controller, transfer);
    (host_scratch, controller_scratch)
}

fn pair(host: &Core, controller: &Core, transfer: bool) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer,
        },
    )
    .unwrap();
}

/// Deterministic incompressible-ish payload: xorshift64*, so the bytes are
/// not trivially uniform while staying reproducible without fixtures.
fn payload(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let word = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_encode(&Sha256::digest(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xF) as usize] as char);
    }
    out
}

#[test]
fn blob_spike_moves_a_file_above_the_legacy_cap() {
    let host = start_direct();
    let controller = start_direct();
    let (_hs, _cs) = paired_with_dirs(&host, &controller, true, "cap");

    // 9 MiB clears the legacy 8 MiB whole-file cap and stays under the
    // 32 MiB spike bound.
    let data = payload(9 * 1024 * 1024);
    let want = sha256_hex(&data);

    let (hash, size) = host
        .blob_offer(&controller.endpoint_id(), &data)
        .expect("offer to a paired peer");
    assert_eq!(size, data.len() as u64);

    let got = controller
        .blob_fetch(&host.endpoint_id(), hash, size)
        .expect("authenticated fetch of an offered hash");
    assert_eq!(got.len(), data.len());
    assert_eq!(sha256_hex(&got), want);
    assert_eq!(got, data);
}

#[test]
fn blob_spike_serves_both_directions() {
    let host = start_direct();
    let controller = start_direct();
    let (_hs, _cs) = paired_with_dirs(&host, &controller, true, "both");
    controller
        .authorize(
            &host.endpoint_id(),
            Grants {
                view: false,
                control: false,
                clipboard: false,
                transfer: true,
            },
        )
        .unwrap();

    let data = payload(9 * 1024 * 1024 + 17);
    let (hash, size) = controller
        .blob_offer(&host.endpoint_id(), &data)
        .expect("controller offers to host");
    let got = host
        .blob_fetch(&controller.endpoint_id(), hash, size)
        .expect("host fetches from controller");
    assert_eq!(got, data);
}

#[test]
fn blob_spike_denies_unauthorized_peer_hash_and_revocation() {
    let host = start_direct();
    let controller = start_direct();
    let evil = start_direct();
    let (_hs, _cs) = paired_with_dirs(&host, &controller, true, "deny");
    let evil_scratch = Scratch::tagged("deny-evil");
    evil.set_blob_directory(evil_scratch.blobs()).unwrap();
    // Evil is paired with the host but holds no transfer grant.
    pair(&host, &evil, false);

    let data = payload(1024);
    let (hash, size) = host
        .blob_offer(&controller.endpoint_id(), &data)
        .expect("offer to controller");

    // An unauthorized peer cannot fetch another peer's offered hash.
    let denied = evil.blob_fetch(&host.endpoint_id(), hash, size);
    assert!(
        denied.is_err(),
        "peer without the transfer grant fetched another peer's hash"
    );

    // An authorized peer cannot fetch a hash that was never offered to it.
    let unknown = iroh_blobs::Hash::new(b"vnilink-spike-never-offered");
    let missing = controller.blob_fetch(&host.endpoint_id(), unknown, 25);
    assert!(
        missing.is_err(),
        "authorized peer fetched an unoffered hash"
    );

    // Revocation closes the serving boundary for new fetches.
    host.revoke(&controller.endpoint_id()).unwrap();
    let after_revoke = controller.blob_fetch(&host.endpoint_id(), hash, size);
    assert!(
        after_revoke.is_err(),
        "revoked peer fetched after revocation"
    );
    // The bytes themselves never cross on a denied path; the offer side
    // keeps its copy while the fetchers hold nothing.
    assert_eq!(data.len(), 1024);
}

#[test]
fn blob_spike_rejects_oversize_and_unpaired_offers() {
    let host = start_direct();
    let controller = start_direct();
    let (_hs, _cs) = paired_with_dirs(&host, &controller, true, "limits");

    let too_big = vec![7u8; crate::BLOB_MAX_BYTES + 1];
    assert!(matches!(
        host.blob_offer(&controller.endpoint_id(), &too_big),
        Err(Error::InvalidSetting(_))
    ));

    let stranger = start_direct();
    assert!(matches!(
        host.blob_offer(&stranger.endpoint_id(), b"hi"),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        controller.blob_fetch(
            &host.endpoint_id(),
            iroh_blobs::Hash::new(b"x"),
            crate::BLOB_MAX_BYTES as u64 + 1
        ),
        Err(Error::InvalidSetting(_))
    ));
}

#[test]
fn changing_recent_fetch_retention_reclaims_payload_in_the_running_store() {
    let host = start_direct();
    let receiver = start_direct();
    let (_host_dir, _receiver_dir) = paired_with_dirs(&host, &receiver, true, "retention-policy");
    let data = payload(64 * 1024);
    let (hash, size) = host.blob_offer(&receiver.endpoint_id(), &data).unwrap();
    for policy in [
        crate::job::FileRetentionPolicy {
            recent_fetch_bytes: 0,
            ..Default::default()
        },
        crate::job::FileRetentionPolicy {
            recent_fetch_age_secs: 0,
            ..Default::default()
        },
    ] {
        receiver
            .set_file_retention_policy(Default::default())
            .unwrap();
        assert_eq!(
            receiver
                .blob_fetch(&host.endpoint_id(), hash, size)
                .unwrap(),
            data
        );
        assert!(receiver.blob_has(hash).unwrap());
        // The store was opened using the original policy. Each independent
        // limit must reach its already installed GC callback.
        receiver.set_file_retention_policy(policy).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(12);
        while receiver.blob_has(hash).unwrap() {
            assert!(
                std::time::Instant::now() < deadline,
                "collector did not release the completed fetch for {policy:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(host.blob_has(hash).unwrap());
    }
    // GC operates on receiver cache, never the sender's currently approved offer.
    assert!(host.blob_has(hash).unwrap());
    assert_eq!(
        receiver
            .blob_fetch(&host.endpoint_id(), hash, size)
            .unwrap(),
        data
    );
}
