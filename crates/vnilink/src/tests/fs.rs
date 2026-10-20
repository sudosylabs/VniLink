//! Stage B slice 1: local filesystem service proofs.
//!
//! Every case asserts on-disk state: hashes, inodes, exact listings and
//! preserved originals. Fixtures live in unique temporary directories.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use iroh::RelayMode;

use super::filesystem_location::FilesystemLocation;
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
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("codec flags");
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

fn aside_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".vnldelete."))
        .collect()
}

fn is_message(error: &Error, expected: &str) -> bool {
    matches!(error, Error::InvalidSetting(message) if *message == expected)
}

fn token_of(core: &Core, root: u64, path: &str, name: &str) -> EntryToken {
    let token = core.stat_entry(root, path, name).unwrap().token;
    assert!(
        token.is_strong(),
        "fixture entries must carry native identity"
    );
    token
}

fn is_stale(error: &Error) -> bool {
    matches!(error, Error::InvalidSetting(message) if *message == crate::fs::STALE_ENTRY)
}

#[test]
fn whole_filesystem_browses_siblings_without_folder_configuration() {
    let scratch = Scratch::new();
    let left = scratch.join("left");
    let right = scratch.join("right");
    std::fs::create_dir(&left).unwrap();
    std::fs::create_dir(&right).unwrap();
    std::fs::write(right.join("note.txt"), b"sibling bytes").unwrap();
    let core = start_direct();
    assert!(core.file_roots().unwrap().is_empty());
    core.enable_filesystem_access().unwrap();
    let left = FilesystemLocation::new(core.file_roots().unwrap(), &left);
    let right = FilesystemLocation::new(core.file_roots().unwrap(), &right);
    assert!(core
        .list_dir(left.root, &left.path(""), 0, 20)
        .unwrap()
        .entries
        .is_empty());
    let page = core.list_dir(right.root, &right.path(""), 0, 20).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].name, "note.txt");
    core.create_dir(right.root, &right.path(""), "created")
        .unwrap();
    assert!(scratch.join("right/created").is_dir());
    // Malformed wire paths and unknown roots are still rejected. Whole
    // filesystem access removes folder configuration, not input validation.
    for bad in ["..", "a/../b", "/absolute", "a//b"] {
        assert!(core.list_dir(right.root, bad, 0, 20).is_err());
    }
    assert!(core.list_dir(u64::MAX, "", 0, 20).is_err());
}

#[test]
#[cfg(unix)]
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let page = core.list_dir(files.root, &files.path(""), 0, 50).unwrap();
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
    let nested = core
        .list_dir(files.root, &files.path("nested/deep"), 0, 50)
        .unwrap();
    assert_eq!(nested.entries.len(), 1);
    assert_eq!(nested.entries[0].name, "inner.txt");
    assert!(core
        .list_dir(files.root, &files.path("empty"), 0, 50)
        .unwrap()
        .entries
        .is_empty());

    // Paging walks the same order two entries at a time.
    let first = core.list_dir(files.root, &files.path(""), 0, 2).unwrap();
    assert_eq!(first.next_offset, Some(2));
    let second = core.list_dir(files.root, &files.path(""), 2, 2).unwrap();
    assert_eq!(second.next_offset, Some(4));
    let third = core.list_dir(files.root, &files.path(""), 4, 2).unwrap();
    assert_eq!(third.next_offset, None);
    let mut all: Vec<String> = vec![];
    all.extend(first.entries.iter().map(|e| e.name.clone()));
    all.extend(second.entries.iter().map(|e| e.name.clone()));
    all.extend(third.entries.iter().map(|e| e.name.clone()));
    assert_eq!(all.len(), 6);
    assert_eq!(all, names);

    assert!(core.list_dir(files.root, &files.path(""), 0, 0).is_err());
    assert!(core.list_dir(files.root, &files.path(""), 0, 501).is_err());
    assert!(format!("{page:?}").contains("beta") == false);
}

static LIST_PARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LIST_MAX_UNIT: AtomicU64 = AtomicU64::new(0);
static LIST_FINISHED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
static LIST_FINISH_CV: std::sync::Condvar = std::sync::Condvar::new();

