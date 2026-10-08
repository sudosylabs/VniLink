//! Stage B slice 1: local filesystem service proofs.
//!
//! Every case asserts on-disk state: hashes, inodes, exact listings and
//! preserved originals. Fixtures live in unique temporary directories.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::fs::{EntryKind, EntryToken, RevisionGate};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_h264_support(true, true).expect("codec flags");
    core
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "vnilink-fs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(unix)]
fn ino_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path).ok().map(|meta| meta.ino())
}

#[cfg(not(unix))]
fn ino_of(_path: &Path) -> Option<u64> {
    None
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn token_of(core: &Core, root: u64, path: &str, name: &str) -> EntryToken {
    core.stat_entry(root, path, name).unwrap().token
}

fn is_stale(error: &Error) -> bool {
    matches!(error, Error::InvalidSetting(message) if *message == crate::fs::STALE_ENTRY)
}

#[test]
fn roots_start_empty_and_jail_traversal() {
    let scratch = Scratch::new();
    let core = start_direct();
    assert!(core.file_roots().unwrap().is_empty());
    assert!(matches!(
        core.list_dir(0, "", 0, 10),
        Err(Error::Unsupported(_))
    ));

    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    core.set_file_roots(vec![root.clone()]).unwrap();
    assert_eq!(core.file_roots().unwrap().len(), 1);

    for bad in [
        "..",
        "a/../../x",
        "/abs",
        "C:foo",
        "a\\b",
        "a/",
        "a//b",
        "a/./b",
    ] {
        assert!(
            core.list_dir(0, bad, 0, 10).is_err(),
            "listing {bad:?} must be refused"
        );
    }
    assert!(matches!(
        core.list_dir(7, "", 0, 10),
        Err(Error::InvalidSetting(_))
    ));
    assert!(!scratch.join("x").exists());

    // A symlink is not an acceptable root.
    #[cfg(unix)]
    {
        let link = scratch.join("linked-root");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(core.set_file_roots(vec![link]).is_err());
        // The good roots survive the failed call.
        assert_eq!(core.file_roots().unwrap().len(), 1);
        // Twice the same root is refused too.
        assert!(core.set_file_roots(vec![root.clone(), root]).is_err());
    }
}

#[test]
fn listings_are_exact_sorted_and_paged() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("beta.txt"), b"beta-bytes").unwrap();
    std::fs::write(root.join("alpha.txt"), b"a").unwrap();
    std::fs::write(root.join("sp ace-ß.txt"), b"unicode-bytes").unwrap();
    std::fs::write(root.join(".hidden"), b"h").unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::create_dir_all(root.join("nested/deep")).unwrap();
    std::fs::write(root.join("nested/deep/inner.txt"), b"inner").unwrap();

    let core = start_direct();
    core.set_file_roots(vec![root]).unwrap();

    let page = core.list_dir(0, "", 0, 50).unwrap();
    let names: Vec<&str> = page
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            ".hidden",
            "alpha.txt",
            "beta.txt",
            "empty",
            "nested",
            "sp ace-ß.txt"
        ]
    );
    assert_eq!(page.total, 6);
    assert_eq!(page.next_offset, None);
    assert!(!page.truncated);

    let beta = page.entries.iter().find(|e| e.name == "beta.txt").unwrap();
    assert_eq!(beta.kind, EntryKind::File);
    assert_eq!(beta.size, 10);
    let empty = page.entries.iter().find(|e| e.name == "empty").unwrap();
    assert_eq!(empty.kind, EntryKind::Dir);
    let nested = core.list_dir(0, "nested/deep", 0, 50).unwrap();
    assert_eq!(nested.entries.len(), 1);
    assert_eq!(nested.entries[0].name, "inner.txt");
    assert!(core.list_dir(0, "empty", 0, 50).unwrap().entries.is_empty());

    // Paging walks the same order two entries at a time.
    let first = core.list_dir(0, "", 0, 2).unwrap();
    assert_eq!(first.next_offset, Some(2));
    let second = core.list_dir(0, "", 2, 2).unwrap();
    assert_eq!(second.next_offset, Some(4));
    let third = core.list_dir(0, "", 4, 2).unwrap();
    assert_eq!(third.next_offset, None);
    let mut all: Vec<String> = vec![];
    all.extend(first.entries.iter().map(|e| e.name.clone()));
    all.extend(second.entries.iter().map(|e| e.name.clone()));
    all.extend(third.entries.iter().map(|e| e.name.clone()));
    assert_eq!(all.len(), 6);
    assert_eq!(all, names);

    assert!(core.list_dir(0, "", 0, 0).is_err());
    assert!(core.list_dir(0, "", 0, 501).is_err());
    assert!(format!("{page:?}").contains("beta") == false);
}

