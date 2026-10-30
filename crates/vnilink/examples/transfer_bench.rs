// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Loopback file-transfer throughput, for comparing builds by hand.
//!
//! Boots two cores on loopback, pairs them with the Transfer grant, and
//! uploads a set of files from one to the other with both background job
//! drivers running, as the application runs them. It reports how long the
//! upload took to be accepted and to complete, with bytes and files per
//! second. Built as an example, not a test, so it measures the shipped
//! configuration rather than test settings.
//!
//! Run: `cargo run --release -p vnilink --features test-probes --example transfer_bench [files] [bytes_per_file]`
//! Defaults: 1,000 files of 4 KiB, then one file of 512 MiB, when no
//! arguments are given.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use vnilink::{
    Core, Grants, JobState, MemoryStore, RelaySettings, RemoteFilesService, ServiceInfo,
    ServiceKind, SessionPhase,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let runs: Vec<(usize, usize)> = match (args.get(1), args.get(2)) {
        (Some(files), Some(size)) => vec![(
            files.parse().expect("a file count"),
            size.parse().expect("a byte count"),
        )],
        _ => vec![(1000, 4096), (1, 512 * 1024 * 1024)],
    };
    for (files, size) in runs {
        run(files, size);
    }
}

fn run(files: usize, size: usize) {
    let scratch = Scratch::new();
    let host = start(&scratch.0.join("host"));
    let controller = start(&scratch.0.join("controller"));
    pair(&host, &controller);
    host.start_job_driver();
    controller.start_job_driver();

    let source = scratch.0.join("controller/root/set");
    std::fs::create_dir_all(&source).unwrap();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for n in 0..files {
        let bytes: Vec<u8> = (0..size)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        std::fs::write(source.join(format!("f{n:05}.bin")), bytes).unwrap();
    }
    // Both cores share their whole disk, as the application does; the
    // upload names folders inside this run's scratch folder.
    let (src_root, src_rel) = place(&controller, &source);
    let (dst_root, dst_rel) = place(&host, &scratch.0.join("host/root"));
    let (service, _pipe) = open_files(&controller, &host);
    let (src_folder, name) = src_rel.rsplit_once('/').expect("the set in its folder");
    let started = Instant::now();
    let job = controller
        .files_service_upload(
            service.id,
            service.generation,
            src_root,
            src_folder,
            &[name.to_owned()],
            dst_root,
            &dst_rel,
        )
        .unwrap();
    let accepted = started.elapsed();
    loop {
        let snapshot = controller.job_snapshot(job).unwrap();
        if snapshot.state.is_terminal() {
            assert_eq!(snapshot.state, JobState::Complete, "{:?}", snapshot.error);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(1200), "too slow");
        std::thread::sleep(Duration::from_millis(2));
    }
    let total = started.elapsed().as_secs_f64();
    println!(
        "{files} x {size} bytes: accepted in {:.0} ms, done in {:.0} ms, {:.1} MB/s, {:.0} files/s",
        accepted.as_secs_f64() * 1e3,
        total * 1e3,
        (files * size) as f64 / total / 1e6,
        files as f64 / total,
    );
    host.shutdown();
    controller.shutdown();
}

fn start(dir: &Path) -> Arc<Core> {
    std::fs::create_dir_all(dir.join("root")).unwrap();
    // Direct loopback only: no relay, so nothing leaves this computer.
    let core =
        Core::open_direct(Arc::new(MemoryStore::empty()), RelaySettings::public_n0()).unwrap();
    // A session needs codecs to open, though this one carries no video.
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "bench"))
        .unwrap();
    core.set_transfer_directory(dir.join("transfers")).unwrap();
    core.enable_filesystem_access().unwrap();
    Arc::new(core)
}

/// The controller's Files on `host`, as the Files tab reaches a computer:
/// a services connection and a Files service on it. The pipe keeps the
/// service, and so its job, alive until the upload is done.
fn open_files(controller: &Core, host: &Core) -> (ServiceInfo, RemoteFilesService) {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let connection = loop {
        let session = controller.controller_session(pending.id).unwrap();
        if session.phase == SessionPhase::Active {
            break session;
        }
        assert!(
            Instant::now() < deadline,
            "the connection did not become active: {session:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let owner = controller
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    (owner, pipe)
}

/// `path` as `core` names it on its shared disk: a root and the path below.
fn place(core: &Core, path: &Path) -> (u64, String) {
    let (root, folder, name) = core
        .locate(path)
        .unwrap()
        .expect("the scratch folder is on the shared disk");
    let rel = if folder.is_empty() {
        name
    } else {
        format!("{folder}/{name}")
    };
    (root, rel)
}

fn pair(host: &Core, controller: &Core) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    // A Files service opens, and files move, under transfer.
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
    controller
        .set_desktop_sharing(Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("vnilink-transfer-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(std::fs::canonicalize(dir).unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