fn list_park_probe(unit: u64, cancelled: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering as Seq;
    LIST_MAX_UNIT.fetch_max(unit, Seq::SeqCst);
    if LIST_PARKED.swap(true, Seq::SeqCst) {
        return;
    }
    while !cancelled.load(Seq::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn list_finish_probe(cancelled: bool) {
    *LIST_FINISHED.lock().unwrap() = Some(cancelled);
    LIST_FINISH_CV.notify_all();
}

/// STD08: detached listing yields the executor and holds no shared lock
/// (a sibling listing completes while the first is parked), and dropping
/// the future aborts the worker before anything is enumerated.
#[test]
fn parked_list_yields_sibling_and_cancels_before_enumeration() {
    use std::sync::{Arc, Mutex};

    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("alpha.txt"), b"a").unwrap();
    std::fs::write(root.join("beta.txt"), b"beta-bytes").unwrap();
    let mut roots = crate::fs::RootSet::default();
    roots.set_filesystem().unwrap();
    let files = FilesystemLocation::new(roots.snapshot(), &root);
    // Stand-in for the shared file-roots lock: the parked listing must
    // hold no lock across its worker await, so the sibling locks,
    // snapshots and completes while the first stays parked.
    let shared = Arc::new(Mutex::new(roots));
    let first = shared.lock().unwrap().clone();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let hooks = crate::fs::ListHooks {
        entry_probe: Some(list_park_probe),
        finish_probe: Some(list_finish_probe),
    };
    let (handle_tx, handle_rx) = std::sync::mpsc::channel();
    let (sibling_tx, sibling_rx) = std::sync::mpsc::channel();
    let sibling_shared = shared.clone();
    // A current-thread runtime only drives tasks under `block_on`; the
    // driver thread owns it while the test observes from outside.
    let driver = std::thread::spawn(move || {
        runtime.block_on(async move {
            let sibling_files = files.clone();
            let list = tokio::spawn(crate::fs::list_page_async(
                first,
                files.root,
                files.path(""),
                0,
                50,
                hooks,
            ));
            handle_tx.send(list).unwrap();
            let cap = std::time::Instant::now() + Duration::from_secs(30);
            while !LIST_PARKED.load(Ordering::SeqCst) {
                if std::time::Instant::now() > cap {
                    return;
                }
                tokio::task::yield_now().await;
            }
            // Spawned only after the park is observed: it must lock the
            // shared roots, snapshot and finish while parked.
            tokio::spawn(async move {
                let snapshot = sibling_shared.lock().unwrap().clone();
                let page = crate::fs::list_page_async(
                    snapshot,
                    sibling_files.root,
                    sibling_files.path(""),
                    0,
                    50,
                    crate::fs::ListHooks::none(),
                )
                .await;
                let _ = sibling_tx.send(page.map(|page| page.total));
            });
            while LIST_FINISHED.lock().unwrap().is_none() {
                if std::time::Instant::now() > cap {
                    return;
                }
                tokio::task::yield_now().await;
            }
        });
    });
    // The worker must reach the parked probe; on the old inline path no
    // probe exists and this times out.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !LIST_PARKED.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "list worker never parked"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Sibling progress while parked: the async worker is free and no
    // shared lock is held, so a full second listing completes.
    let sibling = sibling_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("sibling blocked during parked list");
    assert_eq!(
        sibling.expect("sibling list failed"),
        2,
        "sibling list result changed while parked"
    );
    // Cancellation aborts the worker without enumerating anything: only
    // the pre-access probe fired.
    let list = handle_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    list.abort();
    let finished = LIST_FINISHED.lock().unwrap();
    let (finished, timeout) = LIST_FINISH_CV
        .wait_timeout_while(finished, Duration::from_secs(5), |done| done.is_none())
        .unwrap();
    assert!(
        !timeout.timed_out(),
        "list worker never finished after abort"
    );
    assert_eq!(
        *finished,
        Some(true),
        "list worker did not report cancellation"
    );
    drop(finished);
    assert_eq!(
        LIST_MAX_UNIT.load(Ordering::SeqCst),
        0,
        "parked list enumerated entries"
    );
    driver.join().expect("driver thread panicked");
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    core.create_dir(files.root, &files.path(""), "docs")
        .unwrap();
    assert!(root.join("docs").is_dir());
    core.create_dir(files.root, &files.path("docs"), "2026")
        .unwrap();
    assert!(root.join("docs/2026").is_dir());

    assert!(matches!(
        core.create_dir(files.root, &files.path(""), "docs"),
        Err(Error::FileExists)
    ));
    std::fs::write(root.join("file.txt"), b"f").unwrap();
    assert!(matches!(
        core.create_dir(files.root, &files.path(""), "file.txt"),
        Err(Error::FileExists)
    ));
    assert!(core.create_dir(files.root, &files.path(""), "a/b").is_err());
    assert!(core.create_dir(files.root, &files.path(""), "..").is_err());
    assert!(core
        .create_dir(files.root, &files.path("missing-parent"), "x")
        .is_err());
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
    assert!(core.delete_entry(0, "real", "evil.txt", None).is_err());
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
    assert!(core.delete_entry(0, "", "anything", None).is_err());
}

#[test]
fn rename_file_keeps_identity_and_bytes() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("before.txt"), b"vnl-rename-bytes").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let before_ino = ino_of(&root.join("before.txt"));
    let token = token_of(&core, files.root, &files.path(""), "before.txt");
    core.rename_entry(
        files.root,
        &files.path(""),
        "before.txt",
        Some(token),
        "after.txt",
    )
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let child = ino_of(&root.join("pack/sub/a.txt"));
    let token = token_of(&core, files.root, &files.path(""), "pack");
    core.rename_entry(files.root, &files.path(""), "pack", Some(token), "box")
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    // Preflight: the destination is free when first checked.
    let token = token_of(&core, files.root, &files.path(""), "source.txt");
    // Another actor creates the destination before the commit.
    std::fs::write(root.join("dest.txt"), b"dest-bytes").unwrap();

    let result = core.rename_entry(
        files.root,
        &files.path(""),
        "source.txt",
        Some(token),
        "dest.txt",
    );
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let stale = token_of(&core, files.root, &files.path(""), "note.txt");
    // Barrier-controlled replacement between list and mutation: the
    // replacement is born while the original is still alive, so the two
    // objects cannot share an identity even where the filesystem
    // recycles inode numbers immediately after unlink.
    std::fs::write(root.join("note.tmp"), b"replacement-bytes").unwrap();
    let fresh = token_of(&core, files.root, &files.path(""), "note.tmp");
    assert_ne!(stale, fresh);
    std::fs::remove_file(root.join("note.txt")).unwrap();
    std::fs::rename(root.join("note.tmp"), root.join("note.txt")).unwrap();
    assert_eq!(
        token_of(&core, files.root, &files.path(""), "note.txt"),
        fresh
    );

    let result = core.rename_entry(
        files.root,
        &files.path(""),
        "note.txt",
        Some(stale),
        "renamed.txt",
    );
    assert!(is_stale(&result.unwrap_err()));
    assert!(!root.join("renamed.txt").exists());
    assert_eq!(read(&root.join("note.txt")), b"replacement-bytes");

    // Without a fencing token the current object renames (the UI always
    // passes the listing token; unfenced callers own the race).
    core.rename_entry(files.root, &files.path(""), "note.txt", None, "renamed.txt")
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    assert!(core
        .rename_entry(files.root, &files.path(""), "ghost.txt", None, "x.txt")
        .is_err());
    assert!(core
        .rename_entry(files.root, &files.path(""), "kept.txt", None, "../out.txt")
        .is_err());
    assert!(core
        .rename_entry(files.root, &files.path(""), "kept.txt", None, "sub/x.txt")
        .is_err());
    assert!(core
        .rename_entry(files.root, &files.path(""), "../kept.txt", None, "x.txt")
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
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let before = ino_of(&root.join("Case.txt"));
    let token = token_of(&core, files.root, &files.path(""), "Case.txt");
    match core.rename_entry(
        files.root,
        &files.path(""),
        "Case.txt",
        Some(token),
        "CASE.txt",
    ) {
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
        WireRequest::Delete {
            id: 11,
            generation: 4,
            root: 0,
            path: "docs".to_string(),
            name: "a.txt".to_string(),
            expected: Some(EntryToken {
                dev: Some(3),
                ino: Some(9),
            }),
            trash: false,
        },
        WireRequest::Delete {
            id: 12,
            generation: 4,
            root: 0,
            path: "".to_string(),
            name: "a.txt".to_string(),
            expected: None,
            trash: true,
        },
        WireRequest::Places {
            id: 13,
            generation: 4,
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
    let huge = "a".repeat(crate::fs::MAX_REL_BYTES + 1);
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
fn places_body_round_trips_and_refuses_unsafe_folders() {
    let places = crate::fs::Places {
        roots: vec![
            crate::fs::PlaceRoot {
                id: 2,
                name: "C:".to_owned(),
            },
            crate::fs::PlaceRoot {
                id: 3,
                name: "D:".to_owned(),
            },
        ],
        folders: vec![
            crate::fs::Place {
                kind: crate::fs::FolderKind::Home,
                root: 2,
                path: "Users/abass".to_owned(),
            },
            crate::fs::Place {
                kind: crate::fs::FolderKind::Downloads,
                root: 2,
                path: "Users/abass/Downloads".to_owned(),
            },
            crate::fs::Place {
                kind: crate::fs::FolderKind::Volume,
                root: 2,
                path: "Volumes/Backup".to_owned(),
            },
        ],
    };
    let body = crate::fs::encode_places_body(&places).unwrap();
    assert_eq!(crate::fs::decode_places_body(&body).unwrap(), places);
    let mut escaping = places.clone();
    escaping.folders[0].path = "../outside".to_owned();
    let body = crate::fs::encode_places_body(&escaping).unwrap();
    assert!(crate::fs::decode_places_body(&body).is_err());
    let mut trailing = crate::fs::encode_places_body(&places).unwrap();
    trailing.push(0);
    assert!(crate::fs::decode_places_body(&trailing).is_err());
    // A places request carries no root or path of its own.
    let mut rooted = crate::fs::encode_request(&crate::fs::Request::Places {
        id: 1,
        generation: 1,
    })
    .unwrap();
    rooted[21] = 1;
    assert!(crate::fs::decode_request(&rooted).is_err());
}

/// Home and its usual folders are named by root and path, and each opens.
#[test]
fn places_name_home_and_its_folders_by_root() {
    let core = start_direct();
    assert!(
        core.places().unwrap().roots.is_empty(),
        "nothing is shared yet"
    );
    core.enable_filesystem_access().unwrap();
    let places = core.places().unwrap();
    let roots: Vec<u64> = core
        .file_roots()
        .unwrap()
        .iter()
        .map(|root| root.0)
        .collect();
    assert_eq!(
        places.roots.iter().map(|root| root.id).collect::<Vec<_>>(),
        roots
    );
    #[cfg(unix)]
    assert_eq!(places.roots[0].name, "/");
    let Some(home) = places.folders.first() else {
        return;
    };
    assert_eq!(home.kind, crate::fs::FolderKind::Home);
    let account = std::fs::canonicalize(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap(),
    )
    .unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &account);
    assert_eq!(
        (home.root, home.path.as_str()),
        (location.root, location.path("").as_str())
    );
    for folder in &places.folders {
        assert!(
            core.list_dir(folder.root, &folder.path, 0, 1).is_ok(),
            "{:?} does not open",
            folder.kind
        );
    }
}

/// A whole disk is deeper than a shared folder: 40 levels below a scratch
/// folder still list, and paths stop at 64 levels.
#[test]
fn deep_folders_list_and_the_depth_limit_holds() {
    let scratch = Scratch::new();
    let mut deep = scratch.path.clone();
    for level in 0..40 {
        deep.push(format!("level-{level}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("bottom.txt"), b"deep").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &deep);
    let page = core
        .list_dir(location.root, &location.path(""), 0, 10)
        .unwrap();
    assert_eq!(page.entries[0].name, "bottom.txt");
    let too_deep = vec!["a"; 65].join("/");
    assert!(core.list_dir(location.root, &too_deep, 0, 10).is_err());
    assert!(crate::fs::check_rel(&vec!["a"; 64].join("/")).is_ok());
}

#[test]
fn list_body_round_trips() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"abc").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);
    let page = core.list_dir(files.root, &files.path(""), 0, 50).unwrap();
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
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
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
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
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
    host.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &root);
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    // Browse the remote root through the authenticated session.
    let page = controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].name, "report.txt");
    let token = page.entries[0].token;

    // Remote mkdir lands on the host disk.
    controller
        .fs_mkdir(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            "archive",
        )
        .unwrap();
    assert!(root.join("archive").is_dir());

    // Remote rename is a true rename: same object, same bytes on the host.
    let before = ino_of(&root.join("report.txt"));
    controller
        .fs_rename(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
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
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
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
    host.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &root);
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    // A stale generation never reaches the disk.
    let stale = controller.fs_list(
        session.id,
        session.generation + 1,
        files.root,
        &files.path(""),
        0,
        50,
    );
    assert!(matches!(stale, Err(Error::StaleSession)));
    let stale_rename = controller.fs_rename(
        session.id,
        session.generation + 1,
        files.root,
        &files.path(""),
        "kept.txt",
        None,
        "moved.txt",
    );
    assert!(matches!(stale_rename, Err(Error::StaleSession)));
    let stale_delete = controller.fs_delete(
        session.id,
        session.generation + 1,
        files.root,
        &files.path(""),
        "kept.txt",
        None,
    );
    assert!(matches!(stale_delete, Err(Error::StaleSession)));
    assert_eq!(read(&root.join("kept.txt")), b"kept-bytes");
    assert!(aside_names(&root).is_empty());

    // Traversal past the approved root is refused on the host.
    assert!(controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(".."),
            0,
            50
        )
        .is_err());
    assert!(controller
        .fs_rename(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            "kept.txt",
            None,
            "../out.txt"
        )
        .is_err());
    assert!(controller
        .fs_delete(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            "../kept.txt",
            None,
        )
        .is_err());
    assert!(!scratch.join("out.txt").exists());
    assert_eq!(read(&root.join("kept.txt")), b"kept-bytes");

    // Revocation stops new filesystem work mid-session.
    host.revoke(&controller.endpoint_id()).unwrap();
    // The control session is stopped by revocation too; any follow-up
    // filesystem request fails instead of touching the disk.
    let after = controller.fs_list(
        session.id,
        session.generation,
        files.root,
        &files.path(""),
        0,
        50,
    );
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
    host.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &root);
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    let page = controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
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
        files.root,
        &files.path(""),
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
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
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
        files.root,
        &files.path(""),
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
    host.set_desktop_sharing(crate::session::Grants {
        view: false,
        control: false,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
    host.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &root);
    // Files-only connects without Control grant and without decoder requirement
    // (controller reports no decoder here? Actually test cores have decoder true
    // via set_video_codecs; files-only skips decoder check anyway).
    // Disable decoder to prove files-only needs none (desktop connect would fail).
    controller
        .set_video_codecs(crate::VideoCodecCaps::default())
        .unwrap();
    let session = controller.connect_files(&host.endpoint_id()).unwrap();
    let page = controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
        .unwrap();
    assert!(page.entries.iter().any(|e| e.name == "shared.txt"));
}