#[test]
#[cfg(unix)]
fn listings_skip_non_utf8_names_and_count_them() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("plain.txt"), b"p").unwrap();
    let raw = OsString::from_vec(vec![0x66, 0xFF, 0x62]);

    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();
    // APFS refuses non-UTF8 names at creation; the listing path is still
    // exercised where the platform allows the fixture.
    match std::fs::write(root.join(&raw), b"raw") {
        Err(_) => {
            let page = core.list_dir(0, "", 0, 50).unwrap();
            assert_eq!(page.entries.len(), 1);
            return;
        }
        Ok(()) => {}
    }
    let page = core.list_dir(0, "", 0, 50).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.non_utf8_skipped, 1);
}

#[test]
fn mkdir_creates_single_levels_with_conflicts() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    core.create_dir(0, "", "docs").unwrap();
    assert!(root.join("docs").is_dir());
    core.create_dir(0, "docs", "2026").unwrap();
    assert!(root.join("docs/2026").is_dir());

    assert!(matches!(
        core.create_dir(0, "", "docs"),
        Err(Error::FileExists)
    ));
    std::fs::write(root.join("file.txt"), b"f").unwrap();
    assert!(matches!(
        core.create_dir(0, "", "file.txt"),
        Err(Error::FileExists)
    ));
    assert!(core.create_dir(0, "", "a/b").is_err());
    assert!(core.create_dir(0, "", "..").is_err());
    assert!(core.create_dir(0, "missing-parent", "x").is_err());
}

#[test]
#[cfg(unix)]
fn symlink_parent_replacement_is_refused() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("real/sub")).unwrap();
    std::fs::write(root.join("real/sub/kept.txt"), b"kept").unwrap();
    let outside = scratch.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("evil.txt"), b"evil").unwrap();

    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();
    assert_eq!(core.list_dir(0, "real/sub", 0, 10).unwrap().total, 1);

    // Swap the parent for a symlink after listing.
    std::fs::remove_dir_all(root.join("real")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("real")).unwrap();
    assert!(core.list_dir(0, "real/sub", 0, 10).is_err());
    assert!(core.create_dir(0, "real", "nope").is_err());
    assert!(core
        .rename_entry(0, "real", "evil.txt", None, "moved.txt")
        .is_err());
    assert!(!root.join("moved.txt").exists());
    assert_eq!(read(&outside.join("evil.txt")), b"evil");
}

#[test]
#[cfg(unix)]
fn approved_root_replaced_by_symlink_cannot_escape() {
    let scratch = Scratch::new();
    let approved = scratch.join("approved");
    std::fs::create_dir(&approved).unwrap();
    let outside = scratch.join("outside");
    std::fs::create_dir(&outside).unwrap();

    let core = start_direct();
    core.set_file_roots(vec![approved.clone()]).unwrap();
    assert!(core.list_dir(0, "", 0, 10).is_ok());

    // Replace the approved folder itself with a symlink to outside.
    std::fs::remove_dir(&approved).unwrap();
    std::os::unix::fs::symlink(&outside, &approved).unwrap();

    // Every anchored operation must refuse instead of writing outside.
    assert!(core.list_dir(0, "", 0, 10).is_err());
    assert!(core.create_dir(0, "", "escape").is_err());
    assert!(!outside.join("escape").exists());
    assert!(core.rename_entry(0, "", "anything", None, "else").is_err());
}

#[test]
fn rename_file_keeps_identity_and_bytes() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("before.txt"), b"vnl-rename-bytes").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    let before_ino = ino_of(&root.join("before.txt"));
    let token = token_of(&core, 0, "", "before.txt");
    core.rename_entry(0, "", "before.txt", Some(token), "after.txt")
        .unwrap();

    assert!(!root.join("before.txt").exists());
    assert_eq!(read(&root.join("after.txt")), b"vnl-rename-bytes");
    assert_eq!(ino_of(&root.join("after.txt")), before_ino);
    // No temp or duplicate siblings leak.
    let names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["after.txt"]);
}

#[test]
fn rename_folder_moves_children_without_transfer() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("pack/sub")).unwrap();
    std::fs::create_dir(root.join("pack/empty")).unwrap();
    std::fs::write(root.join("pack/sub/a.txt"), b"a-bytes").unwrap();
    std::fs::write(root.join("pack/top.txt"), b"top-bytes").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    let child = ino_of(&root.join("pack/sub/a.txt"));
    let token = token_of(&core, 0, "", "pack");
    core.rename_entry(0, "", "pack", Some(token), "box")
        .unwrap();

    assert!(!root.join("pack").exists());
    assert!(root.join("box/empty").is_dir());
    assert_eq!(read(&root.join("box/sub/a.txt")), b"a-bytes");
    assert_eq!(read(&root.join("box/top.txt")), b"top-bytes");
    assert_eq!(ino_of(&root.join("box/sub/a.txt")), child);
}

#[test]
fn rename_collision_after_preflight_preserves_both() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source.txt"), b"source-bytes").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    // Preflight: the destination is free when first checked.
    let token = token_of(&core, 0, "", "source.txt");
    // Another actor creates the destination before the commit.
    std::fs::write(root.join("dest.txt"), b"dest-bytes").unwrap();

    let result = core.rename_entry(0, "", "source.txt", Some(token), "dest.txt");
    assert!(matches!(result, Err(Error::FileExists)));
    assert_eq!(read(&root.join("source.txt")), b"source-bytes");
    assert_eq!(read(&root.join("dest.txt")), b"dest-bytes");
}

#[test]
fn rename_against_a_replaced_source_is_refused() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note.txt"), b"original-bytes").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    let stale = token_of(&core, 0, "", "note.txt");
    // Barrier-controlled replacement between list and mutation: the
    // replacement is born while the original is still alive, so the two
    // objects cannot share an identity even where the filesystem
    // recycles inode numbers immediately after unlink.
    std::fs::write(root.join("note.tmp"), b"replacement-bytes").unwrap();
    let fresh = token_of(&core, 0, "", "note.tmp");
    assert_ne!(stale, fresh);
    std::fs::remove_file(root.join("note.txt")).unwrap();
    std::fs::rename(root.join("note.tmp"), root.join("note.txt")).unwrap();
    assert_eq!(token_of(&core, 0, "", "note.txt"), fresh);

    let result = core.rename_entry(0, "", "note.txt", Some(stale), "renamed.txt");
    assert!(is_stale(&result.unwrap_err()));
    assert!(!root.join("renamed.txt").exists());
    assert_eq!(read(&root.join("note.txt")), b"replacement-bytes");

    // Without a fencing token the current object renames (the UI always
    // passes the listing token; unfenced callers own the race).
    core.rename_entry(0, "", "note.txt", None, "renamed.txt")
        .unwrap();
    assert_eq!(read(&root.join("renamed.txt")), b"replacement-bytes");
}

#[test]
fn rename_missing_source_and_bad_names_fail_cleanly() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("kept.txt"), b"kept").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    assert!(core
        .rename_entry(0, "", "ghost.txt", None, "x.txt")
        .is_err());
    assert!(core
        .rename_entry(0, "", "kept.txt", None, "../out.txt")
        .is_err());
    assert!(core
        .rename_entry(0, "", "kept.txt", None, "sub/x.txt")
        .is_err());
    assert!(core
        .rename_entry(0, "", "../kept.txt", None, "x.txt")
        .is_err());
    assert_eq!(read(&root.join("kept.txt")), b"kept");
    assert!(!scratch.join("out.txt").exists());
}

#[test]
fn case_only_rename_keeps_the_object() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("Case.txt"), b"case-bytes").unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root.clone()]).unwrap();

    let before = ino_of(&root.join("Case.txt"));
    let token = token_of(&core, 0, "", "Case.txt");
    match core.rename_entry(0, "", "Case.txt", Some(token), "CASE.txt") {
        Ok(()) => {
            assert_eq!(read(&root.join("CASE.txt")), b"case-bytes");
            assert_eq!(ino_of(&root.join("CASE.txt")), before);
        }
        Err(Error::FileExists) => {
            // Case-sensitive filesystem treating the spellings as colliding
            // with a distinct entry, or a platform refusing in place: the
            // original must survive either way.
            assert_eq!(read(&root.join("Case.txt")), b"case-bytes");
        }
        Err(other) => panic!("unexpected case-only outcome: {other}"),
    }
}