/// STD05: a directory larger than the list cap reports truncation honestly
/// from a single bounded pass, and an exactly full directory does not.
#[test]
fn large_directory_reports_truncated_honestly() {
    let scratch = Scratch::new();
    let big = scratch.join("big");
    let exact = scratch.join("exact");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::create_dir_all(&exact).unwrap();
    for i in 0..5001 {
        std::fs::write(big.join(format!("entry-{i:06}")), []).unwrap();
    }
    for i in 0..5000 {
        std::fs::write(exact.join(format!("entry-{i:06}")), []).unwrap();
    }
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &scratch.path);
    let root = files.root;
    let page = core.list_dir(root, &files.path("big"), 0, 500).unwrap();
    assert!(
        page.truncated,
        "5001-entry listing claims complete: total={}, returned={}",
        page.total,
        page.entries.len()
    );
    assert_eq!(page.total, 5000);
    assert_eq!(page.entries.len(), 500);
    assert_eq!(page.next_offset, Some(500));
    let page = core.list_dir(root, &files.path("exact"), 0, 500).unwrap();
    assert!(!page.truncated);
    assert_eq!(page.total, 5000);
    assert_eq!(page.entries.len(), 500);
}

/// Persisted pins cannot adopt a different directory at the same path.
#[test]
#[cfg(unix)]
fn persisted_root_pin_refuses_same_path_replacement() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    let mut roots = crate::fs::RootSet::default();
    roots.set(std::slice::from_ref(&root)).unwrap();
    let pin = roots.pin(0).unwrap();
    let persisted = serde_json::to_vec(&pin).unwrap();
    std::fs::rename(&root, scratch.join("original")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("valuable"), b"keep").unwrap();
    let pin: crate::fs::PinnedRoot = serde_json::from_slice(&persisted).unwrap();
    let mut restarted = crate::fs::RootSet::default();
    restarted.set(std::slice::from_ref(&root)).unwrap();
    assert!(restarted.verify_pin(&pin).is_err());
    roots.set(std::slice::from_ref(&root)).unwrap();
    assert!(roots.verify_pin(&pin).is_err());
    assert_eq!(std::fs::read(root.join("valuable")).unwrap(), b"keep");
}

#[test]
fn delete_removes_a_file_and_a_folder_tree() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("pack/sub")).unwrap();
    std::fs::create_dir(root.join("pack/empty")).unwrap();
    std::fs::write(root.join("pack/sub/a.txt"), b"a-bytes").unwrap();
    std::fs::write(root.join("pack/top.txt"), b"top-bytes").unwrap();
    std::fs::write(root.join("note.txt"), b"note-bytes").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let note = token_of(&core, files.root, &files.path(""), "note.txt");
    core.delete_entry(files.root, &files.path(""), "note.txt", Some(note))
        .unwrap();
    assert!(!root.join("note.txt").exists());

    let folder = token_of(&core, files.root, &files.path(""), "pack");
    core.delete_entry(files.root, &files.path(""), "pack", Some(folder))
        .unwrap();
    assert!(!root.join("pack").exists());
    assert!(aside_names(&root).is_empty());
    let names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(names.is_empty(), "{names:?}");
}

#[test]
fn delete_against_a_replaced_source_is_refused() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note.txt"), b"original-bytes").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let stale = token_of(&core, files.root, &files.path(""), "note.txt");
    std::fs::write(root.join("note.tmp"), b"replacement-bytes").unwrap();
    let fresh = token_of(&core, files.root, &files.path(""), "note.tmp");
    assert_ne!(stale, fresh);
    std::fs::remove_file(root.join("note.txt")).unwrap();
    std::fs::rename(root.join("note.tmp"), root.join("note.txt")).unwrap();

    let result = core.delete_entry(files.root, &files.path(""), "note.txt", Some(stale));
    assert!(is_stale(&result.unwrap_err()));
    assert_eq!(read(&root.join("note.txt")), b"replacement-bytes");
    assert!(aside_names(&root).is_empty());

    core.delete_entry(files.root, &files.path(""), "note.txt", None)
        .unwrap();
    assert!(!root.join("note.txt").exists());
}