#[test]
fn wire_codec_round_trips_and_rejects() {
    use crate::fs::{Request as WireRequest, Response as WireResponse};

    for request in [
        WireRequest::List {
            id: 7,
            generation: 3,
            root: 0,
            path: "docs/2026".to_string(),
            offset: 3,
            limit: 50,
        },
        WireRequest::Mkdir {
            id: 8,
            generation: 3,
            root: 1,
            path: "".to_string(),
            name: "new".to_string(),
        },
        WireRequest::Rename {
            id: 9,
            generation: 4,
            root: 0,
            path: "docs".to_string(),
            name: "a.txt".to_string(),
            expected: Some(EntryToken {
                dev: Some(3),
                ino: Some(9),
            }),
            new_name: "b.txt".to_string(),
        },
        WireRequest::Rename {
            id: 10,
            generation: 4,
            root: 0,
            path: "".to_string(),
            name: "a.txt".to_string(),
            expected: None,
            new_name: "b.txt".to_string(),
        },
    ] {
        let bytes = crate::fs::encode_request(&request).unwrap();
        assert_eq!(crate::fs::decode_request(&bytes).unwrap(), request);
    }

    let ok = WireResponse::ok(11);
    assert!(ok.is_ok());
    let bytes = crate::fs::encode_response(&ok);
    let back = crate::fs::decode_response(&bytes).unwrap();
    assert_eq!(back.id, 11);
    assert!(back.is_ok());

    // Wrong version, truncation, unknown tag and trailing bytes all fail.
    let mut bad = crate::fs::encode_request(&WireRequest::List {
        id: 1,
        generation: 1,
        root: 0,
        path: "".to_string(),
        offset: 0,
        limit: 10,
    })
    .unwrap();
    bad[0] = 99;
    assert!(crate::fs::decode_request(&bad).is_err());
    let truncated = &bad[..4];
    assert!(crate::fs::decode_request(truncated).is_err());
    let mut tagged = crate::fs::encode_request(&WireRequest::List {
        id: 1,
        generation: 1,
        root: 0,
        path: "".to_string(),
        offset: 0,
        limit: 10,
    })
    .unwrap();
    tagged[13] = 77;
    assert!(crate::fs::decode_request(&tagged).is_err());
    // Oversize text is refused at encode time, never sent.
    let huge = "a".repeat(2048);
    assert!(crate::fs::encode_request(&WireRequest::List {
        id: 1,
        generation: 1,
        root: 0,
        path: huge,
        offset: 0,
        limit: 10,
    })
    .is_err());
}

#[test]
fn list_body_round_trips() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"abc").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    let core = start_direct();
    core.set_file_roots(vec![root]).unwrap();
    let page = core.list_dir(0, "", 0, 50).unwrap();
    let bytes = crate::fs::encode_list_body(&page).unwrap();
    let back = crate::fs::decode_list_body(&bytes).unwrap();
    assert_eq!(back, page);
    assert!(crate::fs::decode_list_body(&bytes[..bytes.len() - 1]).is_err());
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(crate::fs::decode_list_body(&bad).is_err());
}

#[test]
fn revision_gate_fences_late_replies() {
    let mut gate = RevisionGate::new();
    assert!(!gate.accept(0));
    let first = gate.navigate();
    let second = gate.navigate();
    assert_ne!(first, second);
    // The delayed reply to the first navigation must not paint.
    assert!(!gate.accept(first));
    assert!(gate.accept(second));
}

#[test]
fn setting_roots_is_refused_during_an_active_session() {
    let host = start_direct();
    let controller = start_direct();
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        crate::session::Grants {
            control: true,
            clipboard: false,
            transfer: true,
        },
    )
    .unwrap();
    controller
        .connect(&host.endpoint_id())
        .expect("control session");
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    assert!(matches!(host.set_file_roots(vec![root]), Err(Error::Busy)));
}

fn pair_with_transfer(host: &Core, controller: &Core) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        crate::session::Grants {
            control: true,
            clipboard: false,
            transfer: true,
        },
    )
    .unwrap();
}

#[test]
fn remote_browse_mkdir_and_rename_change_the_host_disk() {
    let scratch = Scratch::new();
    let root = scratch.join("host-root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("report.txt"), b"report-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    host.set_file_roots(vec![root.clone()]).unwrap();
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    // Browse the remote root through the authenticated session.
    let page = controller
        .fs_list(session.id, session.generation, 0, "", 0, 50)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].name, "report.txt");
    let token = page.entries[0].token;

    // Remote mkdir lands on the host disk.
    controller
        .fs_mkdir(session.id, session.generation, 0, "", "archive")
        .unwrap();
    assert!(root.join("archive").is_dir());

    // Remote rename is a true rename: same object, same bytes on the host.
    let before = ino_of(&root.join("report.txt"));
    controller
        .fs_rename(
            session.id,
            session.generation,
            0,
            "",
            "report.txt",
            Some(token),
            "final.txt",
        )
        .unwrap();
    assert!(!root.join("report.txt").exists());
    assert_eq!(read(&root.join("final.txt")), b"report-bytes");
    assert_eq!(ino_of(&root.join("final.txt")), before);

    // The remote pane refreshes to the new state.
    let again = controller
        .fs_list(session.id, session.generation, 0, "", 0, 50)
        .unwrap();
    let names: Vec<&str> = again.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["archive", "final.txt"]);
}