#[test]
fn delete_missing_source_and_bad_names_fail_cleanly() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("kept.txt"), b"kept").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    assert!(is_message(
        &core
            .delete_entry(files.root, &files.path(""), "ghost.txt", None)
            .unwrap_err(),
        crate::fs::DELETE_FAILED
    ));
    assert!(core
        .delete_entry(files.root, &files.path(""), "../kept.txt", None)
        .is_err());
    assert!(core
        .delete_entry(files.root, &files.path(""), "sub/x.txt", None)
        .is_err());
    assert_eq!(read(&root.join("kept.txt")), b"kept");
    assert!(aside_names(&root).is_empty());
    assert!(!scratch.join("kept.txt").exists());
}

#[test]
#[cfg(unix)]
fn delete_refuses_a_listed_symlink() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    let outside = scratch.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret-bytes").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link")).unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let token = token_of(&core, files.root, &files.path(""), "link");
    let result = core.delete_entry(files.root, &files.path(""), "link", Some(token));
    assert!(is_message(&result.unwrap_err(), crate::fs::DELETE_FAILED));
    assert!(std::fs::symlink_metadata(root.join("link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(read(&outside.join("secret.txt")), b"secret-bytes");
}

#[test]
#[cfg(unix)]
fn delete_folder_unlinks_symlink_children_without_following() {
    let scratch = Scratch::new();
    let root = scratch.join("root");
    let outside = scratch.join("outside");
    std::fs::create_dir_all(root.join("pack")).unwrap();
    std::fs::create_dir_all(outside.join("dir")).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret-bytes").unwrap();
    std::fs::write(outside.join("dir/nested.txt"), b"nested-bytes").unwrap();
    std::fs::write(root.join("pack/real.txt"), b"real-bytes").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("pack/link")).unwrap();
    std::os::unix::fs::symlink(&outside.join("dir"), root.join("pack/dirlink")).unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(core.file_roots().unwrap(), &root);

    let token = token_of(&core, files.root, &files.path(""), "pack");
    core.delete_entry(files.root, &files.path(""), "pack", Some(token))
        .unwrap();
    assert!(!root.join("pack").exists());
    assert!(aside_names(&root).is_empty());
    assert_eq!(read(&outside.join("secret.txt")), b"secret-bytes");
    assert_eq!(read(&outside.join("dir/nested.txt")), b"nested-bytes");
}

#[test]
fn delete_during_active_transfer_is_refused() {
    let scratch = Scratch::new();
    let host_root = scratch.join("host");
    let controller_root = scratch.join("controller");
    std::fs::create_dir_all(&host_root).unwrap();
    std::fs::create_dir_all(&controller_root).unwrap();
    std::fs::write(host_root.join("a.txt"), b"transfer-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    host.enable_filesystem_access().unwrap();
    controller.enable_filesystem_access().unwrap();
    host.set_transfer_directory(scratch.join("host-blobs"))
        .unwrap();
    controller
        .set_transfer_directory(scratch.join("controller-blobs"))
        .unwrap();
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &host_root);
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &controller_root);
    pair_with_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    controller.connect(&host.endpoint_id()).unwrap();

    let token = token_of(&host, host_files.root, &host_files.path(""), "a.txt");
    let job = controller
        .job_submit_download(
            &host.endpoint_id(),
            host_files.root,
            &host_files.path("a.txt"),
            controller_files.root,
            &controller_files.path(""),
        )
        .unwrap();
    let host_job = host.job_list().unwrap();
    assert_eq!(host_job.len(), 1);
    assert!(!host_job[0].state.is_terminal());

    let blocked = host.delete_entry(host_files.root, &host_files.path(""), "a.txt", Some(token));
    assert!(is_message(
        &blocked.unwrap_err(),
        "the entry is used by an active transfer"
    ));
    assert_eq!(read(&host_root.join("a.txt")), b"transfer-bytes");
    assert!(aside_names(&host_root).is_empty());

    controller.job_cancel(job).unwrap();
    let host_state = host.job_list().unwrap()[0].state;
    assert!(host_state.is_terminal(), "{host_state:?}");
    host.delete_entry(host_files.root, &host_files.path(""), "a.txt", Some(token))
        .unwrap();
    assert!(!host_root.join("a.txt").exists());
}