#[test]
fn remote_requests_enforce_generation_grant_and_scope() {
    let scratch = Scratch::new();
    let root = scratch.join("host-root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("kept.txt"), b"kept-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    host.set_file_roots(vec![root.clone()]).unwrap();
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    // A stale generation never reaches the disk.
    let stale = controller.fs_list(session.id, session.generation + 1, 0, "", 0, 50);
    assert!(matches!(stale, Err(Error::StaleSession)));
    let stale_rename = controller.fs_rename(
        session.id,
        session.generation + 1,
        0,
        "",
        "kept.txt",
        None,
        "moved.txt",
    );
    assert!(matches!(stale_rename, Err(Error::StaleSession)));
    assert_eq!(read(&root.join("kept.txt")), b"kept-bytes");

    // Traversal past the approved root is refused on the host.
    assert!(controller
        .fs_list(session.id, session.generation, 0, "..", 0, 50)
        .is_err());
    assert!(controller
        .fs_rename(
            session.id,
            session.generation,
            0,
            "",
            "kept.txt",
            None,
            "../out.txt"
        )
        .is_err());
    assert!(!scratch.join("out.txt").exists());

    // Revocation stops new filesystem work mid-session.
    host.revoke(&controller.endpoint_id()).unwrap();
    // The control session is stopped by revocation too; any follow-up
    // filesystem request fails instead of touching the disk.
    let after = controller.fs_list(session.id, session.generation, 0, "", 0, 50);
    assert!(after.is_err());
    assert_eq!(read(&root.join("kept.txt")), b"kept-bytes");
}

#[test]
fn remote_rename_conflict_and_stale_token_hold() {
    let scratch = Scratch::new();
    let root = scratch.join("host-root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    host.set_file_roots(vec![root.clone()]).unwrap();
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    let page = controller
        .fs_list(session.id, session.generation, 0, "", 0, 50)
        .unwrap();
    let stale = page.entries[0].token;
    // The host-side object is replaced between listing and rename. The
    // replacement is born first so recycled inode numbers cannot make
    // the two objects compare identically.
    std::fs::write(root.join("a.tmp"), b"replacement-bytes").unwrap();
    std::fs::remove_file(root.join("a.txt")).unwrap();
    std::fs::rename(root.join("a.tmp"), root.join("a.txt")).unwrap();

    let refused = controller.fs_rename(
        session.id,
        session.generation,
        0,
        "",
        "a.txt",
        Some(stale),
        "b.txt",
    );
    assert!(refused.is_err());
    assert!(!root.join("b.txt").exists());
    assert_eq!(read(&root.join("a.txt")), b"replacement-bytes");

    // A destination created after the preflight is not overwritten.
    std::fs::write(root.join("b.txt"), b"b-bytes").unwrap();
    let fresh = controller
        .fs_list(session.id, session.generation, 0, "", 0, 50)
        .unwrap();
    let current = fresh
        .entries
        .iter()
        .find(|e| e.name == "a.txt")
        .unwrap()
        .token;
    let conflict = controller.fs_rename(
        session.id,
        session.generation,
        0,
        "",
        "a.txt",
        Some(current),
        "b.txt",
    );
    assert!(matches!(conflict, Err(Error::FileExists)));
    assert_eq!(read(&root.join("a.txt")), b"replacement-bytes");
    assert_eq!(read(&root.join("b.txt")), b"b-bytes");
}

#[test]
fn files_only_needs_transfer_not_control_or_decoder() {
    let scratch = Scratch::new();
    let root = scratch.join("host-root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("shared.txt"), b"shared-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    // Pair without Control: Transfer only for files-only.
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.authorize(
        &controller.endpoint_id(),
        crate::session::Grants {
            control: false,
            clipboard: false,
            transfer: true,
        },
    )
    .unwrap();
    host.set_file_roots(vec![root.clone()]).unwrap();
    // Files-only connects without Control grant and without decoder requirement
    // (controller reports no decoder here? Actually test cores have decoder true
    // via set_h264_support; files-only skips decoder check anyway).
    // Disable decoder to prove files-only needs none (desktop connect would fail).
    controller.set_h264_support(false, false).unwrap();
    let session = controller.connect_files(&host.endpoint_id()).unwrap();
    let page = controller
        .fs_list(session.id, session.generation, 0, "", 0, 50)
        .unwrap();
    assert!(page.entries.iter().any(|e| e.name == "shared.txt"));
}