#[test]
fn remote_delete_removes_the_host_entry_and_keeps_a_replacement() {
    let scratch = Scratch::new();
    let root = scratch.join("host-root");
    std::fs::create_dir_all(root.join("pack")).unwrap();
    std::fs::write(root.join("pack/child.txt"), b"child-bytes").unwrap();
    std::fs::write(root.join("report.txt"), b"report-bytes").unwrap();

    let host = start_direct();
    let controller = start_direct();
    host.enable_filesystem_access().unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &root);
    pair_with_transfer(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();

    let page = controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
        .unwrap();
    let report = page
        .entries
        .iter()
        .find(|entry| entry.name == "report.txt")
        .unwrap()
        .token;
    let pack = page
        .entries
        .iter()
        .find(|entry| entry.name == "pack")
        .unwrap()
        .token;

    std::fs::write(root.join("report.tmp"), b"replacement-bytes").unwrap();
    std::fs::remove_file(root.join("report.txt")).unwrap();
    std::fs::rename(root.join("report.tmp"), root.join("report.txt")).unwrap();
    let refused = controller.fs_delete(
        session.id,
        session.generation,
        files.root,
        &files.path(""),
        "report.txt",
        Some(report),
    );
    assert!(refused.is_err());
    assert_eq!(read(&root.join("report.txt")), b"replacement-bytes");
    assert!(aside_names(&root).is_empty());

    let fresh = controller
        .fs_list(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            0,
            50,
        )
        .unwrap();
    let current = fresh
        .entries
        .iter()
        .find(|entry| entry.name == "report.txt")
        .unwrap()
        .token;
    controller
        .fs_delete(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            "report.txt",
            Some(current),
        )
        .unwrap();
    controller
        .fs_delete(
            session.id,
            session.generation,
            files.root,
            &files.path(""),
            "pack",
            Some(pack),
        )
        .unwrap();
    assert!(!root.join("report.txt").exists());
    assert!(!root.join("pack").exists());
    assert!(aside_names(&root).is_empty());
}

/// A path from the system's file manager is named as the panes name an
/// item: its folder's root and path, and its own name. A link keeps its
/// own name rather than its target's, and a path outside what is shared
/// is not named at all.
#[test]
fn a_dropped_path_is_named_by_its_folder_and_its_own_name() {
    let scratch = Scratch::new();
    let folder = scratch.join("dropped");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("note.txt"), b"note").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &folder);
    assert_eq!(
        core.locate(&folder.join("note.txt")).unwrap(),
        Some((location.root, location.path(""), "note.txt".into()))
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(folder.join("note.txt"), folder.join("shortcut")).unwrap();
        assert_eq!(
            core.locate(&folder.join("shortcut")).unwrap(),
            Some((location.root, location.path(""), "shortcut".into()))
        );
    }
    let shared = scratch.join("shared");
    std::fs::create_dir(&shared).unwrap();
    core.set_file_roots(vec![shared]).unwrap();
    assert_eq!(core.locate(&folder.join("note.txt")).unwrap(), None);
}

/// Moving to the Trash checks the entry as deleting does: one replaced
/// since it was listed is refused before anything reaches the Trash, and
/// so is a name that is not there. (The move itself would use this
/// account's real Trash, so it is left to manual checks.)
#[test]
fn moving_to_the_trash_refuses_what_changed_since_it_was_listed() {
    let scratch = Scratch::new();
    let folder = scratch.join("trash");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), b"first").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &folder);
    let listed = core
        .list_dir(location.root, &location.path(""), 0, 10)
        .unwrap()
        .entries[0]
        .token;
    std::fs::remove_file(folder.join("a.txt")).unwrap();
    std::fs::write(folder.join("a.txt"), b"replacement").unwrap();
    assert!(matches!(
        core.trash_entry(location.root, &location.path(""), "a.txt", Some(listed)),
        Err(Error::InvalidSetting(message)) if message.contains("changed")
    ));
    assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"replacement");
    assert!(core
        .trash_entry(location.root, &location.path(""), "missing.txt", None)
        .is_err());
}

/// A watched folder on this computer says when an entry comes or goes,
/// made by anything, and stops once unwatched.
#[test]
fn a_watched_folder_says_when_it_changes() {
    let scratch = Scratch::new();
    let folder = scratch.join("watched");
    std::fs::create_dir(&folder).unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &folder);
    let events = core.subscribe().unwrap();
    let watch = core
        .watch_folder(location.root, &location.path(""))
        .unwrap();
    let changed = |events: &crate::events::EventSubscription, seconds: f32| {
        let until = std::time::Instant::now() + Duration::from_secs_f32(seconds);
        while std::time::Instant::now() < until {
            if let Some(crate::CoreEvent::FolderChanged { watch: seen }) =
                events.recv_timeout(Duration::from_millis(100))
            {
                if seen == watch {
                    return true;
                }
            }
        }
        false
    };
    // FSEvents may report the watch's own start; let it settle first.
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_some() {}
    std::fs::write(folder.join("arrived.txt"), b"new").unwrap();
    assert!(changed(&events, 5.0), "creating a file was not reported");
    std::fs::remove_file(folder.join("arrived.txt")).unwrap();
    assert!(changed(&events, 5.0), "deleting a file was not reported");
    core.unwatch_folder(watch);
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_some() {}
    std::fs::write(folder.join("later.txt"), b"later").unwrap();
    assert!(
        !changed(&events, 1.5),
        "an unwatched folder still reported changes"
    );
}

/// VniLink's own temporary entries are never listed, and writing them is
/// not a change to the folder: a pane neither shows `.vnlpub.0.tmp` while
/// a copy is staged beside its destination, nor lists its folder again for
/// it. The rename that lands the file is a change.
#[test]
fn temporary_entries_are_never_listed_or_watched() {
    let scratch = Scratch::new();
    let folder = scratch.join("landing");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("kept.txt"), b"kept").unwrap();
    for name in [
        ".vnlpub.0.tmp",
        ".vnlpart.7.3",
        ".vnlcase.1.tmp",
        ".vnldelete.0f",
    ] {
        std::fs::write(folder.join(name), b"temp").unwrap();
    }
    // Names that only resemble them are the user's.
    std::fs::write(folder.join(".vnlrc"), b"mine").unwrap();
    std::fs::write(folder.join("pairing.vnl"), b"mine").unwrap();
    let core = start_direct();
    core.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(core.file_roots().unwrap(), &folder);
    let page = core
        .list_dir(location.root, &location.path(""), 0, 50)
        .unwrap();
    let names: Vec<&str> = page.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, [".vnlrc", "kept.txt", "pairing.vnl"]);
    assert_eq!(page.total, 3);
    assert_eq!(page.non_utf8_skipped, 0);

    let events = core.subscribe().unwrap();
    let watch = core
        .watch_folder(location.root, &location.path(""))
        .unwrap();
    let changed = |seconds: f32| {
        let until = std::time::Instant::now() + Duration::from_secs_f32(seconds);
        while std::time::Instant::now() < until {
            if let Some(crate::CoreEvent::FolderChanged { watch: seen }) =
                events.recv_timeout(Duration::from_millis(100))
            {
                if seen == watch {
                    return true;
                }
            }
        }
        false
    };
    // The watch may report its own start; let it settle first.
    std::thread::sleep(Duration::from_millis(300));
    while events.try_recv().is_some() {}
    let staged = folder.join(".vnlpub.1.tmp");
    std::fs::write(&staged, vec![7u8; 64 * 1024]).unwrap();
    // kqueue on macOS names no entry reliably, so only where the watch
    // names what changed are writes to a temporary entry left out.
    #[cfg(not(target_os = "macos"))]
    {
        std::thread::sleep(Duration::from_millis(500));
        while events.try_recv().is_some() {}
        for _ in 0..4 {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&staged)
                .and_then(|mut file| std::io::Write::write_all(&mut file, b"more"))
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !changed(1.5),
            "writing a temporary entry was reported as a change"
        );
    }
    std::thread::sleep(Duration::from_millis(500));
    while events.try_recv().is_some() {}
    std::fs::rename(&staged, folder.join("landed.bin")).unwrap();
    assert!(changed(5.0), "landing the file was not reported");
    core.unwatch_folder(watch);
}
